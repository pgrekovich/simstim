import { createRef } from "react";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { listen } from "@tauri-apps/api/event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { SessionView, type SessionViewHandle } from "@/components/SessionView";
import * as ipc from "@/lib/ipc";
import type { SessionData } from "@/lib/ipc";

vi.mock("@/lib/ipc", () => ({
  saveSession: vi.fn().mockResolvedValue(undefined),
  updateActiveSession: vi.fn().mockResolvedValue(undefined),
  readSession: vi.fn().mockResolvedValue({
    title: "",
    date: "",
    notes: "",
    transcript: "",
    audio_path: null,
  }),
  retranscribeSession: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

// SessionView feeds `<audio src>` through `convertFileSrc`; a real Tauri
// runtime isn't present under jsdom, so this stands in with something
// visibly derived from the input path -- good enough to assert the player
// is actually wired to `audio_path`, not just present.
vi.mock("@tauri-apps/api/core", () => ({
  convertFileSrc: vi.fn((path: string) => `asset://localhost/${path}`),
}));

const baseData: SessionData = {
  title: "Weekly sync",
  date: "",
  notes: "",
  transcript: "",
  audio_path: null,
};

/** Invokes the handler SessionView registered via `listen(name, handler)`. */
function emit(name: string, payload: unknown) {
  const call = vi.mocked(listen).mock.calls.find(([eventName]) => eventName === name);
  if (!call) throw new Error(`no listener registered for "${name}" -- was it ever mounted?`);
  const handler = call[1] as (event: { payload: unknown }) => void;
  act(() => {
    handler({ payload });
  });
}

beforeEach(() => {
  vi.mocked(ipc.saveSession).mockClear();
  vi.mocked(ipc.updateActiveSession).mockClear();
  vi.mocked(ipc.readSession).mockClear();
  vi.mocked(ipc.retranscribeSession).mockClear();
  vi.mocked(listen).mockClear();
});

describe("SessionView: stop-transition flush (Task 6 review fix)", () => {
  it("routes an unblurred, still-debouncing notes edit through update_active_session when imperatively flushed", async () => {
    // Reproduces the bug: App's Stop button can be clicked while the notes
    // textarea still has focus and its 500ms debounce timer hasn't fired
    // yet. App.handleStop must be able to force that edit out through the
    // *active* route (update_active_session) before it calls
    // stop_recording -- this is what SessionView's imperative
    // `flushPendingEdits` handle exists for.
    const ref = createRef<SessionViewHandle>();
    render(
      <SessionView
        ref={ref}
        path="/tmp/call.md"
        isActive
        initialData={baseData}
        liveLines={[]}
        interimMe=""
        interimThem=""
      />
    );

    const notes = screen.getByPlaceholderText("your notes...");
    fireEvent.change(notes, { target: { value: "call went well" } });
    // Deliberately no blur and no timer advance: the edit is still only
    // sitting in the debounce, exactly as it would be mid-keystroke.

    await ref.current?.flushPendingEdits();

    expect(ipc.updateActiveSession).toHaveBeenCalledWith(undefined, "call went well");
    expect(ipc.saveSession).not.toHaveBeenCalled();
  });

  it("does not re-flush a title already saved via blur (no redundant IPC call)", async () => {
    const ref = createRef<SessionViewHandle>();
    render(
      <SessionView
        ref={ref}
        path="/tmp/call.md"
        isActive
        initialData={baseData}
        liveLines={[]}
        interimMe=""
        interimThem=""
      />
    );

    const title = screen.getByPlaceholderText("Untitled");
    fireEvent.change(title, { target: { value: "New title" } });
    fireEvent.blur(title);
    expect(ipc.updateActiveSession).toHaveBeenCalledWith("New title", undefined);
    vi.mocked(ipc.updateActiveSession).mockClear();

    await ref.current?.flushPendingEdits();

    expect(ipc.updateActiveSession).not.toHaveBeenCalled();
  });
});

describe("SessionView: empty title normalization (Task 6 review fix)", () => {
  it("saves and displays 'Untitled' when the title is committed empty, for an inactive session", () => {
    render(
      <SessionView
        path="/tmp/call.md"
        isActive={false}
        initialData={{ ...baseData, title: "Old title" }}
        liveLines={[]}
        interimMe=""
        interimThem=""
      />
    );

    const title = screen.getByDisplayValue("Old title") as HTMLInputElement;
    fireEvent.change(title, { target: { value: "   " } });
    fireEvent.blur(title);

    expect(ipc.saveSession).toHaveBeenCalledWith("/tmp/call.md", "Untitled", "");
    expect(title.value).toBe("Untitled");
  });
});

describe("SessionView: audio playback and re-transcribe (Task 4, stage-0.7)", () => {
  it("renders no audio element and no re-transcribe button when audio_path is null", () => {
    render(
      <SessionView
        path="/tmp/call.md"
        isActive={false}
        initialData={baseData}
        liveLines={[]}
        interimMe=""
        interimThem=""
      />
    );

    expect(document.querySelector("audio")).toBeNull();
    expect(screen.queryByRole("button", { name: /re-transcribe/i })).toBeNull();
  });

  it("renders a compact audio player fed by convertFileSrc when audio_path is present", () => {
    render(
      <SessionView
        path="/tmp/call.md"
        isActive={false}
        initialData={{ ...baseData, audio_path: "/tmp/2026-08-15 call.flac" }}
        liveLines={[]}
        interimMe=""
        interimThem=""
      />
    );

    const audio = document.querySelector("audio");
    expect(audio).not.toBeNull();
    expect(audio).toHaveAttribute("controls");
    expect(audio).toHaveAttribute("preload", "metadata");
    expect(audio?.getAttribute("src")).toBe("asset://localhost//tmp/2026-08-15 call.flac");
  });

  it("disables the re-transcribe button while the session is recording", () => {
    render(
      <SessionView
        path="/tmp/call.md"
        isActive
        initialData={{ ...baseData, audio_path: "/tmp/call.flac" }}
        liveLines={[]}
        interimMe=""
        interimThem=""
      />
    );

    expect(screen.getByRole("button", { name: /re-transcribe/i })).toBeDisabled();
  });

  it("clicking a transcript line seeks the audio element to the parsed offset", () => {
    render(
      <SessionView
        path="/tmp/call.md"
        isActive={false}
        initialData={{
          ...baseData,
          audio_path: "/tmp/call.flac",
          transcript: "[00:01:05] Me: hello there\n",
        }}
        liveLines={[]}
        interimMe=""
        interimThem=""
      />
    );

    const audio = document.querySelector("audio") as HTMLAudioElement;
    // jsdom's HTMLMediaElement has no real playback pipeline; `play()`
    // rejects with "not implemented" unless stubbed, and this test only
    // cares that a seek was requested, not that decoding actually started.
    audio.play = vi.fn().mockResolvedValue(undefined);

    fireEvent.click(screen.getByRole("button", { name: /hello there/i }));

    expect(audio.currentTime).toBe(65);
    expect(audio.play).toHaveBeenCalled();
  });

  it("calls retranscribe_session and shows progress phase text from retranscribe-progress events", async () => {
    vi.mocked(ipc.retranscribeSession).mockImplementationOnce(() => new Promise(() => {}));
    render(
      <SessionView
        path="/tmp/call.md"
        isActive={false}
        initialData={{ ...baseData, audio_path: "/tmp/call.flac" }}
        liveLines={[]}
        interimMe=""
        interimThem=""
      />
    );

    fireEvent.click(screen.getByRole("button", { name: /re-transcribe/i }));
    await waitFor(() => expect(ipc.retranscribeSession).toHaveBeenCalledWith("/tmp/call.md"));

    emit("retranscribe-progress", {
      path: "/tmp/call.md",
      phase: "transcribing",
      channel: "me",
      message: null,
    });

    expect(await screen.findByText(/transcribing \(me\)/i)).toBeInTheDocument();
  });

  it("ignores a retranscribe-progress event for a different session's path", async () => {
    vi.mocked(ipc.retranscribeSession).mockImplementationOnce(() => new Promise(() => {}));
    render(
      <SessionView
        path="/tmp/call.md"
        isActive={false}
        initialData={{ ...baseData, audio_path: "/tmp/call.flac" }}
        liveLines={[]}
        interimMe=""
        interimThem=""
      />
    );

    fireEvent.click(screen.getByRole("button", { name: /re-transcribe/i }));
    await waitFor(() => expect(ipc.retranscribeSession).toHaveBeenCalled());

    emit("retranscribe-progress", {
      path: "/tmp/other-call.md",
      phase: "writing",
      channel: null,
      message: null,
    });

    expect(screen.queryByText(/writing/i)).toBeNull();
  });

  it("shows the rejection message inline in a destructive-colored line on failure", async () => {
    vi.mocked(ipc.retranscribeSession).mockRejectedValueOnce(
      "no audio was saved for this call (it may have been recorded with audio retention turned off)"
    );
    render(
      <SessionView
        path="/tmp/call.md"
        isActive={false}
        initialData={{ ...baseData, audio_path: "/tmp/call.flac" }}
        liveLines={[]}
        interimMe=""
        interimThem=""
      />
    );

    fireEvent.click(screen.getByRole("button", { name: /re-transcribe/i }));

    const message = await screen.findByText(/no audio was saved for this call/i);
    expect(message.className).toContain("text-destructive");
    expect(ipc.readSession).not.toHaveBeenCalled();
  });

  it("re-reads the session and refreshes the transcript after a successful re-transcribe", async () => {
    vi.mocked(ipc.readSession).mockResolvedValueOnce({
      title: "Weekly sync",
      date: "",
      notes: "",
      transcript: "[00:00:03] Me: new whisper text\n",
      audio_path: "/tmp/call.flac",
    });
    render(
      <SessionView
        path="/tmp/call.md"
        isActive={false}
        initialData={{
          ...baseData,
          audio_path: "/tmp/call.flac",
          transcript: "[00:00:03] Me: old parakeet text\n",
        }}
        liveLines={[]}
        interimMe=""
        interimThem=""
      />
    );

    expect(screen.getByText(/old parakeet text/)).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /re-transcribe/i }));

    await screen.findByText(/new whisper text/);
    expect(screen.queryByText(/old parakeet text/)).toBeNull();
  });
});
