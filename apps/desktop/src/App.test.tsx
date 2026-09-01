import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "@/App";
import * as ipc from "@/lib/ipc";
import type { SessionMeta } from "@/lib/ipc";
import { listen } from "@tauri-apps/api/event";
import { ask } from "@tauri-apps/plugin-dialog";

/**
 * App drives three "real" Tauri surfaces directly: the `@/lib/ipc` wrappers
 * (also used by SessionView/SettingsDialog, so one mock covers all three
 * components), the raw `@tauri-apps/api/event` `listen` (App and
 * SettingsDialog both call it directly, not through ipc.ts), and
 * `@tauri-apps/plugin-dialog`'s `open` (used by Settings and onboarding, but
 * never invoked in these App-level tests).
 */
vi.mock("@/lib/ipc", () => ({
  getConfig: vi.fn().mockResolvedValue({
    output_dir: "/tmp",
    active_model: "m1",
    onboarding_complete: true,
    theme: "system",
    show_in_dock: true,
    keep_audio: true,
    audio_format: "aac",
    retranscribe_model: "w1",
    auto_retranscribe: false,
    delete_audio_after_retranscribe: false,
    enhance_provider: "claude",
    enhance_models: {},
    auto_enhance: false,
    enhance_rename_title: false,
    enhance_use_calendar: false,
    detect_calls: true,
    auto_stop_after_call: false,
  }),
  listSessions: vi.fn().mockResolvedValue([]),
  readSession: vi
    .fn()
    .mockResolvedValue({ title: "", date: "", notes: "", transcript: "", audio_path: null, summary: "", summary_model: null }),
  startRecording: vi.fn().mockResolvedValue("/tmp/active.md"),
  stopRecording: vi.fn().mockResolvedValue("/tmp/active.md"),
  saveSession: vi.fn().mockResolvedValue(undefined),
  updateActiveSession: vi.fn().mockResolvedValue(undefined),
  listModels: vi
    .fn()
    .mockResolvedValue([{ id: "m1", name: "Test model", size_mb: 100, downloaded: true, active: true }]),
  downloadModel: vi.fn().mockResolvedValue(undefined),
  checkAudioPermissions: vi.fn().mockResolvedValue({
    microphone: { granted: true, error: null },
    system_audio: { granted: true, error: null },
  }),
  completeOnboarding: vi.fn().mockResolvedValue(undefined),
  setActiveModel: vi.fn().mockResolvedValue(undefined),
  setOutputDir: vi.fn().mockResolvedValue(undefined),
  setTheme: vi.fn().mockResolvedValue(undefined),
  setKeepAudio: vi.fn().mockResolvedValue(undefined),
  setAudioFormat: vi.fn().mockResolvedValue(undefined),
  setAutoRetranscribe: vi.fn().mockResolvedValue(undefined),
  setDeleteAudioAfterRetranscribe: vi.fn().mockResolvedValue(undefined),
  setDetectCalls: vi.fn().mockResolvedValue(undefined),
  setAutoStopAfterCall: vi.fn().mockResolvedValue(undefined),
  setRetranscribeModel: vi.fn().mockResolvedValue(undefined),
  deleteModel: vi.fn().mockResolvedValue(undefined),
  retranscribeSession: vi.fn().mockResolvedValue(undefined),
  createSession: vi.fn().mockResolvedValue("/tmp/new.md"),
  searchSessions: vi.fn().mockResolvedValue([]),
  deleteSession: vi.fn().mockResolvedValue(undefined),
  enhanceSession: vi.fn().mockResolvedValue(""),
  setEnhanceProvider: vi.fn().mockResolvedValue(undefined),
  setEnhanceModel: vi.fn().mockResolvedValue(undefined),
  setAutoEnhance: vi.fn().mockResolvedValue(undefined),
  setEnhanceRenameTitle: vi.fn().mockResolvedValue(undefined),
  setEnhanceUseCalendar: vi.fn().mockResolvedValue(undefined),
  listEnhanceProviders: vi.fn().mockResolvedValue([]),
  showMainWindow: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
  ask: vi.fn().mockResolvedValue(false),
}));

