import { createRef } from "react";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  formatSessionMeta,
  SessionView,
  shouldStackTranscriptFind,
  type SessionViewHandle,
} from "@/components/SessionView";
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
    summary: "",
    summary_model: null,
    people: [],
  }),
  retranscribeSession: vi.fn().mockResolvedValue(undefined),
  enhanceSession: vi.fn().mockResolvedValue(""),
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
  summary: "",
  summary_model: null,
  people: [],
};

beforeEach(() => {
  vi.mocked(ipc.saveSession).mockClear();
  vi.mocked(ipc.updateActiveSession).mockClear();
  vi.mocked(ipc.readSession).mockClear();
  vi.mocked(ipc.retranscribeSession).mockClear();
  vi.mocked(ipc.enhanceSession).mockClear().mockResolvedValue("");
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

describe("SessionView: multiline title field", () => {
  it("wraps in a textarea but collapses pasted newlines into single spaces", () => {
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

    // The title stays one logical line (it is the file's H1): the textarea
    // exists only so long titles wrap visually instead of clipping.
    const title = screen.getByDisplayValue("Old title") as HTMLTextAreaElement;
    expect(title.tagName).toBe("TEXTAREA");

    fireEvent.change(title, { target: { value: "CDK migration status \nand S3 handler risks" } });
    expect(title.value).toBe("CDK migration status and S3 handler risks");

    fireEvent.blur(title);
    expect(ipc.saveSession).toHaveBeenCalledWith(
      "/tmp/call.md",
      "CDK migration status and S3 handler risks",
      "",
    );
  });

  it("blurs on Enter instead of inserting a newline", () => {
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

    const title = screen.getByDisplayValue("Old title") as HTMLTextAreaElement;
    title.focus();
    fireEvent.keyDown(title, { key: "Enter" });
    expect(document.activeElement).not.toBe(title);
    expect(title.value).toBe("Old title");
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
    expect(screen.queryByRole("button", { name: /improve transcript/i })).toBeNull();
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

    // The custom AudioPlayer bar (stage 0.10) wraps a hidden <audio>
    // element: no native `controls` chrome, playback drives through the
    // bar's own play button and seek slider.
    const audio = document.querySelector("audio");
    expect(audio).not.toBeNull();
    expect(audio).not.toHaveAttribute("controls");
    expect(audio).toHaveAttribute("preload", "metadata");
    expect(audio?.getAttribute("src")).toBe("asset://localhost//tmp/2026-08-15 call.flac");
    expect(screen.getByRole("button", { name: /^play$/i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /playback speed/i })).toHaveTextContent("1x");
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

    expect(screen.getByRole("button", { name: /improve transcript/i })).toBeDisabled();
  });

  it("clicking a transcript line seeks the audio element to the parsed offset", () => {
    render(
      <SessionView
        path="/tmp/call.md"
        isActive={false}
        initialData={{
          ...baseData,
          audio_path: "/tmp/call.flac",
          summary: "",
          summary_model: null,
          people: [],
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

    fireEvent.click(screen.getByRole("button", { name: /improve transcript/i }));

    await waitFor(() => expect(ipc.retranscribeSession).toHaveBeenCalledWith("/tmp/call.md"));
    expect(screen.getByText(/starting/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /improving/i })).toBeDisabled();
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
    expect(screen.getByRole("button", { name: /improving/i })).toBeDisabled();
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
    expect(screen.getByRole("button", { name: /^improve transcript$/i })).not.toBeDisabled();
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

    expect(screen.getByRole("button", { name: /improving/i })).toBeDisabled();
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

    fireEvent.click(screen.getByRole("button", { name: /improve transcript/i }));

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
      summary: "",
      summary_model: null,
      people: [],
    });
    render(
      <SessionView
        path="/tmp/call.md"
        isActive={false}
        initialData={{
          ...baseData,
          audio_path: "/tmp/call.flac",
          summary: "",
          summary_model: null,
          people: [],
          transcript: "[00:00:03] Me: old parakeet text\n",
        }}
        liveLines={[]}
        interimMe=""
        interimThem=""
        retranscribingJob={null}
      />
    );

    expect(screen.getByText(/old parakeet text/)).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /improve transcript/i }));

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
      summary: "",
      summary_model: null,
      people: [],
    });
    render(
      <SessionView
        path="/tmp/call.md"
        isActive={false}
        initialData={{
          ...baseData,
          audio_path: "/tmp/call.flac",
          summary: "",
          summary_model: null,
          people: [],
          transcript: "[00:00:03] Me: old parakeet text\n",
        }}
        liveLines={[]}
        interimMe=""
        interimThem=""
        retranscribingJob={null}
      />
    );

    expect(screen.getByText(/old parakeet text/)).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /improve transcript/i }));

    await screen.findByText(/new whisper text/);
    expect(screen.queryByText(/old parakeet text/)).toBeNull();
  });
});

