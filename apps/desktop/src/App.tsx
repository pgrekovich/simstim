import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { Settings } from "lucide-react";
import { Button } from "@/components/ui/button";
import { RecordButton } from "@/components/RecordButton";
import { SessionView } from "@/components/SessionView";
import { Sidebar } from "@/components/Sidebar";
import { SettingsDialog } from "@/components/SettingsDialog";
import {
  getConfig,
  listSessions,
  readSession,
  startRecording,
  stopRecording,
  type AppConfig,
  type ModelInfo,
  type SessionData,
  type SessionErrorPayload,
  type SessionMeta,
  type TranscriptInterimPayload,
} from "@/lib/ipc";
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
 * Seed data for `SessionView` while viewing the active recording. A freshly
 * started recording always begins as an untitled, empty-notes file (there is
 * no prior content to load), and `SessionView` only reads `initialData` once
 * on mount -- so this constant is only ever the starting point for title and
 * notes; the live transcript comes from `lines`/interim state instead. Its
 * `transcript` and `date` fields are unused while `isActive` is true.
 */
const ACTIVE_SESSION_PLACEHOLDER: SessionData = {
  title: "Untitled",
  date: "",
  notes: "",
  transcript: "",
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
  const [sessionData, setSessionData] = useState<SessionData | null>(null);
  const [sessionDataError, setSessionDataError] = useState<string | null>(null);

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
    setLines([]);
    setInterimMe("");
    setInterimThem("");
    try {
      const path = await startRecording();
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

  const sessionActive = status.kind === "loading-model" || status.kind === "recording";
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
      <Sidebar sessions={sessions} selectedPath={selectedPath} onSelect={setSelectedPath} />

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
              key={selectedPath}
              path={selectedPath}
              isActive
              initialData={ACTIVE_SESSION_PLACEHOLDER}
              liveLines={lines}
              interimMe={interimMe}
              interimThem={interimThem}
              onTitleChange={refreshSessions}
            />
          ) : sessionDataError ? (
            <p className="text-sm text-destructive">{sessionDataError}</p>
          ) : sessionData === null ? (
            <p className="text-sm text-muted-foreground">loading...</p>
          ) : (
            <SessionView
              key={selectedPath}
              path={selectedPath}
              isActive={false}
              initialData={sessionData}
              liveLines={lines}
              interimMe={interimMe}
              interimThem={interimThem}
              onTitleChange={refreshSessions}
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