vi.mock("@tauri-apps/plugin-opener", () => ({
  openPath: vi.fn().mockResolvedValue(undefined),
  revealItemInDir: vi.fn().mockResolvedValue(undefined),
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

/**
 * The control that starts a recording depends on the screen: the empty
 * state (nothing selected) offers a "Start Recording" row and hides the
 * topbar; once a session is open (or recording), the topbar's "Record"
 * button exists instead. One matcher covers tests starting from either.
 */
function findStartControl() {
  return screen.findByRole("button", { name: /^(start recording|record$)/i });
}

beforeEach(() => {
  vi.mocked(ipc.getConfig).mockReset().mockResolvedValue({
    output_dir: "/tmp",
    active_model: "m1",
    onboarding_complete: true,
    theme: "system",
    show_in_dock: true,
    keep_audio: true,
    audio_format: "aac",
    retranscribe_model: "w1",
    auto_retranscribe: false,
    delete_audio_after_retranscribe: false,
    enhance_provider: "claude",
    enhance_models: {},
    auto_enhance: false,
    enhance_rename_title: false,
    enhance_use_calendar: false,
    detect_calls: true,
    auto_stop_after_call: false,
  });
  vi.mocked(ipc.startRecording).mockReset().mockResolvedValue("/tmp/active.md");
  vi.mocked(ipc.stopRecording).mockReset().mockResolvedValue("/tmp/active.md");
  vi.mocked(ipc.listSessions).mockClear();
  vi.mocked(ipc.saveSession).mockClear();
  vi.mocked(ipc.updateActiveSession).mockClear();
  vi.mocked(ipc.retranscribeSession).mockClear().mockResolvedValue(undefined);
  vi.mocked(ipc.createSession).mockClear().mockResolvedValue("/tmp/new.md");
  vi.mocked(ipc.showMainWindow).mockClear();
  vi.mocked(listen).mockClear();
});

describe("App: guided first run", () => {
  it("shows onboarding and keeps active-window shortcuts from bypassing it", async () => {
    vi.mocked(ipc.getConfig).mockResolvedValue({
      ...(await ipc.getConfig()),
      onboarding_complete: false,
    });
    vi.mocked(ipc.listModels).mockResolvedValue([
      {
        id: "m1",
        name: "Test model",
        technical_name: null,
        tagline: null,
        size_mb: 100,
        downloaded: true,
        active: true,
        role: "live",
        size_on_disk_bytes: 100,
        retired: false,
      },
    ]);

    render(<App />);
    expect(await screen.findByText("Your calls stay on this Mac.")).toBeVisible();
    fireEvent.keyDown(window, { key: "n", metaKey: true });
    expect(ipc.createSession).not.toHaveBeenCalled();
  });
});

describe("App: call detection via the banner window (stage 1.1)", () => {
  /** Fires the event BannerApp (the banner window) sends on Start. */
  async function emitBannerStart(
    payload = { bundle_id: "com.tinyspeck.slackmacgap", app_name: "Slack" },
  ) {
    await waitFor(() =>
      expect(vi.mocked(listen).mock.calls.some(([name]) => name === "banner-start")).toBe(true),
    );
    emit("banner-start", payload);
  }

  it("starts a new pre-titled recording from a banner-start event", async () => {
    render(<App />);
    await emitBannerStart();

    await waitFor(() => expect(ipc.startRecording).toHaveBeenCalledTimes(1));
    const [path, title, bundleId] = vi.mocked(ipc.startRecording).mock.calls[0];
    expect(path).toBeNull();
    expect(title).toMatch(/^Slack call /);
    expect(bundleId).toBe("com.tinyspeck.slackmacgap");
    expect(ipc.showMainWindow).not.toHaveBeenCalled();
  });

  it("surfaces the main window instead of starting while recording", async () => {
    render(<App />);
    fireEvent.click(await findStartControl());
    await screen.findByRole("button", { name: /stop/i });

    await emitBannerStart();
    await waitFor(() => expect(ipc.showMainWindow).toHaveBeenCalledTimes(1));
    expect(ipc.startRecording).toHaveBeenCalledTimes(1);
  });

  it("surfaces the main window instead of starting when the model is not ready", async () => {
    vi.mocked(ipc.listModels).mockResolvedValueOnce([
      {
        id: "m1",
        name: "Test model",
        technical_name: null,
        tagline: null,
        size_mb: 100,
        downloaded: false,
        active: true,
        role: "live",
        size_on_disk_bytes: 0,
        retired: false,
      },
    ]);
    render(<App />);
    await screen.findByText("model not downloaded");

    await emitBannerStart();
    await waitFor(() => expect(ipc.showMainWindow).toHaveBeenCalledTimes(1));
    expect(ipc.startRecording).not.toHaveBeenCalled();
  });

  it("does not duplicate the model warning beside its toolbar action", async () => {
    vi.mocked(ipc.listModels).mockResolvedValueOnce([
      {
        id: "m1",
        name: "Test model",
        technical_name: null,
        tagline: null,
        size_mb: 100,
        downloaded: false,
        active: true,
        role: "live",
        size_on_disk_bytes: 0,
        retired: false,
      },
    ]);

    render(<App />);
    await screen.findByText("model not downloaded");
    fireEvent.click(screen.getByRole("button", { name: /^New Note⌘N$/ }));

    expect(await screen.findByRole("button", { name: "Open settings" })).toBeVisible();
    expect(screen.queryByText("model not downloaded")).not.toBeInTheDocument();
  });

  it("stops through the normal stop path on a banner-stop event", async () => {
    render(<App />);
    await emitBannerStart();
    await screen.findByRole("button", { name: /stop/i });

    emit("banner-stop", undefined);
    await waitFor(() => expect(ipc.stopRecording).toHaveBeenCalledTimes(1));
  });

  it("auto-stops through stopRecording when the setting is enabled", async () => {
    vi.mocked(ipc.getConfig).mockResolvedValue({
      output_dir: "/tmp",
      active_model: "m1",
      onboarding_complete: true,
      theme: "system",
      show_in_dock: true,
      keep_audio: true,
      audio_format: "aac",
      retranscribe_model: "w1",
      auto_retranscribe: false,
      delete_audio_after_retranscribe: false,
      enhance_provider: "claude",
    enhance_models: {},
    auto_enhance: false,
    enhance_rename_title: false,
    enhance_use_calendar: false,
      detect_calls: true,
      auto_stop_after_call: true,
    });
    render(<App />);
    await emitBannerStart();
    await screen.findByRole("button", { name: /stop/i });

    const ended = {
      bundle_id: "com.tinyspeck.slackmacgap",
      recording_id: "/tmp/active.md",
    };
    emit("call-ended", ended);
    emit("call-ended", ended);
    await waitFor(() => expect(ipc.stopRecording).toHaveBeenCalledTimes(1));
  });

  it("auto-stops a manually started recording after an associated call ends", async () => {
    vi.mocked(ipc.getConfig).mockResolvedValue({
      output_dir: "/tmp",
      active_model: "m1",
      onboarding_complete: true,
      theme: "system",
      show_in_dock: true,
      keep_audio: true,
      audio_format: "aac",
      retranscribe_model: "w1",
      auto_retranscribe: false,
      delete_audio_after_retranscribe: false,
      enhance_provider: "claude",
    enhance_models: {},
    auto_enhance: false,
    enhance_rename_title: false,
    enhance_use_calendar: false,
      detect_calls: true,
      auto_stop_after_call: true,
    });
    render(<App />);
    fireEvent.click(await findStartControl());
    await screen.findByRole("button", { name: /stop/i });

    emit("call-ended", {
      bundle_id: "org.mozilla.firefox",
      recording_id: "/tmp/active.md",
    });
    await waitFor(() => expect(ipc.stopRecording).toHaveBeenCalledTimes(1));
  });
});

describe("App: menu-bar recording toggle (stage 3)", () => {
  async function emitMenuBarToggle() {
    await waitFor(() =>
      expect(
        vi.mocked(listen).mock.calls.some(([name]) => name === "menu-bar-toggle-recording"),
      ).toBe(true),
    );
    emit("menu-bar-toggle-recording", undefined);
  }

  it("starts a fresh session even when an old note is selected", async () => {
    vi.mocked(ipc.listSessions).mockResolvedValueOnce([
      {
        path: "/tmp/old.md",
        title: "Old note",
        date: "2026-08-27T10:00:00+02:00",
        duration_secs: 10,
      },
    ]);
    render(<App />);
    fireEvent.click(await screen.findByText("Old note"));

    await emitMenuBarToggle();
    await waitFor(() => expect(ipc.startRecording).toHaveBeenCalledTimes(1));
    expect(ipc.startRecording).toHaveBeenCalledWith(null);
  });

  it("stops the active session through the normal stop path", async () => {
    render(<App />);
    await emitMenuBarToggle();
    await waitFor(() => expect(ipc.startRecording).toHaveBeenCalledTimes(1));

    await emitMenuBarToggle();
    await waitFor(() => expect(ipc.stopRecording).toHaveBeenCalledTimes(1));
    expect(ipc.startRecording).toHaveBeenCalledTimes(1);
  });

  it("opens the main window when the live model is unavailable", async () => {
    vi.mocked(ipc.listModels).mockResolvedValueOnce([
      {
        id: "m1",
        name: "Test model",
        technical_name: null,
        tagline: null,
        size_mb: 100,
        downloaded: false,
        active: true,
        role: "live",
        size_on_disk_bytes: 0,
        retired: false,
      },
    ]);
    render(<App />);
    await screen.findByText("model not downloaded");

    await emitMenuBarToggle();
    await waitFor(() => expect(ipc.showMainWindow).toHaveBeenCalledTimes(1));
    expect(ipc.startRecording).not.toHaveBeenCalled();
  });

  it("creates a note and restores the window from the native menu", async () => {
    render(<App />);
    await waitFor(() =>
      expect(vi.mocked(listen).mock.calls.some(([name]) => name === "menu-bar-new-note")).toBe(true),
    );

    emit("menu-bar-new-note", undefined);

    await waitFor(() => expect(ipc.showMainWindow).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(ipc.createSession).toHaveBeenCalledTimes(1));
  });

  it("opens Settings from the native menu", async () => {
    render(<App />);
    await waitFor(() =>
      expect(vi.mocked(listen).mock.calls.some(([name]) => name === "menu-bar-open-settings")).toBe(true),
    );

    emit("menu-bar-open-settings", undefined);

    await waitFor(() => expect(ipc.showMainWindow).toHaveBeenCalledTimes(1));
    expect(await screen.findByRole("dialog")).toBeVisible();
  });
});

describe("App: hot-mic fix (final review Critical finding)", () => {
  it("leaves recording mode immediately while slow finalization stays in the background", async () => {
    let finishStop!: (path: string) => void;
    vi.mocked(ipc.stopRecording).mockImplementation(
      () => new Promise<string>((resolve) => { finishStop = resolve; }),
    );
    render(<App />);

    fireEvent.click(await findStartControl());
    const stopButton = await screen.findByRole("button", { name: /stop/i });
    fireEvent.click(stopButton);

    await waitFor(() => expect(ipc.stopRecording).toHaveBeenCalledTimes(1));
    expect(screen.getByText("saving")).toBeVisible();
    expect(screen.queryByRole("button", { name: /stop/i })).not.toBeInTheDocument();

    act(() => finishStop("/tmp/active.md"));
    expect(await screen.findByText(/^saved$/)).toBeVisible();
  });

  it("keeps the record control wired to Stop when a session-error lands mid-recording", async () => {
    render(<App />);

    const recordButton = await findStartControl();
    fireEvent.click(recordButton);
    await waitFor(() => expect(ipc.startRecording).toHaveBeenCalledTimes(1));

    // Recording is in progress: the button already reads "Stop ...".
    await screen.findByRole("button", { name: /stop/i });

    // A worker-side error (e.g. a save failure) lands mid-recording. Before
    // the fix, deriving the button's mode from `status.kind` meant this
    // flipped the button back to "Record" -- clicking it would then run
    // handleStart against an already-running session instead of stopping it.
    emit("session-error", { message: "disk write failed" });

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Something went wrong");
    expect(alert).toHaveTextContent("disk write failed");

    fireEvent.click(screen.getByRole("button", { name: "Dismiss error" }));
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();

    const button = await screen.findByRole("button", { name: /stop/i });
    expect(button).toBeVisible();

    fireEvent.click(button);

    await waitFor(() => expect(ipc.stopRecording).toHaveBeenCalledTimes(1));
    expect(ipc.startRecording).toHaveBeenCalledTimes(1);
  });
});

describe("App: a saving session must not block the next call (owner bug: missed a call start)", () => {
  /** Starts a recording, then stops it with `stopRecording` left pending, so
   *  the app sits in the "saving" state a long encode produces. Returns the
   *  resolver for the pending stop. */
  async function enterSavingState() {
    let finishStop!: (path: string) => void;
    vi.mocked(ipc.stopRecording).mockImplementation(
      () => new Promise<string>((resolve) => { finishStop = resolve; }),
    );
    render(<App />);
    fireEvent.click(await findStartControl());
    fireEvent.click(await screen.findByRole("button", { name: /stop/i }));
    await waitFor(() => expect(ipc.stopRecording).toHaveBeenCalledTimes(1));
    expect(screen.getByText("saving")).toBeVisible();
    return (path: string) => act(() => finishStop(path));
  }

  it("creates a new note and records into it while the previous session is still saving", async () => {
    const finishStop = await enterSavingState();

    fireEvent.click(screen.getByRole("button", { name: /new note/i }));
    await waitFor(() => expect(ipc.createSession).toHaveBeenCalledTimes(1));

    fireEvent.click(await findStartControl());
    await waitFor(() => expect(ipc.startRecording).toHaveBeenCalledTimes(2));
    expect(vi.mocked(ipc.startRecording).mock.calls[1][0]).toBe("/tmp/new.md");
    await screen.findByRole("button", { name: /stop/i });

    // The old call's save finishing must not repaint the status of the NEW,
    // live recording as "saved" (the record control must stay a Stop).
    finishStop("/tmp/active.md");
    expect(screen.getByRole("button", { name: /stop/i })).toBeVisible();
    expect(screen.queryByText(/^saved$/)).not.toBeInTheDocument();
  });

  it("starts a detected call from the banner while the previous session is still saving", async () => {
    vi.mocked(ipc.startRecording).mockResolvedValue("/tmp/detected.md");
    await enterSavingState();

    emit("banner-start", { bundle_id: "us.zoom.xos", app_name: "Zoom" });

    await waitFor(() => expect(ipc.startRecording).toHaveBeenCalledTimes(2));
    // A detected call always records into a brand-new session (path null),
    // which the backend accepts even mid-finalization.
    expect(vi.mocked(ipc.startRecording).mock.calls[1][0]).toBeNull();
  });

  it("still reports the finished save when nothing newer replaced the status", async () => {
    const finishStop = await enterSavingState();
    finishStop("/tmp/active.md");
    expect(await screen.findByText(/^saved$/)).toBeVisible();
  });

  it("surfaces a failed background save as an error even after a new recording started", async () => {
    let failStop!: (err: unknown) => void;
    vi.mocked(ipc.stopRecording).mockImplementation(
      () => new Promise<string>((_resolve, reject) => { failStop = reject; }),
    );
    render(<App />);
    fireEvent.click(await findStartControl());
    fireEvent.click(await screen.findByRole("button", { name: /stop/i }));
    await waitFor(() => expect(ipc.stopRecording).toHaveBeenCalledTimes(1));

    fireEvent.click(screen.getByRole("button", { name: /new note/i }));
    await waitFor(() => expect(ipc.createSession).toHaveBeenCalledTimes(1));
    fireEvent.click(await findStartControl());
    await screen.findByRole("button", { name: /stop/i });

    // A save failure means the old call's file may be incomplete; the owner
    // must see it even though a new recording is already running.
    act(() => failStop("encode failed: disk full"));
    expect(await screen.findByText(/encode failed: disk full/)).toBeVisible();
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
          summary: "",
          summary_model: null,
        };
      }
      return { title: "Untitled", date: "", notes: "", transcript: "", audio_path: null, summary: "", summary_model: null };
    });

    render(<App />);

    const recordButton = await findStartControl();
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

    const recordButton = await findStartControl();
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
      summary: "",
      summary_model: null,
    });
    vi.mocked(ipc.startRecording).mockResolvedValue("/tmp/other.md");

    render(<App />);

    const otherRow = await screen.findByRole("button", { name: /other call/i });
    fireEvent.click(otherRow);
    await screen.findByDisplayValue("old notes");

    const recordButton = await findStartControl();
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
      summary: "",
      summary_model: null,
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

    const recordButton = await findStartControl();
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
      summary: "",
      summary_model: null,
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
      summary: "",
      summary_model: null,
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
      summary: "",
      summary_model: null,
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
      summary: "",
      summary_model: null,
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
      summary: "",
      summary_model: null,
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
      summary: "",
      summary_model: null,
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
      summary: "",
      summary_model: null,
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
      summary: "",
      summary_model: null,
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
      summary: "",
      summary_model: null,
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
      summary: "",
      summary_model: null,
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

    const recordButton = await findStartControl();
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

    // The stashed error surfaces now, instead of "saved" or nothing.
    await screen.findByText(/whisper model not downloaded/i);
  });
});

