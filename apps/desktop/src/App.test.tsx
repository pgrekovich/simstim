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
  retranscribeSession: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
}));

// SessionView feeds a session's `<audio src>` through `convertFileSrc` when
// `audio_path` is set (needed below to drive a real manual Re-transcribe
// click through the UI): a real Tauri asset URL scheme is irrelevant here, a
// stable, inspectable stand-in is all the test needs.
vi.mock("@tauri-apps/api/core", () => ({
  convertFileSrc: vi.fn((path: string) => `asset://localhost/${path}`),
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
  vi.mocked(ipc.retranscribeSession).mockClear().mockResolvedValue(undefined);
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

  it("also refetches on the 'transcript updated, but the audio could not be deleted' error (minor fix-review finding)", async () => {
    // That specific "error" phase is the one case where the transcript on
    // disk DID change even though the phase isn't "done" -- the whisper pass
    // succeeded and wrote the new transcript; only the follow-up audio
    // deletion failed. Without this, the on-screen transcript stays stale
    // exactly like the "done" case above would without ITS refetch.
    const sessions: SessionMeta[] = [
      { path: "/tmp/other.md", title: "Other call", date: new Date().toISOString(), duration_secs: null },
    ];
    vi.mocked(ipc.listSessions).mockResolvedValue(sessions);
    vi.mocked(ipc.readSession).mockResolvedValueOnce({
      title: "Other call",
      date: "",
      notes: "old notes",
      transcript: "[00:00:01] Me: original parakeet text",
      audio_path: "/tmp/other.flac",
    });

    render(<App />);

    const otherRow = await screen.findByRole("button", { name: /other call/i });
    fireEvent.click(otherRow);
    await waitFor(() => expect(document.body.textContent).toContain("original parakeet text"));

    vi.mocked(ipc.readSession).mockResolvedValueOnce({
      title: "Other call",
      date: "",
      notes: "old notes",
      transcript: "[00:00:01] Me: rewritten by whisper",
      audio_path: "/tmp/other.flac",
    });

    emit("retranscribe-progress", {
      path: "/tmp/other.md",
      phase: "error",
      channel: null,
      message: "transcript updated, but the audio could not be deleted: permission denied",
    });

    await waitFor(() => expect(document.body.textContent).toContain("rewritten by whisper"));
    expect(document.body.textContent).not.toContain("original parakeet text");
  });

  it("does not refetch on an ordinary re-transcribe error (the transcript was never touched)", async () => {
    const sessions: SessionMeta[] = [
      { path: "/tmp/other.md", title: "Other call", date: new Date().toISOString(), duration_secs: null },
    ];
    vi.mocked(ipc.listSessions).mockResolvedValue(sessions);
    vi.mocked(ipc.readSession).mockResolvedValue({
      title: "Other call",
      date: "",
      notes: "old notes",
      transcript: "[00:00:01] Me: hi",
      audio_path: "/tmp/other.flac",
    });

    render(<App />);

    const otherRow = await screen.findByRole("button", { name: /other call/i });
    fireEvent.click(otherRow);
    await waitFor(() => expect(document.body.textContent).toContain("hi"));

    const callsBeforeEvent = vi.mocked(ipc.readSession).mock.calls.length;

    emit("retranscribe-progress", {
      path: "/tmp/other.md",
      phase: "error",
      channel: null,
      message: "whisper model not downloaded",
    });

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

  it("shows an AUTO job's failure in the header even for the session currently being viewed (the common case)", async () => {
    // Second review round finding: an earlier version of this fix suppressed
    // the header whenever `path === selectedPath`, on the theory that a
    // manual click's own inline error would already cover it. But
    // `selectedPath` stays pointed at whatever session just stopped
    // recording (`handleStop` never changes it), which is exactly the
    // session an auto job runs for -- so that guard silenced precisely the
    // ordinary case this whole fix exists for. Nobody clicked anything here;
    // the failure must still reach the header.
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

    await screen.findByText(/whisper model not downloaded/i);
  });

  it("does not double-report in the header a MANUAL click's failure while that click is still in flight", async () => {
    // Drives the manual click through the real UI (not a bare event) so the
    // in-flight state App relies on (`manualRetranscribePathsRef`) is genuine:
    // SessionView's `handleRetranscribe` calls `onRetranscribeStart` the
    // instant the click fires, before `retranscribeSession` resolves.
    const sessions: SessionMeta[] = [
      { path: "/tmp/viewed.md", title: "Viewed call", date: new Date().toISOString(), duration_secs: null },
    ];
    vi.mocked(ipc.listSessions).mockResolvedValue(sessions);
    vi.mocked(ipc.readSession).mockResolvedValue({
      title: "Viewed call",
      date: "",
      notes: "",
      transcript: "[00:00:01] Me: hi",
      audio_path: "/tmp/viewed.flac",
    });
    // Never resolves: mirrors the real backend, where the promise only
    // settles once the job's own terminal event has already been decided --
    // the test emits that terminal event itself below, independently.
    vi.mocked(ipc.retranscribeSession).mockImplementationOnce(() => new Promise(() => {}));

    render(<App />);

    const viewedRow = await screen.findByRole("button", { name: /viewed call/i });
    fireEvent.click(viewedRow);
    await waitFor(() => expect(document.body.textContent).toContain("hi"));

    const retranscribeButton = await screen.findByRole("button", { name: /^re-transcribe$/i });
    fireEvent.click(retranscribeButton);
    await screen.findByRole("button", { name: /re-transcribing/i });

    emit("retranscribe-progress", {
      path: "/tmp/viewed.md",
      phase: "error",
      channel: null,
      message: "whisper model not downloaded",
    });

    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(screen.queryByText(/whisper model not downloaded/i)).not.toBeInTheDocument();
  });

  it("does not permanently silence a LATER auto-job failure after a manual click rejects with no terminal event (third review round)", async () => {
    // Reproduces the leak: `retranscribe_session` rejects up front, before
    // emitting anything at all, on several validation paths (model not
    // downloaded, missing audio, not-active, already-running/queued -- see
    // retranscribe.rs). A manual click hitting one of those shows its error
    // inline correctly (SessionView's own catch), but no
    // `retranscribe-progress` event ever fires for it -- so the listener's
    // terminal-event cleanup never runs, and the old code left the path
    // marked "manual in flight" forever. The very next genuine auto-job
    // failure for that same session would then be silently swallowed too.
    const sessions: SessionMeta[] = [
      { path: "/tmp/viewed.md", title: "Viewed call", date: new Date().toISOString(), duration_secs: null },
    ];
    vi.mocked(ipc.listSessions).mockResolvedValue(sessions);
    vi.mocked(ipc.readSession).mockResolvedValue({
      title: "Viewed call",
      date: "",
      notes: "",
      transcript: "[00:00:01] Me: hi",
      audio_path: "/tmp/viewed.flac",
    });
    // Rejects immediately, exactly like one of the up-front validation
    // checks -- no progress event ever accompanies this.
    vi.mocked(ipc.retranscribeSession).mockRejectedValueOnce(new Error("whisper model not downloaded"));

    render(<App />);

    const viewedRow = await screen.findByRole("button", { name: /viewed call/i });
    fireEvent.click(viewedRow);
    await waitFor(() => expect(document.body.textContent).toContain("hi"));

    const retranscribeButton = await screen.findByRole("button", { name: /^re-transcribe$/i });
    fireEvent.click(retranscribeButton);

    // The manual click's own inline error, exactly as before this fix.
    await screen.findByText(/whisper model not downloaded/i);

    // A LATER, genuinely automatic job for the SAME session fails. Nothing
    // manual is in flight anymore (the click above already rejected and
    // finished) -- this must reach the header, not be swallowed by a stale
    // "manual in flight" entry left over from the click.
    emit("retranscribe-progress", {
      path: "/tmp/viewed.md",
      phase: "error",
      channel: null,
      message: "delete-after-retranscribe failed: permission denied",
    });

    await screen.findByText(/delete-after-retranscribe failed: permission denied/i);
  });

  it("does not lose a background error that lands mid-recording -- surfaces it once the recording stops (final review Important finding)", async () => {
    // Reproduces the bug: `maybe_auto_retranscribe` emits its terminal error
    // inside `stop_recording`, before that command's own promise resolves.
    // The listener already (correctly) refuses to clobber a genuinely live
    // "recording" status with an unrelated background failure -- but the old
    // code just discarded the message right there in that branch, forever.
    // If status never happens to be non-"recording" again while some OTHER
    // background error also fires, the owner would never learn about this
    // one at all.
    render(<App />);

    const recordButton = await screen.findByRole("button", { name: /^record$/i });
    fireEvent.click(recordButton);
    await waitFor(() => expect(ipc.startRecording).toHaveBeenCalledTimes(1));
    // Flips status.kind from "loading-model" to "recording".
    emit("transcript-line", "[00:00:01] Me: hi");
    await screen.findByRole("button", { name: /stop/i });

    // A background job's error lands WHILE status is genuinely "recording".
    emit("retranscribe-progress", {
      path: "/tmp/some-other-session.md",
      phase: "error",
      channel: null,
      message: "whisper model not downloaded",
    });

    // Must not be shown yet -- and must not clobber the "recording" status.
    expect(screen.queryByText(/whisper model not downloaded/i)).not.toBeInTheDocument();
    await screen.findByRole("button", { name: /stop/i });

    // Stop the recording: status leaves "recording" (-> "saved").
    const stopButton = await screen.findByRole("button", { name: /stop/i });
    fireEvent.click(stopButton);
    await waitFor(() => expect(ipc.stopRecording).toHaveBeenCalledTimes(1));

    // The stashed error surfaces now, instead of "saved to ..." or nothing.
    await screen.findByText(/whisper model not downloaded/i);
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
