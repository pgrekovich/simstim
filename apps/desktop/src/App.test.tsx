import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "@/App";
import * as ipc from "@/lib/ipc";
import type { SessionMeta } from "@/lib/ipc";
import { listen } from "@tauri-apps/api/event";

/**
 * App drives three "real" Tauri surfaces directly: the `@/lib/ipc` wrappers
 * (also used by SessionView/SettingsDialog, so one mock covers all three
 * components), the raw `@tauri-apps/api/event` `listen` (App and
 * SettingsDialog both call it directly, not through ipc.ts), and
 * `@tauri-apps/plugin-dialog`'s `open` (SettingsDialog only, never invoked in
 * these tests since the folder picker is never clicked).
 */
vi.mock("@/lib/ipc", () => ({
  getConfig: vi.fn().mockResolvedValue({ output_dir: "/tmp", active_model: "m1", theme: "system" }),
  listSessions: vi.fn().mockResolvedValue([]),
  readSession: vi
    .fn()
    .mockResolvedValue({ title: "", date: "", notes: "", transcript: "", audio_path: null }),
  startRecording: vi.fn().mockResolvedValue("/tmp/active.md"),
  stopRecording: vi.fn().mockResolvedValue("/tmp/active.md"),
  saveSession: vi.fn().mockResolvedValue(undefined),
  updateActiveSession: vi.fn().mockResolvedValue(undefined),
  listModels: vi
    .fn()
    .mockResolvedValue([{ id: "m1", name: "Test model", size_mb: 100, downloaded: true, active: true }]),
  downloadModel: vi.fn().mockResolvedValue(undefined),
  setActiveModel: vi.fn().mockResolvedValue(undefined),
  setOutputDir: vi.fn().mockResolvedValue(undefined),
  setTheme: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
}));

/**
 * Invokes the handler App (or SettingsDialog) registered via `listen(name,
 * handler)`. Uses the LAST registration for `name`, not the first: some
 * listener effects re-subscribe when their dependencies change (e.g. App's
 * `retranscribe-progress` listener depends on `[selectedPath, activePath]`),
 * so `listen` can be called more than once for the same event name over a
 * test. The mock's own `unlisten` is a no-op, so nothing removes the stale
 * `mock.calls` entry for an old subscription -- grabbing the first match
 * would invoke a closure holding whatever state was current back when THAT
 * subscription was created, not the current one.
 */
function emit(name: string, payload: unknown) {
  const calls = vi.mocked(listen).mock.calls.filter(([eventName]) => eventName === name);
  const call = calls[calls.length - 1];
  if (!call) throw new Error(`no listener registered for "${name}" -- was it ever mounted?`);
  const handler = call[1] as (event: { payload: unknown }) => void;
  act(() => {
    handler({ payload });
  });
}

beforeEach(() => {
  vi.mocked(ipc.startRecording).mockClear();
  vi.mocked(ipc.stopRecording).mockClear();
  vi.mocked(ipc.listSessions).mockClear();
  vi.mocked(ipc.saveSession).mockClear();
  vi.mocked(ipc.updateActiveSession).mockClear();
  vi.mocked(listen).mockClear();
});

describe("App: hot-mic fix (final review Critical finding)", () => {
  it("keeps the record control wired to Stop when a session-error lands mid-recording", async () => {
    render(<App />);

    const recordButton = await screen.findByRole("button", { name: /^record$/i });
    fireEvent.click(recordButton);
    await waitFor(() => expect(ipc.startRecording).toHaveBeenCalledTimes(1));

    // Recording is in progress: the button already reads "Stop ...".
    await screen.findByRole("button", { name: /stop/i });

    // A worker-side error (e.g. a save failure) lands mid-recording. Before
    // the fix, deriving the button's mode from `status.kind` meant this
    // flipped the button back to "Record" -- clicking it would then run
    // handleStart against an already-running session instead of stopping it.
    emit("session-error", { message: "disk write failed" });

    const button = await screen.findByRole("button", { name: /stop/i });
    expect(button).toBeVisible();

    fireEvent.click(button);

    await waitFor(() => expect(ipc.stopRecording).toHaveBeenCalledTimes(1));
    expect(ipc.startRecording).toHaveBeenCalledTimes(1);
  });
});