describe("App: sidebar refresh on model-ready (final review Important finding)", () => {
  it("refreshes the session list exactly once when the model finishes loading, not on every later line", async () => {
    render(<App />);

    const recordButton = await findStartControl();
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

describe("App: deleting a note from the sidebar (stage 0.10)", () => {
  const doomed: SessionMeta[] = [
    { path: "/tmp/doomed.md", title: "Doomed call", date: new Date().toISOString(), duration_secs: null },
  ];

  beforeEach(() => {
    vi.mocked(ipc.deleteSession).mockClear().mockResolvedValue(undefined);
    vi.mocked(ask).mockClear();
  });

  it("confirms with the note's title, deletes, clears the selection and refreshes the list", async () => {
    vi.mocked(ipc.listSessions).mockResolvedValue(doomed);
    vi.mocked(ask).mockResolvedValue(true);
    vi.mocked(ipc.readSession).mockResolvedValue({
      title: "Doomed call",
      date: "",
      notes: "",
      transcript: "",
      audio_path: null,
      summary: "",
      summary_model: null,
    });

    render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: /doomed call/i }));
    await waitFor(() => expect(document.body.textContent).not.toContain("Start Recording"));

    const callsBefore = vi.mocked(ipc.listSessions).mock.calls.length;
    fireEvent.click(screen.getByRole("button", { name: /move to trash/i }));

    await waitFor(() =>
      expect(ipc.deleteSession).toHaveBeenCalledExactlyOnceWith("/tmp/doomed.md"),
    );
    // The dialog names the note and says where the files go.
    const askMessage = vi.mocked(ask).mock.calls[0][0];
    expect(askMessage).toContain('"Doomed call"');
    expect(askMessage).toContain("Trash");
    // The deleted note was selected: the pane must fall back to the empty
    // state, and the sidebar must re-scan the folder.
    await waitFor(() =>
      expect(document.body.textContent).toContain("Start Recording"),
    );
    await waitFor(() =>
      expect(vi.mocked(ipc.listSessions).mock.calls.length).toBeGreaterThan(callsBefore),
    );
  });

  it("does not delete anything when the confirm dialog is declined", async () => {
    vi.mocked(ipc.listSessions).mockResolvedValue(doomed);
    vi.mocked(ask).mockResolvedValue(false);

    render(<App />);
    await screen.findByRole("button", { name: /doomed call/i });
    fireEvent.click(screen.getByRole("button", { name: /move to trash/i }));

    await waitFor(() => expect(ask).toHaveBeenCalledTimes(1));
    expect(ipc.deleteSession).not.toHaveBeenCalled();
  });

  it("surfaces the backend's refusal message instead of silently swallowing it", async () => {
    vi.mocked(ipc.listSessions).mockResolvedValue(doomed);
    vi.mocked(ask).mockResolvedValue(true);
    vi.mocked(ipc.deleteSession).mockRejectedValue(
      "this session is being re-transcribed; wait for it to finish before deleting",
    );

    render(<App />);
    await screen.findByRole("button", { name: /doomed call/i });
    fireEvent.click(screen.getByRole("button", { name: /move to trash/i }));

    await waitFor(() =>
      expect(document.body.textContent).toContain("being re-transcribed"),
    );
  });
});

describe("App: global search (stage 2.0)", () => {
  const all: SessionMeta[] = [
    { path: "/tmp/sync.md", title: "Weekly sync", date: new Date().toISOString(), duration_secs: null },
    { path: "/tmp/standup.md", title: "Standup", date: new Date().toISOString(), duration_secs: null },
  ];
  const hit = {
    ...all[0],
    snippets: [
      { section: "transcript" as const, text: "[00:00:04] Me: about the budget" },
    ],
    total_matches: 1,
  };

  it("opens a modal palette and leaves the sidebar list in place while searching", async () => {
    vi.mocked(ipc.listSessions).mockResolvedValue(all);
    vi.mocked(ipc.searchSessions).mockClear().mockResolvedValue([hit]);

    render(<App />);
    await screen.findByRole("button", { name: /standup/i });

    fireEvent.click(screen.getByRole("button", { name: /search all notes/i }));
    const input = screen.getByRole("searchbox", { name: /search all notes/i });
    fireEvent.change(input, { target: { value: "budget" } });

    await waitFor(() =>
      expect(ipc.searchSessions).toHaveBeenCalledExactlyOnceWith("budget"),
    );
    expect(screen.getByText("Standup")).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Spoken" })).toBeInTheDocument();
    const mark = document.querySelector("mark");
    expect(mark?.textContent).toBe("budget");
  });

  it("opens a spoken result and closes the palette", async () => {
    vi.mocked(ipc.listSessions).mockResolvedValue(all);
    vi.mocked(ipc.searchSessions).mockClear().mockResolvedValue([hit]);
    vi.mocked(ipc.readSession).mockResolvedValue({
      title: "Weekly sync",
      date: "",
      notes: "",
      transcript: "[00:00:04] Me: about the budget",
      audio_path: null,
      summary: "",
      summary_model: null,
    });

    render(<App />);
    await screen.findByRole("button", { name: /^New Note⌘N$/ });
    fireEvent.keyDown(window, { key: "k", metaKey: true });
    fireEvent.change(screen.getByRole("searchbox", { name: /search all notes/i }), {
      target: { value: "budget" },
    });
    await waitFor(() => expect(ipc.searchSessions).toHaveBeenCalledWith("budget"));

    fireEvent.click(await screen.findByRole("option", { name: /about the budget/i }));

    await waitFor(() => expect(screen.queryByRole("dialog", { name: /search all notes/i })).toBeNull());
    await waitFor(() =>
      expect(document.body.textContent).toContain("about the budget"),
    );
    expect(screen.getByRole("searchbox", { name: "Find in transcript" })).toHaveValue("budget");
    expect(document.querySelector('[data-current-match="true"]')).toHaveTextContent("budget");
  });

  it("uses Command+F for the selected transcript and keeps Command+K global", async () => {
    vi.mocked(ipc.listSessions).mockResolvedValue(all);
    vi.mocked(ipc.readSession).mockResolvedValue({
      title: "Weekly sync",
      date: "",
      notes: "",
      transcript: "[00:00:04] Me: about the budget",
      audio_path: null,
      summary: "",
      summary_model: null,
    });

    render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: /weekly sync/i }));
    await screen.findByText("about the budget");

    fireEvent.keyDown(window, { key: "f", metaKey: true });
    expect(await screen.findByRole("searchbox", { name: "Find in transcript" })).toHaveFocus();
    expect(screen.getByRole("tab", { name: "Transcript" })).toHaveAttribute("data-state", "active");

    fireEvent.keyDown(window, { key: "k", metaKey: true });
    expect(screen.getByRole("dialog", { name: /search all notes/i })).toBeInTheDocument();
  });

  it("falls back to global search for Command+F when no note is selected", async () => {
    vi.mocked(ipc.listSessions).mockResolvedValue([]);
    render(<App />);
    await screen.findByText("New Note");

    fireEvent.keyDown(window, { key: "f", metaKey: true });

    expect(screen.getByRole("dialog", { name: /search all notes/i })).toBeInTheDocument();
  });
});

