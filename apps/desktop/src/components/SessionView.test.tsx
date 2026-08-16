import { createRef } from "react";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
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

beforeEach(() => {
  vi.mocked(ipc.saveSession).mockClear();
  vi.mocked(ipc.updateActiveSession).mockClear();
  vi.mocked(ipc.readSession).mockClear();
  vi.mocked(ipc.retranscribeSession).mockClear();
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
        retranscribingJob={null}
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
        retranscribingJob={null}
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
        retranscribingJob={null}
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
        retranscribingJob={null}
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
        retranscribingJob={null}
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
        retranscribingJob={null}
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
        retranscribingJob={null}
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

  it("calls retranscribe_session and shows starting text as soon as it's clicked", async () => {
    vi.mocked(ipc.retranscribeSession).mockImplementationOnce(() => new Promise(() => {}));
    render(
      <SessionView
        path="/tmp/call.md"
        isActive={false}
        initialData={{ ...baseData, audio_path: "/tmp/call.flac" }}
        liveLines={[]}
        interimMe=""
        interimThem=""
        retranscribingJob={null}
      />
    );

    fireEvent.click(screen.getByRole("button", { name: /re-transcribe/i }));

    await waitFor(() => expect(ipc.retranscribeSession).toHaveBeenCalledWith("/tmp/call.md"));
    expect(screen.getByText(/starting/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /re-transcribing/i })).toBeDisabled();
  });

  it("renders the lifted job's phase text (from App) when it names this session's path", () => {
    render(
      <SessionView
        path="/tmp/call.md"
        isActive={false}
        initialData={{ ...baseData, audio_path: "/tmp/call.flac" }}
        liveLines={[]}
        interimMe=""
        interimThem=""
        retranscribingJob={{ path: "/tmp/call.md", text: "transcribing (me)..." }}
      />
    );

    expect(screen.getByText(/transcribing \(me\)/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /re-transcribing/i })).toBeDisabled();
  });

  it("ignores the lifted job when it names a different session's path", () => {
    render(
      <SessionView
        path="/tmp/call.md"
        isActive={false}
        initialData={{ ...baseData, audio_path: "/tmp/call.flac" }}
        liveLines={[]}
        interimMe=""
        interimThem=""
        retranscribingJob={{ path: "/tmp/other-call.md", text: "writing..." }}
      />
    );

    expect(screen.queryByText(/writing/i)).toBeNull();
    expect(screen.getByRole("button", { name: /^re-transcribe$/i })).not.toBeDisabled();
  });

  it("security-review fix: stays disabled across a remount while the job is still running for this path", () => {
    // Reproduces the bug: a local `retranscribing` boolean reset to `false`
    // on every mount, so switching away from this session mid-job (which
    // unmounts this instance under React's `key={path}` remount, see the
    // class doc comment) and back silently re-enabled the button even
    // though the backend job (global, one at a time) was still running.
    // `retranscribingJob` is owned by App instead, so a FRESH instance for
    // the same path must come up already disabled if App still says a job
    // is running for it -- no click, no prior render, in this test.
    const initialData = { ...baseData, audio_path: "/tmp/call.flac" };
    const { unmount } = render(
      <SessionView
        path="/tmp/call.md"
        isActive={false}
        initialData={initialData}
        liveLines={[]}
        interimMe=""
        interimThem=""
        retranscribingJob={{ path: "/tmp/call.md", text: "transcribing (them)..." }}
      />
    );
    unmount();

    render(
      <SessionView
        path="/tmp/call.md"
        isActive={false}
        initialData={initialData}
        liveLines={[]}
        interimMe=""
        interimThem=""
        retranscribingJob={{ path: "/tmp/call.md", text: "transcribing (them)..." }}
      />
    );

    expect(screen.getByRole("button", { name: /re-transcribing/i })).toBeDisabled();
    expect(screen.getByText(/transcribing \(them\)/i)).toBeInTheDocument();
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
        retranscribingJob={null}
      />
    );

    fireEvent.click(screen.getByRole("button", { name: /re-transcribe/i }));

    const message = await screen.findByText(/no audio was saved for this call/i);
    expect(message.className).toContain("text-destructive");
    expect(ipc.readSession).not.toHaveBeenCalled();
  });

  it("refetches the transcript even on the 'transcript updated, but the audio could not be deleted' rejection (minor fix-review finding)", async () => {
    // That rejection is the one case where the promise rejects even though
    // the transcript itself WAS rewritten on disk -- only the follow-up
    // audio-deletion step failed. Unlike an ordinary failure (the test
    // above), the on-screen transcript must still refresh here.
    vi.mocked(ipc.retranscribeSession).mockRejectedValueOnce(
      new Error("transcript updated, but the audio could not be deleted: permission denied")
    );
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
        retranscribingJob={null}
      />
    );

    expect(screen.getByText(/old parakeet text/)).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /re-transcribe/i }));

    await screen.findByText(/could not be deleted/i);
    await screen.findByText(/new whisper text/);
    expect(screen.queryByText(/old parakeet text/)).toBeNull();
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
        retranscribingJob={null}
      />
    );

    expect(screen.getByText(/old parakeet text/)).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /re-transcribe/i }));

    await screen.findByText(/new whisper text/);
    expect(screen.queryByText(/old parakeet text/)).toBeNull();
  });
});