describe("SessionView: notes/transcript tabs (stage 0.10)", () => {
  it("renders Markdown in both Notes and Summary", async () => {
    render(
      <SessionView
        path="/tmp/call.md"
        isActive={false}
        initialData={{
          ...baseData,
          notes: "## Decisions\n\n- Ship **Friday**",
          summary: "> Generated **brief**",
          summary_model: "local/qwen",
          people: [],
        }}
        liveLines={[]}
        interimMe=""
        interimThem=""
        retranscribingJob={null}
      />,
    );

    expect(await screen.findByRole("heading", { level: 2, name: "Decisions" })).toBeInTheDocument();
    expect(screen.getByText("Friday").tagName).toBe("STRONG");
    expect(screen.queryByText("## Decisions")).toBeNull();

    fireEvent.mouseDown(screen.getByRole("tab", { name: "Summary" }));
    expect((await screen.findByText("brief")).tagName).toBe("STRONG");
    expect(screen.getByText(/Generated/).closest("blockquote")).not.toBeNull();
  });

  it("edits rendered Notes through the existing debounced save path", async () => {
    render(
      <SessionView
        path="/tmp/call.md"
        isActive={false}
        initialData={{ ...baseData, notes: "**Old** note" }}
        liveLines={[]}
        interimMe=""
        interimThem=""
        retranscribingJob={null}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Edit notes" }));
    const textarea = screen.getByPlaceholderText("your notes...");
    await waitFor(() => expect(textarea).toHaveFocus());
    fireEvent.change(textarea, { target: { value: "## New note" } });
    fireEvent.blur(textarea);

    await waitFor(() =>
      expect(ipc.saveSession).toHaveBeenCalledWith(
        "/tmp/call.md",
        "Weekly sync",
        "## New note",
      ),
    );
    expect(await screen.findByRole("heading", { level: 2, name: "New note" })).toBeInTheDocument();
  });

  it("places the switcher in the application toolbar when its portal slot exists", async () => {
    const toolbarSlot = document.createElement("div");
    toolbarSlot.id = "topbar-tabs-slot";
    document.body.appendChild(toolbarSlot);

    let unmount = () => {};
    try {
      ({ unmount } = render(
        <SessionView
          path="/tmp/call.md"
          isActive={false}
          initialData={baseData}
          liveLines={[]}
          interimMe=""
          interimThem=""
          retranscribingJob={null}
        />
      ));

      const notesTab = screen.getByRole("tab", { name: "Notes" });
      await waitFor(() => expect(toolbarSlot).toContainElement(notesTab));
      expect(toolbarSlot).not.toContainElement(screen.getByPlaceholderText("Untitled"));
    } finally {
      unmount();
      toolbarSlot.remove();
    }
  });

  it("defaults to the Notes tab with the transcript content hidden, and switches on click", () => {
    render(
      <SessionView
        path="/tmp/call.md"
        isActive={false}
        initialData={{ ...baseData, transcript: "[00:00:03] Me: hello tabs\n" }}
        liveLines={[]}
        interimMe=""
        interimThem=""
        retranscribingJob={null}
      />
    );

    const notesTab = screen.getByRole("tab", { name: "Notes" });
    const transcriptTab = screen.getByRole("tab", { name: "Transcript" });
    expect(notesTab).toHaveAttribute("data-state", "active");

    // Force-mounted but CSS-hidden while inactive (keeps audio playing
    // across switches): assert the state attribute the hiding hangs on.
    const transcriptPanel = screen.getByText("hello tabs").closest('[data-slot="tabs-content"]');
    expect(transcriptPanel).toHaveAttribute("data-state", "inactive");

    // Radix tab triggers activate on mousedown, not click.
    fireEvent.mouseDown(transcriptTab);
    expect(transcriptTab).toHaveAttribute("data-state", "active");
    expect(transcriptPanel).toHaveAttribute("data-state", "active");
    const notesPanel = screen
      .getByPlaceholderText("your notes...")
      .closest('[data-slot="tabs-content"]');
    expect(notesPanel).toHaveAttribute("data-state", "inactive");
  });

  it("keeps the session title outside the tab panels so Summary and Transcript both show it", () => {
    render(
      <SessionView
        path="/tmp/call.md"
        isActive={false}
        initialData={{
          ...baseData,
          notes: "keep Notes primary",
          summary: "A concise summary",
          summary_model: "local/qwen",
          people: [],
          transcript: "[00:00:03] Me: hello tabs\n",
        }}
        liveLines={[]}
        interimMe=""
        interimThem=""
        retranscribingJob={null}
      />
    );

    const title = screen.getByDisplayValue("Weekly sync");
    expect(title.closest('[data-slot="tabs-content"]')).toBeNull();
    expect(screen.getByRole("tab", { name: "Notes" })).toHaveAttribute("data-state", "active");

    fireEvent.mouseDown(screen.getByRole("tab", { name: "Summary" }));
    expect(screen.getByDisplayValue("Weekly sync")).toBe(title);

    fireEvent.mouseDown(screen.getByRole("tab", { name: "Transcript" }));
    expect(screen.getByDisplayValue("Weekly sync")).toBe(title);
  });

  it("opens Summary by default for a finished session whose Notes are empty", () => {
    render(
      <SessionView
        path="/tmp/call.md"
        isActive={false}
        initialData={{
          ...baseData,
          notes: "   ",
          summary: "A concise summary",
          summary_model: "local/qwen",
          people: [],
        }}
        liveLines={[]}
        interimMe=""
        interimThem=""
        retranscribingJob={null}
      />
    );

    expect(screen.getByRole("tab", { name: "Summary" })).toHaveAttribute("data-state", "active");
    expect(screen.getByRole("tab", { name: "Notes" })).toHaveAttribute("data-state", "inactive");
  });
});

describe("SessionView: transcript find (stage 2.1)", () => {
  const transcript = [
    "[00:00:03] Me: Berlin first",
    "[00:00:09] Them: then Berlin again",
  ].join("\n");

  it("stacks only when the measured tab-to-find gap would be under 16 px", () => {
    expect(shouldStackTranscriptFind(500, 728, 200)).toBe(false);
    expect(shouldStackTranscriptFind(501, 728, 200)).toBe(true);
  });

  it("opens Transcript, targets the requested timestamp, and wraps navigation", async () => {
    render(
      <SessionView
        path="/tmp/call.md"
        isActive={false}
        initialData={{ ...baseData, transcript }}
        liveLines={[]}
        interimMe=""
        interimThem=""
        retranscribingJob={null}
        transcriptFindRequest={{ id: 1, query: "berlin", timestamp: "00:00:09" }}
      />
    );

    const input = await screen.findByRole("searchbox", { name: "Find in transcript" });
    expect(input).toHaveValue("berlin");
    expect(screen.getByRole("tab", { name: "Transcript" })).toHaveAttribute("data-state", "active");
    expect(screen.getByText("2 / 2")).toBeInTheDocument();
    expect(screen.getAllByText(/berlin/i)[1]).toHaveAttribute("data-current-match", "true");

    fireEvent.click(screen.getByRole("button", { name: "Next transcript match" }));
    expect(screen.getByText("1 / 2")).toBeInTheDocument();
    expect(screen.getAllByText(/berlin/i)[0]).toHaveAttribute("data-current-match", "true");

    fireEvent.keyDown(input, { key: "Enter", shiftKey: true });
    expect(screen.getByText("2 / 2")).toBeInTheDocument();
  });

  it("shows an empty count and removes highlights when closed", async () => {
    render(
      <SessionView
        path="/tmp/call.md"
        isActive={false}
        initialData={{ ...baseData, transcript }}
        liveLines={[]}
        interimMe=""
        interimThem=""
        retranscribingJob={null}
        transcriptFindRequest={{ id: 1, query: "missing" }}
      />
    );

    expect(await screen.findByText("0 / 0")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Next transcript match" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Close transcript find" }));
    expect(screen.queryByRole("searchbox", { name: "Find in transcript" })).toBeNull();
    expect(document.querySelectorAll("mark")).toHaveLength(0);
  });
});