describe("App: empty state and active-window shortcuts (stage 0.10)", () => {
  it("shows the shortcut menu when nothing is selected, and its rows work", async () => {
    render(<App />);
    await screen.findByText("New Note");

    // Settings first: the New Note row selects the created note, which
    // replaces this menu with the session view.
    fireEvent.click(screen.getByRole("button", { name: /settings.*⌘/i }));
    await screen.findByRole("dialog");
    fireEvent.keyDown(document.activeElement ?? document.body, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    fireEvent.click(screen.getByRole("button", { name: /new note.*⌘.*n/i }));
    await waitFor(() => expect(ipc.createSession).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(screen.queryByText("Start Recording")).toBeNull());
  });

  it("⌘N creates a note, ⌘⇧N starts recording, and ⌘K opens search", async () => {
    render(<App />);
    await screen.findByText("New Note");

    fireEvent.keyDown(window, { key: "n", metaKey: true });
    await waitFor(() => expect(ipc.createSession).toHaveBeenCalledTimes(1));
    // The whole new-note flow must settle (`busy` back to false) before the
    // record hotkey can pass its gating.
    await waitFor(() =>
      expect(screen.getByRole("button", { name: /^record$/i })).toBeEnabled(),
    );

    fireEvent.keyDown(window, { key: "n", metaKey: true, shiftKey: true });
    await waitFor(() => expect(ipc.startRecording).toHaveBeenCalledTimes(1));

    fireEvent.keyDown(window, { key: "k", metaKey: true });
    expect(screen.getByRole("dialog", { name: /search all notes/i })).toBeInTheDocument();
  });

  it("collapses and restores the sidebar without changing the selected note", async () => {
    const note: SessionMeta[] = [{
      path: "/tmp/note.md",
      title: "Kept note",
      date: new Date().toISOString(),
      duration_secs: null,
    }];
    vi.mocked(ipc.listSessions).mockResolvedValue(note);
    vi.mocked(ipc.readSession).mockResolvedValue({
      title: "Kept note",
      date: "",
      notes: "still here",
      transcript: "",
      audio_path: null,
      summary: "",
      summary_model: null,
    });

    render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: /kept note/i }));
    await screen.findByRole("button", { name: "Edit notes" });
    const renderedNote = document.querySelector(".markdown-content");
    expect(renderedNote).toHaveTextContent("still here");
    fireEvent.keyDown(window, { key: "\\", metaKey: true });

    expect(screen.queryByRole("button", { name: /kept note/i })).toBeNull();
    expect(renderedNote).toBeInTheDocument();
    expect(renderedNote).toHaveTextContent("still here");
    const showSidebar = screen.getByRole("button", { name: /show sidebar/i });
    expect(showSidebar.className).not.toContain("bg-foreground/[0.06]");
    expect(showSidebar.parentElement?.className).toContain("w-56");
    expect(showSidebar.parentElement?.className).toContain("pl-[88px]");
    expect(showSidebar.className).toContain("size-6");
    expect(showSidebar.nextElementSibling).toHaveAttribute(
      "aria-label",
      "Search all notes",
    );
    expect(showSidebar.nextElementSibling?.nextElementSibling).toHaveAttribute(
      "aria-label",
      "New note",
    );
    fireEvent.click(showSidebar);
    expect(await screen.findByRole("button", { name: /kept note/i })).toBeInTheDocument();
  });

  it("⌘⇧N does nothing while already recording", async () => {
    render(<App />);
    fireEvent.click(await findStartControl());
    await screen.findByRole("button", { name: /stop/i });
    expect(ipc.startRecording).toHaveBeenCalledTimes(1);

    fireEvent.keyDown(window, { key: "n", metaKey: true, shiftKey: true });
    // Give any (wrong) async start a tick to land before asserting.
    await new Promise((r) => setTimeout(r, 20));
    expect(ipc.startRecording).toHaveBeenCalledTimes(1);
  });
});