describe("App: active-session notes survive a switch-away-and-back (final review Important finding)", () => {
  it("reseeds SessionView from the last typed notes, not a blank placeholder, when returning to the active session", async () => {
    const sessions: SessionMeta[] = [
      { path: "/tmp/active.md", title: "Untitled", date: new Date().toISOString(), duration_secs: null },
      { path: "/tmp/other.md", title: "Other call", date: new Date().toISOString(), duration_secs: null },
    ];
    vi.mocked(ipc.listSessions).mockResolvedValue(sessions);
    vi.mocked(ipc.readSession).mockImplementation(async (path: string) => {
      if (path === "/tmp/other.md") {
        return {
          title: "Other call",
          date: "",
          notes: "notes from the other call",
          transcript: "",
          audio_path: null,
        };
      }
      return { title: "Untitled", date: "", notes: "", transcript: "", audio_path: null };
    });

    render(<App />);

    const recordButton = await screen.findByRole("button", { name: /^record$/i });
    fireEvent.click(recordButton);
    await waitFor(() => expect(ipc.startRecording).toHaveBeenCalledTimes(1));
    await screen.findByRole("button", { name: /stop/i });

    // Type mid-call notes for the active session, well within the 500ms
    // debounce window (deliberately never blurred/flushed by this test).
    const notes = await screen.findByPlaceholderText("your notes...");
    fireEvent.change(notes, { target: { value: "call went great, follow up next week" } });

    // Switch away to a different (past) session: this unmounts the active
    // SessionView instance.
    const otherRow = await screen.findByRole("button", { name: /other call/i });
    fireEvent.click(otherRow);
    await screen.findByDisplayValue("notes from the other call");

    // Switch back to the still-recording active session: SessionView
    // remounts fresh. Before the fix, `initialData` was a fixed
    // "Untitled"/"" placeholder here, so the notes textarea would come back
    // empty -- and the very next keystroke would debounce-save that
    // near-empty state over the real notes.
    const activeRow = await screen.findByRole("button", { name: /^untitled/i });
    fireEvent.click(activeRow);

    await screen.findByDisplayValue("call went great, follow up next week");
  });
});

describe("App: record continues into the open note (behavior fix)", () => {
  it("passes null to start_recording when no session is selected", async () => {
    render(<App />);

    const recordButton = await screen.findByRole("button", { name: /^record$/i });
    fireEvent.click(recordButton);

    await waitFor(() => expect(ipc.startRecording).toHaveBeenCalledWith(null));
  });

  it("passes the selected session's path to start_recording, and seeds the view from its existing content", async () => {
    const sessions: SessionMeta[] = [
      { path: "/tmp/other.md", title: "Other call", date: new Date().toISOString(), duration_secs: null },
    ];
    vi.mocked(ipc.listSessions).mockResolvedValue(sessions);
    vi.mocked(ipc.readSession).mockResolvedValue({
      title: "Other call",
      date: "",
      notes: "old notes",
      transcript: "[00:00:01] Me: hi",
      audio_path: null,
    });
    vi.mocked(ipc.startRecording).mockResolvedValue("/tmp/other.md");

    render(<App />);

    const otherRow = await screen.findByRole("button", { name: /other call/i });
    fireEvent.click(otherRow);
    await screen.findByDisplayValue("old notes");

    const recordButton = await screen.findByRole("button", { name: /^record$/i });
    fireEvent.click(recordButton);

    await waitFor(() => expect(ipc.startRecording).toHaveBeenCalledWith("/tmp/other.md"));
    // Title/notes stay seeded from the file (not reset to "Untitled"/empty)
    // once the session flips to actively recording.
    await screen.findByDisplayValue("Other call");
    await screen.findByDisplayValue("old notes");
    // The existing transcript line must still be visible: recording into an
    // open note must not lose what was already there.
    await waitFor(() => expect(document.body.textContent).toContain("00:00:01"));
    await waitFor(() => expect(document.body.textContent).toContain("hi"));
  });

  it("flushes a pending, still-debouncing notes edit via save_session before start_recording is called (lost-edit race fix)", async () => {
    // Reproduces the bug: with a session selected (inactive), typing a note
    // and hitting Record right away -- well within the 500ms debounce --
    // used to lose the edit outright. save_session was still the correct
    // route (the session is not active yet), but start_recording flips
    // AppState's active path synchronously, so a debounce firing even
    // slightly late would get rejected by save_session's single-writer
    // guard with nothing left to retry. handleStart must flush the pending
    // edit itself, before ever calling start_recording.
    const sessions: SessionMeta[] = [
      { path: "/tmp/other.md", title: "Other call", date: new Date().toISOString(), duration_secs: null },
    ];
    vi.mocked(ipc.listSessions).mockResolvedValue(sessions);
    vi.mocked(ipc.readSession).mockResolvedValue({
      title: "Other call",
      date: "",
      notes: "old notes",
      transcript: "",
      audio_path: null,
    });
    vi.mocked(ipc.startRecording).mockResolvedValue("/tmp/other.md");

    render(<App />);

    const otherRow = await screen.findByRole("button", { name: /other call/i });
    fireEvent.click(otherRow);
    const notes = await screen.findByDisplayValue("old notes");

    // Deliberately no blur and no timer advance: the edit is still only
    // sitting in the debounce, exactly as it would be right after typing.
    fireEvent.change(notes, { target: { value: "quick note before recording" } });
    expect(ipc.saveSession).not.toHaveBeenCalled();

    const recordButton = await screen.findByRole("button", { name: /^record$/i });
    fireEvent.click(recordButton);

    await waitFor(() => expect(ipc.startRecording).toHaveBeenCalledTimes(1));
    expect(ipc.saveSession).toHaveBeenCalledWith(
      "/tmp/other.md",
      "Other call",
      "quick note before recording"
    );

    // Not just "called at some point" -- the flush must complete BEFORE
    // start_recording is invoked, not merely before it resolves.
    const saveOrder = vi.mocked(ipc.saveSession).mock.invocationCallOrder[0];
    const startOrder = vi.mocked(ipc.startRecording).mock.invocationCallOrder[0];
    expect(saveOrder).toBeLessThan(startOrder);
  });
});