describe("SessionView: enhance (stage 0.11)", () => {
  it("runs enhance from the toolbar pill and spins the Summary tab while the click is in flight", async () => {
    let resolveEnhance: (value: string) => void = () => {};
    vi.mocked(ipc.enhanceSession).mockImplementation(
      () => new Promise<string>((resolve) => { resolveEnhance = resolve; })
    );

    render(
      <SessionView
        path="/tmp/call.md"
        isActive={false}
        initialData={{ ...baseData, notes: "rough", transcript: "[00:00:01] Me: hi" }}
        liveLines={[]}
        interimMe=""
        interimThem=""
        retranscribingJob={null}
      />
    );

    fireEvent.click(screen.getByRole("button", { name: /^summarize$/i }));
    await waitFor(() => expect(ipc.enhanceSession).toHaveBeenCalledExactlyOnceWith("/tmp/call.md"));

    // While the command is in flight the pill disappears and a disabled
    // Summary tab appears with a spinner (stage 4.1 polish: the old work
    // chip clipped against the toolbar) -- the refreshed summary itself
    // arrives later via App (enhance-progress "done" refetches and remounts).
    const summaryTab = screen.getByRole("tab", { name: /summary/i });
    expect(summaryTab).toBeDisabled();
    expect(screen.getByTestId("summary-tab-spinner")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^summarize$/i })).toBeNull();

    resolveEnhance("- **ship v2 friday**");
    await waitFor(() => expect(screen.queryByTestId("summary-tab-spinner")).toBeNull());
  });

  it("spins the Summary tab whenever App reports a running enhance for this path", () => {
    render(
      <SessionView
        path="/tmp/call.md"
        isActive={false}
        initialData={{ ...baseData, transcript: "[00:00:01] Me: hi" }}
        liveLines={[]}
        interimMe=""
        interimThem=""
        retranscribingJob={null}
        enhanceRunning
      />
    );
    expect(screen.getByTestId("summary-tab-spinner")).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: /summary/i })).toBeDisabled();
    expect(screen.queryByRole("button", { name: /^summarize$/i })).toBeNull();
  });

  it("offers no Enhance control while recording or without a transcript", () => {
    const { unmount } = render(
      <SessionView
        path="/tmp/call.md"
        isActive
        initialData={{ ...baseData, transcript: "[00:00:01] Me: hi" }}
        liveLines={["[00:00:01] Me: hi"]}
        interimMe=""
        interimThem=""
        retranscribingJob={null}
      />
    );
    expect(screen.queryByRole("button", { name: /^summarize$/i })).toBeNull();
    unmount();

    render(
      <SessionView
        path="/tmp/call.md"
        isActive={false}
        initialData={{ ...baseData, transcript: "" }}
        liveLines={[]}
        interimMe=""
        interimThem=""
        retranscribingJob={null}
      />
    );
    expect(screen.queryByRole("button", { name: /^summarize$/i })).toBeNull();
  });

  it("retires the pill once a summary exists and offers re-run in the Summary tab's provenance line", async () => {
    render(
      <SessionView
        path="/tmp/call.md"
        isActive={false}
        initialData={{
          ...baseData,
          transcript: "[00:00:01] Me: hi",
          summary: "- **ship v2 friday**",
          summary_model: "claude",
          people: [],
        }}
        liveLines={[]}
        interimMe=""
        interimThem=""
        retranscribingJob={null}
      />
    );

    expect(screen.queryByRole("button", { name: /^summarize$/i })).toBeNull();

    fireEvent.click(screen.getByRole("tab", { name: "Summary" }));
    expect(screen.getByText(/by claude/i)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /again/i }));
    await waitFor(() => expect(ipc.enhanceSession).toHaveBeenCalledExactlyOnceWith("/tmp/call.md"));
  });
});