describe("App: note actions menu (stage 0.10)", () => {
  const one: SessionMeta[] = [
    { path: "/tmp/sync.md", title: "Weekly sync", date: new Date().toISOString(), duration_secs: null },
  ];

  it("offers Open file and Show in Finder for the open note", async () => {
    const opener = await import("@tauri-apps/plugin-opener");
    vi.mocked(ipc.listSessions).mockResolvedValue(one);

    render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: /weekly sync/i }));
    const trigger = await screen.findByRole("button", { name: /more actions/i });

    fireEvent.pointerDown(trigger);
    fireEvent.click(trigger);
    fireEvent.click(await screen.findByText("Open file"));
    await waitFor(() =>
      expect(opener.openPath).toHaveBeenCalledExactlyOnceWith("/tmp/sync.md"),
    );

    fireEvent.pointerDown(trigger);
    fireEvent.click(trigger);
    fireEvent.click(await screen.findByText("Show in Finder"));
    await waitFor(() =>
      expect(opener.revealItemInDir).toHaveBeenCalledExactlyOnceWith("/tmp/sync.md"),
    );

    fireEvent.pointerDown(trigger);
    fireEvent.click(trigger);
    fireEvent.click(await screen.findByText("Settings"));
    expect(await screen.findByRole("dialog")).toBeVisible();
  });

  it("hides the menu when nothing is selected", async () => {
    render(<App />);
    await screen.findByText("New Note");
    expect(screen.queryByRole("button", { name: /more actions/i })).toBeNull();
  });
});