describe("App: refreshes a re-transcribed session after a background done event (fix-review finding, wave 2)", () => {
  it("remounts the currently selected SessionView with the fresh transcript once retranscribe-progress reports done", async () => {
    // Reproduces the bug: a SessionView only re-seeds its local transcript
    // state on mount. If a re-transcribe job finishes while the user is
    // looking at a DIFFERENT SessionView instance for the same path (e.g.
    // switched away mid-job and back, remounting it from a stale on-disk
    // read taken before the job finished), nothing ever refreshed it. App's
    // retranscribe-progress listener must itself refetch and force a remount
    // for the currently selected (inactive) session on the "done" phase,
    // regardless of which instance actually clicked "Re-transcribe".
    const sessions: SessionMeta[] = [
      { path: "/tmp/other.md", title: "Other call", date: new Date().toISOString(), duration_secs: null },
    ];
    vi.mocked(ipc.listSessions).mockResolvedValue(sessions);
    vi.mocked(ipc.readSession).mockResolvedValueOnce({
      title: "Other call",
      date: "",
      notes: "old notes",
      transcript: "[00:00:01] Me: original parakeet text",
      audio_path: null,
    });

    render(<App />);

    const otherRow = await screen.findByRole("button", { name: /other call/i });
    fireEvent.click(otherRow);
    await waitFor(() => expect(document.body.textContent).toContain("original parakeet text"));

    // The refetch App itself performs on "done" returns the rewritten
    // transcript -- simulating the re-transcribe pipeline's actual effect on
    // disk (title/notes/audio_path unchanged, only the transcript differs).
    vi.mocked(ipc.readSession).mockResolvedValueOnce({
      title: "Other call",
      date: "",
      notes: "old notes",
      transcript: "[00:00:01] Me: rewritten by whisper",
      audio_path: null,
    });

    emit("retranscribe-progress", {
      path: "/tmp/other.md",
      phase: "done",
      channel: null,
      message: null,
    });

    await waitFor(() => expect(document.body.textContent).toContain("rewritten by whisper"));
    expect(document.body.textContent).not.toContain("original parakeet text");
  });

  it("does not refetch when the done event is for a session other than the one currently selected", async () => {
    const sessions: SessionMeta[] = [
      { path: "/tmp/other.md", title: "Other call", date: new Date().toISOString(), duration_secs: null },
    ];
    vi.mocked(ipc.listSessions).mockResolvedValue(sessions);
    vi.mocked(ipc.readSession).mockResolvedValue({
      title: "Other call",
      date: "",
      notes: "old notes",
      transcript: "[00:00:01] Me: hi",
      audio_path: null,
    });

    render(<App />);

    const otherRow = await screen.findByRole("button", { name: /other call/i });
    fireEvent.click(otherRow);
    await waitFor(() => expect(document.body.textContent).toContain("hi"));

    const callsBeforeEvent = vi.mocked(ipc.readSession).mock.calls.length;

    emit("retranscribe-progress", {
      path: "/tmp/some-unrelated-session.md",
      phase: "done",
      channel: null,
      message: null,
    });

    // No extra call should ever fire for an unrelated path -- give any
    // errant async work a tick to have shown up before asserting its absence.
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(vi.mocked(ipc.readSession).mock.calls.length).toBe(callsBeforeEvent);
  });
});