describe("SessionView: title meta line (date and duration)", () => {
  // TZ-free date strings (no offset) parse as local time, so the expected
  // clock reading is stable across machines.
  it("formats same-year notes without the year and appends the duration", () => {
    expect(
      formatSessionMeta("2026-09-02T14:03:52", 611, new Date("2026-09-04T00:00:00")),
    ).toBe("Sep 2, 14:03 · 10:11");
  });

  it("spells out the year for a note from another year and omits a missing duration", () => {
    expect(
      formatSessionMeta("2025-08-28T09:05:00", null, new Date("2026-09-04T00:00:00")),
    ).toBe("Aug 28, 2025, 09:05");
  });

  it("returns null for the active session's blank placeholder date", () => {
    expect(formatSessionMeta("", 100, new Date())).toBeNull();
    expect(formatSessionMeta("not a date", 100, new Date())).toBeNull();
  });

  it("renders the meta line under an opened past note's title", () => {
    render(
      <SessionView
        path="/tmp/call.md"
        isActive={false}
        initialData={{ ...baseData, date: "2026-09-02T14:03:52" }}
        durationSecs={611}
        liveLines={[]}
        interimMe=""
        interimThem=""
        retranscribingJob={null}
      />
    );

    expect(
      screen.getByText(formatSessionMeta("2026-09-02T14:03:52", 611)!),
    ).toBeVisible();
  });

  it("lists who was on the call on its own line under the date", () => {
    render(
      <SessionView
        path="/tmp/call.md"
        isActive={false}
        initialData={{
          ...baseData,
          date: "2026-09-02T14:03:52",
          people: ["Anna Berg", "Mark"],
        }}
        durationSecs={611}
        liveLines={[]}
        interimMe=""
        interimThem=""
        retranscribingJob={null}
      />
    );

    expect(screen.getByText(formatSessionMeta("2026-09-02T14:03:52", 611)!)).toBeVisible();
    expect(screen.getByText("with Anna Berg, Mark")).toBeVisible();
  });

  it("shows no meta line while the session is being recorded", () => {
    render(
      <SessionView
        path="/tmp/call.md"
        isActive
        initialData={{ ...baseData, date: "2026-09-02T14:03:52" }}
        durationSecs={611}
        liveLines={[]}
        interimMe=""
        interimThem=""
        retranscribingJob={null}
      />
    );

    expect(
      screen.queryByText(formatSessionMeta("2026-09-02T14:03:52", 611)!),
    ).not.toBeInTheDocument();
  });
});

