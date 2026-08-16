import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { Settings } from "lucide-react";
import { Button } from "@/components/ui/button";
import { RecordButton } from "@/components/RecordButton";
import { SessionView, type SessionViewHandle } from "@/components/SessionView";
import { Sidebar } from "@/components/Sidebar";
import { SettingsDialog } from "@/components/SettingsDialog";
import {
  createSession,
  getConfig,
  listSessions,
  readSession,
  startRecording,
  stopRecording,
  type AppConfig,
  type ModelInfo,
  type RetranscribeProgress,
  type SessionData,
  type SessionErrorPayload,
  type SessionMeta,
  type TranscriptInterimPayload,
} from "@/lib/ipc";
import { describeRetranscribeProgress, type RetranscribeJob } from "@/lib/retranscribeStatus";
import { initTheme } from "@/lib/theme";
import { truncateMiddle } from "@/lib/utils";

/**
 * The engine (VAD + ASR) loads in a worker thread after `start_recording`
 * resolves, so there is no explicit "model ready" signal from the backend.
 * We show "loading model" until the first transcript line arrives (which
 * only happens once the engine is up and has actually transcribed
 * something) or until this fallback fires, whichever comes first. If the
 * call is silent for the whole window, this heuristic still flips the
 * status to "recording" so the UI doesn't get stuck saying "loading model"
 * forever.
 */
const MODEL_LOAD_FALLBACK_MS = 15_000;

/**
 * Starting title/notes for a freshly started recording that has no session
 * selected to continue (there is no prior content to load). `handleStart`
 * seeds `activeTitle`/`activeNotes` from here only when starting brand-new;
 * when Record is pressed with a session selected, it instead reads that
 * session's own title/notes/transcript and seeds from those (see
 * `handleStart`'s "continuePath" handling) -- that's the behavior fix:
 * recording continues the open note rather than always creating a new one.
 * Either way, `activeTitle`/`activeNotes` are kept live thereafter via
 * `SessionView`'s `onActiveEdit` callback, so a `SessionView` remount for the
 * active session (switching away and back while still recording) always
 * seeds from the last actually-typed title/notes instead of resetting to a
 * placeholder. See `SessionView`'s class doc comment for why a fixed
 * placeholder here was the original bug. The `transcript` and `date` fields
 * are unused while `isActive` is true (the live transcript comes from
 * `lines`/interim state instead), so they stay fixed.
 */
const ACTIVE_SESSION_PLACEHOLDER: Pick<SessionData, "title" | "notes"> = {
  title: "Untitled",
  notes: "",
};

type Status =
  | { kind: "idle" }
  | { kind: "loading-model" }
  | { kind: "recording" }
  | { kind: "saved"; path: string }
  | { kind: "error"; message: string };