describe("App: enhance progress (stage 0.11)", () => {
  const one: SessionMeta[] = [
    { path: "/tmp/sync.md", title: "Weekly sync", date: new Date().toISOString(), duration_secs: null },
  ];

  it("refreshes the open note when its enhance lands", async () => {
    vi.mocked(ipc.listSessions).mockResolvedValue(one);
    vi.mocked(ipc.readSession).mockResolvedValue({
      title: "Weekly sync", date: "", notes: "", transcript: "[00:00:01] Me: hi",
      audio_path: null, summary: "", summary_model: null,
    });

    render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: /weekly sync/i }));
    await waitFor(() => expect(document.body.textContent).toContain("hi"));

    vi.mocked(ipc.readSession).mockResolvedValue({
      title: "Weekly sync", date: "", notes: "", transcript: "[00:00:01] Me: hi",
      audio_path: null, summary: "- fresh auto summary", summary_model: "claude",
    });
    emit("enhance-progress", { path: "/tmp/sync.md", phase: "done", provider: "claude", message: null });

    await screen.findByText("fresh auto summary");
    expect(document.body.textContent).toContain("by claude");
  });

  it("shows a summarizing row in the sidebar while an enhance runs, for any session", async () => {
    vi.mocked(ipc.listSessions).mockResolvedValue(one);
    render(<App />);
    const row = await screen.findByRole("button", { name: /weekly sync/i });

    emit("enhance-progress", { path: "/tmp/sync.md", phase: "running", provider: "local", message: null });
    await waitFor(() => expect(row.textContent).toContain("summarizing"));
    expect(within(row).getByTestId("working-dot")).toBeInTheDocument();

    emit("enhance-progress", { path: "/tmp/sync.md", phase: "done", provider: "local", message: null });
    await waitFor(() => expect(row.textContent).not.toContain("summarizing"));
  });

  it("shows the local map/reduce step in the working chip and keeps it fresh", async () => {
    vi.mocked(ipc.listSessions).mockResolvedValue(one);
    vi.mocked(ipc.readSession).mockResolvedValue({
      title: "Weekly sync", date: "", notes: "", transcript: "[00:00:01] Me: hi",
      audio_path: null, summary: "", summary_model: null,
    });

    render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: /weekly sync/i }));
    await waitFor(() => expect(document.body.textContent).toContain("hi"));

    emit("enhance-progress", { path: "/tmp/sync.md", phase: "running", provider: "local", message: null });
    const chip = await screen.findByTestId("enhance-working");
    expect(chip.textContent).toContain("Summarizing...");

    emit("enhance-progress", {
      path: "/tmp/sync.md",
      phase: "running",
      provider: "local",
      message: "Summarizing part 2 of 7...",
    });
    await waitFor(() => expect(chip.textContent).toContain("part 2 of 7"));

    emit("enhance-progress", { path: "/tmp/sync.md", phase: "done", provider: "local", message: null });
    await waitFor(() => expect(screen.queryByTestId("enhance-working")).not.toBeInTheDocument());
  });

  it("offers Re-enhance in the overflow menu and runs it for the open note", async () => {
    vi.mocked(ipc.listSessions).mockResolvedValue(one);
    vi.mocked(ipc.readSession).mockResolvedValue({
      title: "Weekly sync", date: "", notes: "", transcript: "[00:00:01] Me: hi",
      audio_path: null, summary: "- old summary", summary_model: "claude",
    });
    vi.mocked(ipc.enhanceSession).mockClear().mockResolvedValue("- new");

    render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: /weekly sync/i }));
    await screen.findByText("old summary");

    const trigger = screen.getByRole("button", { name: /more actions/i });
    fireEvent.pointerDown(trigger);
    fireEvent.click(trigger);
    const item = await screen.findByRole("menuitem", { name: /re-enhance/i });
    fireEvent.click(item);

    await waitFor(() =>
      expect(ipc.enhanceSession).toHaveBeenCalledExactlyOnceWith("/tmp/sync.md"),
    );
  });

  it("disables the overflow enhance item while a job is already running", async () => {
    vi.mocked(ipc.listSessions).mockResolvedValue(one);
    vi.mocked(ipc.readSession).mockResolvedValue({
      title: "Weekly sync", date: "", notes: "", transcript: "[00:00:01] Me: hi",
      audio_path: null, summary: "", summary_model: null,
    });

    render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: /weekly sync/i }));
    await waitFor(() => expect(document.body.textContent).toContain("hi"));

    emit("enhance-progress", { path: "/tmp/sync.md", phase: "running", provider: "local", message: null });

    const trigger = screen.getByRole("button", { name: /more actions/i });
    fireEvent.pointerDown(trigger);
    fireEvent.click(trigger);
    const item = await screen.findByRole("menuitem", { name: /summarizing/i });
    expect(item).toHaveAttribute("aria-disabled", "true");
  });

  it("surfaces an enhance failure through the global error notice", async () => {
    vi.mocked(ipc.listSessions).mockResolvedValue(one);
    render(<App />);
    await screen.findByRole("button", { name: /weekly sync/i });

    emit("enhance-progress", {
      path: "/tmp/sync.md",
      phase: "error",
      provider: "claude",
      message: "Claude Code took longer than 5 minutes and was stopped",
    });
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("took longer than 5 minutes");
  });
});