describe("SessionView: copy summary", () => {
  it("puts the summary Markdown on the clipboard and flashes 'copied'", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    render(
      <SessionView
        path="/tmp/call.md"
        isActive={false}
        initialData={{
          ...baseData,
          transcript: "[00:00:01] Me: hi",
          summary: "### Decisions\n- ship v2 friday",
          summary_model: "claude",
        }}
        liveLines={[]}
        interimMe=""
        interimThem=""
        retranscribingJob={null}
      />
    );

    fireEvent.click(screen.getByRole("tab", { name: "Summary" }));
    fireEvent.click(screen.getByRole("button", { name: /copy summary/i }));

    expect(writeText).toHaveBeenCalledExactlyOnceWith("### Decisions\n- ship v2 friday");
    await waitFor(() => expect(screen.getByRole("button", { name: /copy summary/i })).toHaveTextContent("Copied"));
  });
});

describe("SessionView: copy transcript", () => {
  it("puts every timestamped line on the clipboard from the player bar", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    render(
      <SessionView
        path="/tmp/call.md"
        isActive={false}
        initialData={{
          ...baseData,
          audio_path: "/tmp/call.m4a",
          transcript: "[00:00:01] Me: hi\n[00:00:04] Them: hello",
        }}
        liveLines={[]}
        interimMe=""
        interimThem=""
        retranscribingJob={null}
      />
    );

    fireEvent.click(screen.getByRole("tab", { name: "Transcript" }));
    fireEvent.click(screen.getByRole("button", { name: /copy transcript/i }));
    expect(writeText).toHaveBeenCalledExactlyOnceWith("[00:00:01] Me: hi\n[00:00:04] Them: hello");
    await waitFor(() => expect(screen.getByRole("button", { name: /copy transcript/i })).toHaveTextContent("Copied"));
  });
});