function App() {
  const [status, setStatus] = useState<Status>({ kind: "idle" });
  const [lines, setLines] = useState<string[]>([]);
  const [interimMe, setInterimMe] = useState("");
  const [interimThem, setInterimThem] = useState("");
  const [recordingStartedAt, setRecordingStartedAt] = useState<number | null>(null);
  const [modelReady, setModelReady] = useState(true);
  const [busy, setBusy] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const loadingFallbackRef = useRef<number | null>(null);

  const [sessions, setSessions] = useState<SessionMeta[]>([]);
  const [selectedPath, setSelectedPath] = useState<string | null>(null);
  // The path returned by `start_recording`, while a session is actively
  // being recorded (cleared on stop). Used to decide whether the content
  // pane should show the live in-memory `lines` instead of reading the file
  // from disk: see the `viewingActiveSession` comment below.
  const [activePath, setActivePath] = useState<string | null>(null);
  // The active session's title/notes, lifted out of SessionView so they
  // survive a remount (switching away from the active session and back):
  // see ACTIVE_SESSION_PLACEHOLDER and SessionView's `onActiveEdit` prop.
  // Reset to the placeholder in handleStart, kept live by onActiveEdit.
  const [activeTitle, setActiveTitle] = useState(ACTIVE_SESSION_PLACEHOLDER.title);
  const [activeNotes, setActiveNotes] = useState(ACTIVE_SESSION_PLACEHOLDER.notes);
  const [sessionData, setSessionData] = useState<SessionData | null>(null);
  const [sessionDataError, setSessionDataError] = useState<string | null>(null);
  // See `RetranscribeJob`'s doc comment: survives a SessionView remount,
  // unlike a local boolean would.
  const [retranscribingJob, setRetranscribingJob] = useState<RetranscribeJob>(null);
  // Attached to WHICHEVER SessionView is currently mounted -- active or
  // inactive (see the JSX below, both branches carry this ref). handleStop
  // reads this to flush any pending title/notes edit through
  // update_active_session BEFORE calling stop_recording -- see SessionView's
  // class doc comment for why that ordering, not the instance's own unmount
  // cleanup, is what actually has to do this flush.
  //
  // handleStart reads it too (Critical fix-review finding): pressing Record
  // on a selected, still-inactive session with an unflushed, still-debouncing
  // edit used to lose that edit outright. The instance is inactive at that
  // point, so its pending edit still routes through save_session -- but
  // start_recording flips AppState's active path synchronously, so if the
  // debounce fired even slightly late, save_session would reject it (this
  // path just became the active recording) with nothing left to retry.
  // Flushing here, before start_recording is ever called, closes that
  // window entirely instead of narrowing it.
  const activeSessionViewRef = useRef<SessionViewHandle>(null);

  // The settings dialog owns fetching config/models (on mount, on open, and
  // on download completion) and reports back through this callback, so the
  // "model not downloaded" gate here updates without an app restart.
  function handleModelsChanged(config: AppConfig, models: ModelInfo[]) {
    const active = models.find((m) => m.id === config.active_model);
    setModelReady(Boolean(active?.downloaded));
  }

  // index.html already applied a pre-paint guess (system preference) before
  // React mounted; reconcile it against the persisted config's theme choice
  // as soon as it loads. Index.html's own inline script covers the gap
  // before this resolves, so a brief "system" guess for a "light"/"dark"
  // override user is the only acceptable flash (per Global Constraints).
  useEffect(() => {
    getConfig()
      .then((config) => initTheme(config.theme))
      .catch(() => {
        // Defensive only: `get_config` doesn't actually reject on a missing
        // config file (`Config::load` falls back to `Config::default()`), so
        // this is just guarding against an unexpected IPC failure. If it
        // ever does fire, the pre-paint script's system guess stands.
      });
  }, []);

  const refreshSessions = useCallback(async () => {
    try {
      const fresh = await listSessions();
      setSessions(fresh);
      return fresh;
    } catch {
      // Leave the previous list in place rather than blanking the sidebar
      // on a transient scan failure.
      return null;
    }
  }, []);

  // Refresh triggers (Global Constraints / Task 5 brief): mount, window
  // focus, recording start/stop (handled in handleStart/handleStop below),
  // and session-error (handled in the listener below). Window `focus` alone
  // covers "the user alt-tabbed/switched back to the app": `focus` and
  // `visibilitychange` both fire on that same event in a single-window
  // desktop app, so listening to both would just double the refetch --
  // only `focus` is wired up.
  useEffect(() => {
    refreshSessions();
    function handleFocus() {
      refreshSessions();
    }
    window.addEventListener("focus", handleFocus);
    return () => {
      window.removeEventListener("focus", handleFocus);
    };
  }, [refreshSessions]);

  useEffect(() => {
    const unlisten = listen<string>("transcript-line", (event) => {
      setLines((prev) => [...prev, event.payload]);
      setStatus((prev) => (prev.kind === "loading-model" ? { kind: "recording" } : prev));
      if (loadingFallbackRef.current !== null) {
        window.clearTimeout(loadingFallbackRef.current);
        loadingFallbackRef.current = null;
      }
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  // Live (not-yet-finalized) per-channel drafts for the session currently
  // being recorded. An empty `text` in the payload means "clear this
  // channel's draft" (Global Constraints): the same setter handles both.
  useEffect(() => {
    const unlisten = listen<TranscriptInterimPayload>("transcript-interim", (event) => {
      const { channel, text } = event.payload;
      if (channel === "me") {
        setInterimMe(text);
      } else {
        setInterimThem(text);
      }
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  // Worker-side errors (engine/VAD init, save failures) surfaced instead of
  // living only in stderr: reflect them in the status line, same as a
  // failed start/stop call, and refresh the sidebar (a save failure can mean
  // the on-disk session list changed underneath us).
  useEffect(() => {
    const unlisten = listen<SessionErrorPayload>("session-error", (event) => {
      setStatus({ kind: "error", message: event.payload.message });
      refreshSessions();
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, [refreshSessions]);

  // Owns `retranscribingJob` (see its doc comment): the backend runs at
  // most one re-transcribe job at a time across ALL sessions
  // (`retranscribe.rs`'s `RetranscribeState`), so `path` alone identifies
  // it. A non-terminal phase sets/updates the job for its `path`; "done" or
  // "error" clears it, but only if it still matches (defensive -- in
  // practice there's only ever one job, so a mismatch shouldn't happen).
  useEffect(() => {
    const unlisten = listen<RetranscribeProgress>("retranscribe-progress", (event) => {
      const { path, phase } = event.payload;
      if (phase === "done" || phase === "error") {
        setRetranscribingJob((prev) => (prev?.path === path ? null : prev));
        return;
      }
      setRetranscribingJob({ path, text: describeRetranscribeProgress(event.payload) });
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  // The sidebar's one-time refresh at `start_recording`'s resolution (in
  // handleStart below) fires before the session file necessarily exists on
  // disk: the engine/VAD load on the worker thread first, and only once
  // that's done does it do its first save (see session.rs's module docs) --
  // seconds away, with nothing else to prompt a re-scan in the meantime. So
  // a recording in progress can be invisible in the sidebar for that whole
  // stretch. `status.kind` flips "loading-model" -> "recording" right as
  // that gap closes (either the first transcript line arriving, or --
  // silence for the whole window -- the `MODEL_LOAD_FALLBACK_MS` fallback
  // above), so refreshing exactly on that transition, watched here rather
  // than duplicated in both of those call sites, covers both triggers with
  // one `refreshSessions()` per recording (the transition only ever happens
  // once: after "recording", status only moves on to "saved"/"error", not
  // back to "loading-model", until a whole new handleStart).
  const prevStatusKindRef = useRef(status.kind);
  useEffect(() => {
    if (prevStatusKindRef.current === "loading-model" && status.kind === "recording") {
      refreshSessions();
    }
    prevStatusKindRef.current = status.kind;
  }, [status.kind, refreshSessions]);

  // Loads the selected session for SessionView. The one exception is the
  // session currently being recorded (`activePath`): its file is being
  // written by the worker in the background and may lag or race a direct
  // read, so for that one session the live in-memory `lines` (from the
  // transcript-line listener above) are shown instead of a `read_session`
  // dump -- see `viewingActiveSession` below.
  //
  // `sessionData`/`sessionDataError` are cleared unconditionally at the top,
  // before the early return and before kicking off the fetch: switching
  // straight from one past session to another still changes `selectedPath`
  // (and both are non-null, neither is `activePath`), so without this the
  // previous session's title/notes/transcript would stay on screen -- and
  // look like B's data -- until `readSession(selectedPath)` for the new
  // path resolves. Resetting here means the pane shows the "loading..."
  // state for that gap instead. The `cancelled` guard still protects
  // against an even older fetch resolving after a newer one starts.
  useEffect(() => {
    setSessionData(null);
    setSessionDataError(null);
    if (selectedPath === null || selectedPath === activePath) {
      return;
    }
    let cancelled = false;
    readSession(selectedPath)
      .then((data) => {
        if (!cancelled) setSessionData(data);
      })
      .catch((err) => {
        if (!cancelled) setSessionDataError(String(err));
      });
    return () => {
      cancelled = true;
    };
  }, [selectedPath, activePath]);

  function clearLoadingFallback() {
    if (loadingFallbackRef.current !== null) {
      window.clearTimeout(loadingFallbackRef.current);
      loadingFallbackRef.current = null;
    }
  }

  async function handleStart() {
    if (busy) return;
    setBusy(true);
    setInterimMe("");
    setInterimThem("");
    // Record into the currently selected session if there is one, else start
    // a fresh one (Global Constraints / behavior fix: Record must continue
    // the open note, not always create a new one). Read the existing
    // session once, right here at start, so the live view can seed its
    // transcript/title/notes from what's actually on disk instead of the
    // "Untitled"/empty placeholder -- new lines from this recording are
    // appended after it by the `transcript-line` listener above, never
    // re-rendering what's already seeded.
    const continuePath = selectedPath;
    try {
      // Flush any pending, still-debouncing title/notes edit on the
      // currently mounted SessionView (whichever session is selected, still
      // inactive at this point) before doing anything else -- see
      // `activeSessionViewRef`'s doc comment for the lost-edit race this
      // closes. A no-op (resolves immediately) if there's nothing pending or
      // no SessionView mounted at all (nothing selected).
      await activeSessionViewRef.current?.flushPendingEdits();

      let seedTitle = ACTIVE_SESSION_PLACEHOLDER.title;
      let seedNotes = ACTIVE_SESSION_PLACEHOLDER.notes;
      let seedLines: string[] = [];
      if (continuePath !== null) {
        const existing = await readSession(continuePath);
        seedTitle = existing.title;
        seedNotes = existing.notes;
        seedLines = existing.transcript.split("\n").filter((line) => line.length > 0);
      }
      setLines(seedLines);
      setActiveTitle(seedTitle);
      setActiveNotes(seedNotes);
      const path = await startRecording(continuePath);
      setRecordingStartedAt(Date.now());
      setActivePath(path);
      setSelectedPath(path);
      setStatus({ kind: "loading-model" });
      clearLoadingFallback();
      loadingFallbackRef.current = window.setTimeout(() => {
        setStatus((prev) => (prev.kind === "loading-model" ? { kind: "recording" } : prev));
        loadingFallbackRef.current = null;
      }, MODEL_LOAD_FALLBACK_MS);
      refreshSessions();
    } catch (err) {
      setStatus({ kind: "error", message: String(err) });
    } finally {
      setBusy(false);
    }
  }

  async function handleStop() {
    if (busy) return;
    setBusy(true);
    clearLoadingFallback();
    try {
      // Must happen before stop_recording: once the worker is torn down,
      // update_active_session rejects with "not recording", and by the time
      // this component's own unmount cleanup runs (after sessionData resets
      // to null and the active-session JSX branch disappears) it's too
      // late. See SessionView's class doc comment.
      await activeSessionViewRef.current?.flushPendingEdits();
      const path = await stopRecording();
      setRecordingStartedAt(null);
      setActivePath(null);
      setInterimMe("");
      setInterimThem("");
      setStatus({ kind: "saved", path });
      refreshSessions();
    } catch (err) {
      setStatus({ kind: "error", message: String(err) });
    } finally {
      setBusy(false);
    }
  }

  // Creates a new, empty note the user can type into right away -- no
  // recording involved. Deliberately leaves `activePath`/recording state
  // untouched: this is just another (inactive) session file that happens to
  // start selected, not a substitute for `handleStart`. Recording always
  // creates its own file via `start_recording`, independent of this one.
  async function handleNewSession() {
    if (busy) return;
    setBusy(true);
    try {
      const path = await createSession();
      await refreshSessions();
      setSelectedPath(path);
    } catch (err) {
      setStatus({ kind: "error", message: String(err) });
    } finally {
      setBusy(false);
    }
  }

  // Whether a recording is in progress, independent of `status`: derived
  // from `activePath` (set for as long as the worker is running, cleared
  // only by a successful stop) rather than `status.kind`. A session-error or
  // a failed save mid-recording flips `status` to `{ kind: "error" }` without
  // the worker actually stopping -- `status` is display-only. If the record
  // control's mode (and its `onClick` target below) were derived from
  // `status.kind` instead, that error would silently swap the button back to
  // "Record" while still genuinely recording: clicking it would run
  // handleStart against an already-running session (wiping the in-memory
  // `lines` and getting an "already recording" rejection back) with no way
  // left in the UI to actually stop.
  const sessionActive = activePath !== null;
  const viewingActiveSession = selectedPath !== null && selectedPath === activePath;
  // The model-missing hint only replaces the status text while there is no
  // more specific status to show (idle, not recording): once a real status
  // exists ("saved to ...", an error, or an active recording) it takes
  // priority, so a stale model-download reminder never masks it.
  const showModelHint = !sessionActive && !modelReady && status.kind === "idle";
  const statusText = showModelHint
    ? "model not downloaded"
    : status.kind === "loading-model"
      ? "loading model"
      : status.kind === "recording"
        ? "recording"
        : status.kind === "saved"
          ? `saved to ${truncateMiddle(status.path)}`
          : status.kind === "error"
            ? status.message
            : "";

  return (
    <div className="flex h-screen">
      <Sidebar
        sessions={sessions}
        selectedPath={selectedPath}
        activePath={activePath}
        onSelect={setSelectedPath}
        onNewSession={handleNewSession}
      />

      <div className="flex min-w-0 flex-1 flex-col">
        <header className="flex h-12 shrink-0 items-center gap-3 border-b px-4">
          <RecordButton
            size="compact"
            recording={sessionActive}
            disabled={busy || (!sessionActive && !modelReady)}
            startedAt={recordingStartedAt}
            onClick={sessionActive ? handleStop : handleStart}
          />

          <span
            className={
              status.kind === "error"
                ? "min-w-0 flex-1 truncate text-sm text-destructive"
                : "min-w-0 flex-1 truncate text-sm text-muted-foreground"
            }
          >
            {statusText}
          </span>

          {showModelHint && (
            <Button variant="outline" size="sm" onClick={() => setSettingsOpen(true)}>
              Open settings
            </Button>
          )}

          <Button
            variant="ghost"
            size="icon"
            aria-label="Settings"
            onClick={() => setSettingsOpen(true)}
          >
            <Settings />
          </Button>
        </header>

        <div className="flex min-h-0 flex-1 flex-col p-6">
          {selectedPath === null ? (
            <p className="flex h-full items-center justify-center text-center text-sm text-muted-foreground">
              select a session or start recording
            </p>
          ) : viewingActiveSession ? (
            <SessionView
              ref={activeSessionViewRef}
              key={selectedPath}
              path={selectedPath}
              isActive
              initialData={{
                title: activeTitle,
                notes: activeNotes,
                date: "",
                transcript: "",
                audio_path: null,
              }}
              liveLines={lines}
              interimMe={interimMe}
              interimThem={interimThem}
              retranscribingJob={retranscribingJob}
              onTitleChange={refreshSessions}
              onSaveError={(message) => setStatus({ kind: "error", message })}
              onActiveEdit={(title, notes) => {
                setActiveTitle(title);
                setActiveNotes(notes);
              }}
            />
          ) : sessionDataError ? (
            <p className="text-sm text-destructive">{sessionDataError}</p>
          ) : sessionData === null ? (
            <p className="text-sm text-muted-foreground">loading...</p>
          ) : (
            <SessionView
              ref={activeSessionViewRef}
              key={selectedPath}
              path={selectedPath}
              isActive={false}
              initialData={sessionData}
              liveLines={lines}
              interimMe={interimMe}
              interimThem={interimThem}
              retranscribingJob={retranscribingJob}
              onTitleChange={refreshSessions}
              onSaveError={(message) => setStatus({ kind: "error", message })}
            />
          )}
        </div>
      </div>

      <SettingsDialog
        open={settingsOpen}
        onOpenChange={setSettingsOpen}
        onModelsChanged={handleModelsChanged}
      />
    </div>
  );
}

export default App;