describe("App: surfaces terminal re-transcribe errors from background jobs (stage 0.9 Task 2 review finding)", () => {
  it("shows the error message in the header for a session other than the one currently viewed", async () => {
    // Reproduces the bug: an auto-triggered re-transcribe job is
    // fire-and-forget (see retranscribe.rs's module docs) and has no other
    // UI of its own, so its failure (a stale-job drop, a delete-after
    // failure, a missing model) used to be completely invisible -- the
    // listener returned early on "error" without ever reading
    // `payload.message`.
    const sessions: SessionMeta[] = [
      { path: "/tmp/viewed.md", title: "Viewed call", date: new Date().toISOString(), duration_secs: null },
      { path: "/tmp/other.md", title: "Other call", date: new Date().toISOString(), duration_secs: null },
    ];
    vi.mocked(ipc.listSessions).mockResolvedValue(sessions);
    vi.mocked(ipc.readSession).mockResolvedValue({
      title: "Viewed call",
      date: "",
      notes: "",
      transcript: "[00:00:01] Me: hi",
      audio_path: null,
    });

    render(<App />);

    const viewedRow = await screen.findByRole("button", { name: /viewed call/i });
    fireEvent.click(viewedRow);
    await waitFor(() => expect(document.body.textContent).toContain("hi"));

    // The failing job belongs to "/tmp/other.md", not the session currently
    // selected ("/tmp/viewed.md").
    emit("retranscribe-progress", {
      path: "/tmp/other.md",
      phase: "error",
      channel: null,
      message: "whisper model not downloaded",
    });

    await screen.findByText(/whisper model not downloaded/i);
  });

  it("does not surface the error for the session currently being viewed (avoids double-reporting a manual click's own inline error)", async () => {
    const sessions: SessionMeta[] = [
      { path: "/tmp/viewed.md", title: "Viewed call", date: new Date().toISOString(), duration_secs: null },
    ];
    vi.mocked(ipc.listSessions).mockResolvedValue(sessions);
    vi.mocked(ipc.readSession).mockResolvedValue({
      title: "Viewed call",
      date: "",
      notes: "",
      transcript: "[00:00:01] Me: hi",
      audio_path: null,
    });

    render(<App />);

    const viewedRow = await screen.findByRole("button", { name: /viewed call/i });
    fireEvent.click(viewedRow);
    await waitFor(() => expect(document.body.textContent).toContain("hi"));

    emit("retranscribe-progress", {
      path: "/tmp/viewed.md",
      phase: "error",
      channel: null,
      message: "whisper model not downloaded",
    });

    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(screen.queryByText(/whisper model not downloaded/i)).not.toBeInTheDocument();
  });
});

describe("App: sidebar refresh on model-ready (final review Important finding)", () => {
  it("refreshes the session list exactly once when the model finishes loading, not on every later line", async () => {
    render(<App />);

    const recordButton = await screen.findByRole("button", { name: /^record$/i });
    fireEvent.click(recordButton);
    await waitFor(() => expect(ipc.startRecording).toHaveBeenCalledTimes(1));

    const callsBeforeModelReady = vi.mocked(ipc.listSessions).mock.calls.length;

    // The first transcript line marks the model finishing its load
    // (loading-model -> recording): this is when the session file first
    // reliably exists on disk, so the sidebar must refresh here.
    emit("transcript-line", "[00:00:01] Me: hi");
    await waitFor(() =>
      expect(vi.mocked(ipc.listSessions).mock.calls.length).toBe(callsBeforeModelReady + 1)
    );

    // Further lines within the same recording must not trigger another
    // refresh (no storm): the transition only happens once.
    emit("transcript-line", "[00:00:04] Them: hey");
    emit("transcript-line", "[00:00:07] Me: sounds good");
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(vi.mocked(ipc.listSessions).mock.calls.length).toBe(callsBeforeModelReady + 1);
  });
});
