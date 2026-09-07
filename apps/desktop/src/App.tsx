import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { ask } from "@tauri-apps/plugin-dialog";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import {
  CircleAlert,
  ClipboardCopy,
  Ellipsis,
  FileText,
  Folder,
  MessageCircleQuestion,
  PanelLeftOpen,
  Search,
  Settings,
  Sparkles,
  SquarePen,
  Trash2,
  X,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { RecordButton } from "@/components/RecordButton";
import { ResizeHandle } from "@/components/ResizeHandle";
import { EmptyState } from "@/components/EmptyState";
import { HearingIndicator } from "@/components/HearingIndicator";
import { AskCallDrawer } from "@/components/AskCallDrawer";
import { AskNotesView } from "@/components/AskNotesView";
import { type ChatAskRequest } from "@/components/Chat";
import { GlobalSearch, type SpokenSearchTarget } from "@/components/GlobalSearch";
import {
  SessionView,
  type SessionViewHandle,
  type TranscriptFindRequest,
} from "@/components/SessionView";
import { Sidebar } from "@/components/Sidebar";
import { SettingsDialog } from "@/components/SettingsDialog";
import { Onboarding } from "@/components/Onboarding";
import {
  createSession,
  deleteSession,
  enhanceSession,
  getConfig,
  listSessions,
  readSession,
  recordingActivity,
  showMainWindow,
  startRecording,
  stopRecording,
  type AppConfig,
  type CallDetectedPayload,
  type CallEndedPayload,
  type ChatSource,
  type EnhanceProgress,
  type HearingStatus,
  type ModelInfo,
  type RetranscribeProgress,
  type SessionData,
  type SessionErrorPayload,
  type SessionMeta,
  type TranscriptInterimPayload,
} from "@/lib/ipc";
import {
  describeRetranscribeProgress,
  isTranscriptUpdatedButAudioDeleteFailed,
  type RetranscribeJob,
} from "@/lib/retranscribeStatus";
import { initTheme } from "@/lib/theme";

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
const SIDEBAR_COLLAPSED_KEY = "simstim.sidebar-collapsed";

function initialSidebarCollapsed(): boolean {
  try {
    return window.localStorage.getItem(SIDEBAR_COLLAPSED_KEY) === "true";
  } catch {
    return false;
  }
}

/**
 * Resizable pane widths (stage 4.2 polish, owner request): the sidebar and
 * the Ask-this-call drawer are dragged at their shared border, persisted per
 * pane, and double-click restores the design default. Bounds keep either
 * pane from being dragged into uselessness or swallowing the note.
 */
export interface PaneWidthSpec {
  key: string;
  default: number;
  min: number;
  max: number;
}

export const SIDEBAR_WIDTH: PaneWidthSpec = {
  key: "simstim.sidebar-width",
  // 280: the owner tuned this by eye from the original 224 (2026-09-04).
  // Titles get the extra room. The default WINDOW width (tauri.conf.json)
  // must stay >= MAIN_PANE_MIN + this, or the mount-time reclamp squeezes
  // the sidebar back toward its minimum and the default never actually
  // shows.
  default: 280,
  min: 180,
  max: 400,
};

export const ASK_CALL_WIDTH: PaneWidthSpec = {
  key: "simstim.ask-call-width",
  default: 300,
  min: 260,
  max: 520,
};

/**
 * The note pane must stay usable no matter how the side panes are dragged --
 * and "usable" is set by its TOOLBAR, not its text column: three tabs, the
 * Enhance control, Record (wider while recording), the ask and overflow
 * icons and their gaps need ~640px before anything clips (owner report: the
 * tab list was getting cut under Record with both panes open).
 */
const MAIN_PANE_MIN = 640;

/**
 * `maxOverride` tightens (never widens) the spec's own ceiling: drags pass
 * the space actually available in the window so a side pane can't crush the
 * note below [`MAIN_PANE_MIN`]; the minimum still wins over it, so a tiny
 * window degrades to overlap rather than an unusably thin pane.
 */
export function clampPaneWidth(
  spec: PaneWidthSpec,
  width: number,
  maxOverride?: number,
): number {
  if (!Number.isFinite(width)) return spec.default;
  const max = Math.max(spec.min, Math.min(spec.max, maxOverride ?? spec.max));
  return Math.min(max, Math.max(spec.min, Math.round(width)));
}

function initialPaneWidth(spec: PaneWidthSpec): number {
  try {
    const stored = window.localStorage.getItem(spec.key);
    if (stored === null) return spec.default;
    return clampPaneWidth(spec, Number(stored));
  } catch {
    return spec.default;
  }
}

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

function detectedCallTitle(appName: string, now = new Date()): string {
  const time = now.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  return `${appName} call ${time}`;
}

type Status =
  | { kind: "idle" }
  | { kind: "loading-model" }
  | { kind: "recording" }
  | { kind: "saving" }
  | { kind: "saved"; path: string }
  | { kind: "error"; message: string };

function App() {
  const [status, setStatus] = useState<Status>({ kind: "idle" });
  const [lines, setLines] = useState<string[]>([]);
  const [interimMe, setInterimMe] = useState("");
  const [interimThem, setInterimThem] = useState("");
  const [recordingStartedAt, setRecordingStartedAt] = useState<number | null>(null);
  // Who is being heard right now, polled once a second while recording
  // (see HearingIndicator); `null` between recordings.
  const [hearing, setHearing] = useState<HearingStatus | null>(null);
  const [modelReady, setModelReady] = useState(true);
  const [appConfig, setAppConfig] = useState<AppConfig | null>(null);
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [busy, setBusy] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [searchOpen, setSearchOpen] = useState(false);
  // Ask your notes (stage 4.2): when open, the folder-wide chat owns the
  // main pane (the selection stays put underneath and comes back on close).
  const [askNotesOpen, setAskNotesOpen] = useState(false);
  // A question handed to Ask your notes from the ⌘K palette's bottom row.
  const [askNotesRequest, setAskNotesRequest] = useState<ChatAskRequest | null>(null);
  const askNotesRequestIdRef = useRef(0);
  // Ask this call (stage 4.2): the right-hand drawer over the open note.
  const [askCallOpen, setAskCallOpen] = useState(false);
  // A question handed to the drawer from the ⌘K palette's "Ask this call" row.
  const [askCallRequest, setAskCallRequest] = useState<ChatAskRequest | null>(null);
  const askCallRequestIdRef = useRef(0);
  // A clicked citation chip's jump target, keyed by path so it only reaches
  // the SessionView for that note (same recipe as transcriptFindRequest).
  const [transcriptJumpRequest, setTranscriptJumpRequest] = useState<
    { id: number; timestamp: string; path: string } | null
  >(null);
  const transcriptJumpRequestIdRef = useRef(0);
  const [transcriptFindRequest, setTranscriptFindRequest] = useState<
    (TranscriptFindRequest & { path: string }) | null
  >(null);
  const transcriptFindRequestIdRef = useRef(0);
  const [sidebarCollapsed, setSidebarCollapsed] = useState(initialSidebarCollapsed);
  const [sidebarWidth, setSidebarWidth] = useState(() => initialPaneWidth(SIDEBAR_WIDTH));
  const [askCallWidth, setAskCallWidth] = useState(() => initialPaneWidth(ASK_CALL_WIDTH));
  // Width of the pane being dragged, snapshotted at drag start; each drag
  // event applies the pointer's total travel to this base.
  const dragBaseWidthRef = useRef(0);
  const [autoStopAfterCall, setAutoStopAfterCall] = useState(false);
  const loadingFallbackRef = useRef<number | null>(null);
  const callLifecycleRef = useRef({
    activePath: null as string | null,
    autoStopAfterCall: false,
  });
  const handleStopRef = useRef<() => Promise<void>>(async () => {});
  const bannerStartRef = useRef<(payload: CallDetectedPayload) => void>(() => {});
  const menuBarToggleRef = useRef<() => void>(() => {});
  const menuBarNewNoteRef = useRef<() => void>(() => {});
  const onboardingCompleteRef = useRef(false);
  // State alone cannot reject two stop events delivered in the same React
  // tick. This synchronous guard protects the one backend recording handle.
  const stopInFlightRef = useRef(false);
  // Monotonic id per stop, so a background finalization that finishes late
  // (after a newer stop already owns the status line) reports nothing --
  // see handleStop.
  const stopEpochRef = useRef(0);

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
  // Bumped when a re-transcribe job finishes for the currently selected,
  // inactive session (see the `retranscribe-progress` listener below):
  // included in that SessionView's `key` so it fully remounts and re-seeds
  // from the fresh `sessionData`, since it otherwise only reads
  // `initialData` once, on mount.
  const [reloadTick, setReloadTick] = useState(0);
  // See `RetranscribeJob`'s doc comment: survives a SessionView remount,
  // unlike a local boolean would.
  const [retranscribingJob, setRetranscribingJob] = useState<RetranscribeJob>(null);
  // Which session (if any) an enhance is currently running for -- the same
  // lifted-out-of-SessionView treatment `retranscribingJob` got: the backend
  // runs at most one enhance app-wide (`enhance.rs`'s EnhanceState), a job
  // can outlive the initiating SessionView instance (switching sessions
  // remounts it under a fresh key), and the sidebar needs it for every
  // session, not just the open one. Owned by the `enhance-progress`
  // listener below. `message` carries the local provider's map/reduce step
  // ("Summarizing part 2 of 7...") so a long run shows real progress.
  const [enhancing, setEnhancing] = useState<{ path: string; message: string | null } | null>(
    null,
  );
  const enhancingPath = enhancing?.path ?? null;
  // Paths with a MANUALLY-initiated re-transcribe currently in flight
  // (review fix, Task 2): SessionView calls `handleManualRetranscribeStart`
  // the instant its own click handler starts the job, before awaiting
  // `retranscribeSession` -- see SessionView's `handleRetranscribe`. The
  // `retranscribe-progress` listener below consults this on the terminal
  // "error" phase to tell a manual failure (already surfaced inline by that
  // SessionView instance's own `retranscribeError`, set from the rejected
  // promise) apart from an AUTO-triggered job's failure, which has no other
  // UI anywhere and must be reported through the header instead. A ref, not
  // state: only ever read inside the listener's closure, and adding/removing
  // a path shouldn't itself trigger a re-render.
  const manualRetranscribePathsRef = useRef<Set<string>>(new Set());
  function handleManualRetranscribeStart(path: string) {
    manualRetranscribePathsRef.current.add(path);
  }
  // Final-review Important finding: an auto job's terminal "error" can land
  // in the `retranscribe-progress` listener below while `status.kind` is
  // STILL "recording" -- e.g. session A's auto-triggered job (queued behind
  // another one) finally reports failure right as a brand-new recording B
  // has started. The listener already skips clobbering "recording" with that
  // failure (correct: don't stomp on a genuinely live recording's status),
  // but the old code just discarded the message there, permanently -- if
  // nothing else happened to trigger another background job's error while
  // status was NOT "recording", the owner would simply never learn about it.
  // Stashing it here (one slot is enough: only one background job is ever
  // running at a time, see `retranscribe.rs`'s module docs) and surfacing it
  // the instant `status.kind` leaves "recording" (the effect below, which
  // already tracks that same transition) closes that window instead of just
  // narrowing it.
  const suppressedAutoErrorRef = useRef<string | null>(null);
  // Third review round: `retranscribe_session` rejects up front, before
  // emitting anything, on several validation paths (not-active/finalizing,
  // missing audio, retired/missing model, already-running/already-queued --
  // see `retranscribe.rs`'s doc comments), so the terminal-event cleanup in
  // the listener below never runs for those. Without this, a path would stay
  // marked "manual in flight" forever after one of those rejections,
  // permanently suppressing the header for a LATER genuine auto-job failure
  // on the very same session -- exactly the silence this whole fix exists to
  // remove, just deferred. SessionView calls this from its own catch block
  // (see `onRetranscribeEnd`'s doc comment there) whenever a manual attempt
  // ends without ever producing a terminal event. `Set.delete` on an absent
  // path is a no-op, so this is also safe to call for a rejection that
  // arrives AFTER a terminal event already cleared the path.
  function handleManualRetranscribeEnd(path: string) {
    manualRetranscribePathsRef.current.delete(path);
  }
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

  // Core Audio events are independent of React renders. Keep one listener
  // for the app lifetime and let it read the latest lifecycle state through
  // this ref, avoiding a brief unsubscribe/resubscribe gap when recording
  // starts or the auto-stop setting changes. Written from an effect, not
  // during render, so only committed state ever reaches the listener.
  useEffect(() => {
    callLifecycleRef.current = {
      activePath,
      autoStopAfterCall,
    };
  }, [activePath, autoStopAfterCall]);

  useEffect(() => {
    onboardingCompleteRef.current = appConfig?.onboarding_complete === true;
  }, [appConfig?.onboarding_complete]);

  // The settings dialog owns fetching config/models (on mount, on open, and
  // on download completion) and reports back through this callback, so the
  // "model not downloaded" gate here updates without an app restart.
  function handleModelsChanged(config: AppConfig, models: ModelInfo[]) {
    const active = models.find((m) => m.id === config.active_model);
    setAppConfig(config);
    setModels(models);
    setModelReady(Boolean(active?.downloaded));
    setAutoStopAfterCall(config.auto_stop_after_call);
  }

  // index.html already applied a pre-paint guess (system preference) before
  // React mounted; reconcile it against the persisted config's theme choice
  // as soon as it loads. Index.html's own inline script covers the gap
  // before this resolves, so a brief "system" guess for a "light"/"dark"
  // override user is the only acceptable flash (per Global Constraints).
  useEffect(() => {
    getConfig()
      .then((config) => {
        setAppConfig(config);
        initTheme(config.theme);
      })
      .catch(() => {
        // Defensive only: `get_config` doesn't actually reject on a missing
        // config file (`Config::load` falls back to `Config::default()`), so
        // this is just guarding against an unexpected IPC failure. If it
        // ever does fire, the pre-paint script's system guess stands.
      });
  }, []);

  useEffect(() => {
    try {
      window.localStorage.setItem(SIDEBAR_COLLAPSED_KEY, String(sidebarCollapsed));
    } catch {
      // A blocked localStorage should not make the application unusable.
    }
  }, [sidebarCollapsed]);

  useEffect(() => {
    try {
      window.localStorage.setItem(SIDEBAR_WIDTH.key, String(sidebarWidth));
      window.localStorage.setItem(ASK_CALL_WIDTH.key, String(askCallWidth));
    } catch {
      // Same tolerance as the collapsed flag above.
    }
  }, [sidebarWidth, askCallWidth]);

  // The note pane's MAIN_PANE_MIN invariant must hold through window
  // resizes, restored widths and pane toggles -- not just drags (owner
  // request: the toolbar must never crumple, however the user resizes).
  // The drawer, the secondary pane, gives way first; then the sidebar. At
  // the window's own minWidth (800, tauri.conf.json) the invariant is
  // exactly satisfiable with both panes at their minimums. The setters
  // bail out on unchanged values, so this effect (which depends on what it
  // sets) settles instead of looping.
  const drawerVisible = askCallOpen && selectedPath !== null && !askNotesOpen;
  useEffect(() => {
    function reclamp() {
      const available = window.innerWidth - MAIN_PANE_MIN;
      let drawer = askCallWidth;
      if (drawerVisible) {
        drawer = clampPaneWidth(
          ASK_CALL_WIDTH,
          askCallWidth,
          available - (sidebarCollapsed ? 0 : sidebarWidth),
        );
        setAskCallWidth(drawer);
      }
      if (!sidebarCollapsed) {
        setSidebarWidth(
          clampPaneWidth(SIDEBAR_WIDTH, sidebarWidth, available - (drawerVisible ? drawer : 0)),
        );
      }
    }
    reclamp();
    window.addEventListener("resize", reclamp);
    return () => window.removeEventListener("resize", reclamp);
  }, [sidebarCollapsed, sidebarWidth, drawerVisible, askCallWidth]);

  // The floating banner window (stage 1.1) owns rendering call
  // suggestions; its clicks forward here so recording keeps its single
  // code path. Same latest-value-through-a-ref pattern as handleStopRef:
  // the permanent listener must see current busy/model/recording state.
  useEffect(() => {
    bannerStartRef.current = (payload: CallDetectedPayload) => {
      if (!appConfig?.onboarding_complete || busy || !modelReady || activePath !== null) {
        // Cannot start right now (mid-operation, model missing, or already
        // recording): surface the app instead of silently doing nothing.
        void showMainWindow();
        return;
      }
      void handleStart(payload);
    };
  });

  useEffect(() => {
    const unlisten = listen<CallDetectedPayload>("banner-start", (event) => {
      bannerStartRef.current(event.payload);
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  useEffect(() => {
    const unlisten = listen("banner-stop", () => {
      // handleStop carries its own busy/in-flight guards.
      void handleStopRef.current();
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  // The native menu sends this toggle event. A start always creates a fresh
  // session because there is no visible selection context while another
  // application has focus.
  useEffect(() => {
    menuBarToggleRef.current = () => {
      if (!appConfig?.onboarding_complete) {
        void showMainWindow();
        return;
      }
      if (busy) {
        void showMainWindow();
        return;
      }
      if (activePath !== null) {
        void handleStopRef.current();
        return;
      }
      if (!modelReady) {
        void showMainWindow();
        return;
      }
      void handleStart(undefined, true);
    };
  });

  useEffect(() => {
    const unlisten = listen("menu-bar-toggle-recording", () => {
      menuBarToggleRef.current();
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  useEffect(() => {
    const unlisten = listen("menu-bar-add-note", () => {
      if (!onboardingCompleteRef.current) {
        void showMainWindow();
        return;
      }
      void showMainWindow().then(() => {
        activeSessionViewRef.current?.focusNotes();
      });
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  useEffect(() => {
    menuBarNewNoteRef.current = () => {
      if (!appConfig?.onboarding_complete) {
        void showMainWindow();
        return;
      }
      void showMainWindow().then(() => handleNewSession());
    };
  });

  useEffect(() => {
    const unlisten = listen("menu-bar-new-note", () => {
      menuBarNewNoteRef.current();
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  useEffect(() => {
    const unlisten = listen("menu-bar-open-settings", () => {
      void showMainWindow().then(() => setSettingsOpen(true));
    });
    return () => {
      unlisten.then((f) => f());
    };
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
  // living only in stderr: reflect them in the global error notice, same as a
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
      const { path, phase, message } = event.payload;
      if (phase === "done" || phase === "error") {
        setRetranscribingJob((prev) => (prev?.path === path ? null : prev));
        // Review finding (stage 0.9 Task 2): this listener used to return
        // early on "error" without ever reading `message`, so an
        // auto-triggered job's failure (a stale-job drop on revalidation, a
        // delete-after-retranscribe failure, a model that went missing) had
        // no UI at all -- auto jobs are fire-and-forget (see
        // retranscribe.rs's module docs), so `retranscribe-progress` is the
        // only signal they ever produce. Surface it through the header's
        // existing error status rather than a separate toast.
        //
        // `wasManual` tells a MANUAL click's failure apart from an
        // AUTO-triggered one (second review round: `path !== selectedPath`
        // was the wrong proxy for this -- right after a call stops,
        // `selectedPath` IS that just-stopped session, which is exactly the
        // session an auto job runs for, so that guard silenced precisely the
        // common case this fix exists for). A manual click's own SessionView
        // instance already shows the failure inline (its own
        // `retranscribeError` state, set from the rejected
        // `retranscribeSession` promise -- see SessionView's
        // `handleRetranscribe`, which calls `onRetranscribeStart` right
        // before that awaits), so reporting it again here too would show the
        // same failure twice on screen; every other path (any auto job,
        // viewed or not) has no such inline display, so it's always reported
        // here.
        const wasManual = manualRetranscribePathsRef.current.delete(path);
        // Also skip while the header is showing a live recording: an auto
        // job only ever starts once ITS OWN recording has already stopped
        // (Global Constraints), so in practice this can't race a recording
        // of the same session -- but the header status is global, not
        // per-session, so a background job for some OTHER session finishing
        // while a DIFFERENT recording is genuinely in progress is at least
        // theoretically possible, and clobbering "recording" with an
        // unrelated background failure would be worse than staying quiet.
        if (phase === "error" && !wasManual) {
          const errorMessage = message ?? "re-transcribe failed";
          setStatus((prev) => {
            if (prev.kind === "recording") {
              // Final-review Important finding: don't just drop this on the
              // floor -- stash it so it still reaches the owner once the
              // header stops showing a live recording (the effect below).
              suppressedAutoErrorRef.current = errorMessage;
              return prev;
            }
            return { kind: "error", message: errorMessage };
          });
        }
        // Minor fix-review finding: a SessionView only re-seeds its local
        // `transcript` state from `initialData` on mount (see its class doc
        // comment) -- the instance that actually clicked "Re-transcribe"
        // handles its own re-read once the command's promise resolves, but
        // a DIFFERENT SessionView instance for the same path (the user
        // switched away mid-job and back, which unmounts/remounts it under
        // the same `key={path}`) seeded from a stale on-disk read taken
        // before this job finished, and has no code path of its own that
        // would ever refresh it. Cover that case here: on "done" for the
        // currently selected (and not actively-recording) session, refetch
        // and bump `reloadTick` so the SessionView key changes and it
        // remounts with the fresh transcript. Harmless if this happens to
        // be the initiating instance too -- it just remounts with the same
        // data it already fetched itself.
        //
        // Also refetch on the "transcript updated, but the audio could not
        // be deleted" error (minor fix-review finding, stage 0.9 final
        // review): that specific "error" phase is the one case where the
        // transcript on disk DID change even though the phase isn't "done" --
        // `maybe_delete_audio`'s failure surfaces through the "error" phase
        // (see `retranscribe.rs`), but the whisper pass itself already
        // succeeded and wrote the new transcript before the deletion step
        // ever ran. Without this, the on-screen transcript stays stale after
        // that error the same way it would after a "done" with no refetch.
        //
        // Fix-review finding (wave 2): the fetch must resolve BEFORE
        // `reloadTick` bumps, not just before this callback returns --
        // `setSessionData` and `setReloadTick` used to fire in the same
        // synchronous tick as separate promise continuations, so the remount
        // (driven by `reloadTick`) could happen before `readSession`'s
        // promise had actually resolved, seeding the fresh SessionView
        // instance from whatever `sessionData` still held (possibly still
        // the earlier, stale value) rather than the just-fetched one. Doing
        // this as one async sequence -- await the read, THEN bump the tick
        // -- guarantees the remount always sees the fresh data.
        const shouldRefetchTranscript =
          phase === "done" || (phase === "error" && isTranscriptUpdatedButAudioDeleteFailed(message));
        if (shouldRefetchTranscript && path === selectedPath && path !== activePath) {
          readSession(path)
            .then((data) => {
              setSessionData(data);
              setReloadTick((n) => n + 1);
            })
            .catch((err) => setSessionDataError(String(err)));
        }
        return;
      }
      setRetranscribingJob({ path, text: describeRetranscribeProgress(event.payload) });
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, [selectedPath, activePath]);

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
    // Final-review Important finding: surface a re-transcribe error that
    // landed mid-recording and got stashed instead of shown (see
    // `suppressedAutoErrorRef`'s doc comment and the `retranscribe-progress`
    // listener above) the instant status leaves "recording" for anything
    // else -- this is the same transition this effect already watches for
    // the sidebar-refresh case above, just the opposite direction. Without
    // this, that error would otherwise never reach the owner at all unless
    // some LATER, unrelated background job happened to fail while a
    // recording wasn't in progress.
    if (prevStatusKindRef.current === "recording" && status.kind !== "recording" && suppressedAutoErrorRef.current) {
      const message = suppressedAutoErrorRef.current;
      suppressedAutoErrorRef.current = null;
      setStatus({ kind: "error", message });
    }
    prevStatusKindRef.current = status.kind;
  }, [status.kind, refreshSessions]);

  // The hearing indicator's clock: one cheap IPC read per second while a
  // recording runs, nothing at all otherwise. Errors leave the last reading
  // in place rather than flickering the dots.
  useEffect(() => {
    if (activePath === null) {
      setHearing(null);
      return;
    }
    let cancelled = false;
    const poll = () => {
      recordingActivity()
        .then((status) => {
          if (!cancelled) setHearing(status);
        })
        .catch(() => {});
    };
    poll();
    const timer = window.setInterval(poll, 1000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [activePath]);

  // "saved" is a confirmation, not a state the user lives in: it dissolves
  // back to idle after a beat instead of sitting in the toolbar until some
  // unrelated status happens to replace it. Identity-compared so a newer
  // status set during the wait (a fresh recording, an error) is never
  // clobbered; the cleanup covers that same race from the timer side.
  useEffect(() => {
    if (status.kind !== "saved") return;
    const timer = window.setTimeout(() => {
      setStatus((prev) => (prev === status ? { kind: "idle" } : prev));
    }, 4000);
    return () => window.clearTimeout(timer);
  }, [status]);

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

  // Owns `enhancingPath` (see its doc comment). Every enhance -- manual or
  // auto -- reports through this one event stream (`enhance.rs` emits
  // `running` once the slot is claimed and exactly one terminal
  // `done`/`error`), so this is the single place the UI learns about
  // summarizing work: the toolbar chip and the sidebar spinner derive from
  // `enhancingPath`, failures surface through the global error notice, and
  // a finished job refetches the open note so the new summary (and a
  // model-proposed title) appear without a manual switch-away-and-back.
  useEffect(() => {
    const unlisten = listen<EnhanceProgress>("enhance-progress", (event) => {
      const { path, phase, message } = event.payload;
      if (phase === "running") {
        setEnhancing({ path, message });
        return;
      }
      setEnhancing((prev) => (prev?.path === path ? null : prev));
      if (phase === "error") {
        const errorMessage = message ?? "enhance failed";
        setStatus((prev) => {
          if (prev.kind === "recording") {
            // Same stash-instead-of-drop treatment the re-transcribe
            // listener above gives a background failure landing while the
            // header shows a live recording -- see `suppressedAutoErrorRef`.
            suppressedAutoErrorRef.current = errorMessage;
            return prev;
          }
          return { kind: "error", message: errorMessage };
        });
        return;
      }
      // "done": refetch and remount the open note (same recipe as the
      // re-transcribe listener above -- await the read, THEN bump the tick).
      // Unconditional even for a manually initiated job: the initiating
      // SessionView instance may have been unmounted mid-job, and a remount
      // with the same fresh data is harmless when it wasn't. The sidebar
      // list refreshes too: an enhance can retitle an auto-named note.
      void refreshSessions();
      if (path === selectedPath && path !== activePath) {
        readSession(path)
          .then((data) => {
            setSessionData(data);
            setReloadTick((t) => t + 1);
          })
          .catch(() => {});
      }
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, [selectedPath, activePath, refreshSessions]);

  function clearLoadingFallback() {
    if (loadingFallbackRef.current !== null) {
      window.clearTimeout(loadingFallbackRef.current);
      loadingFallbackRef.current = null;
    }
  }

  async function handleStart(detected?: CallDetectedPayload, forceFresh = false) {
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
    const continuePath = detected || forceFresh ? null : selectedPath;
    try {
      // Flush any pending, still-debouncing title/notes edit on the
      // currently mounted SessionView (whichever session is selected, still
      // inactive at this point) before doing anything else -- see
      // `activeSessionViewRef`'s doc comment for the lost-edit race this
      // closes. A no-op (resolves immediately) if there's nothing pending or
      // no SessionView mounted at all (nothing selected).
      await activeSessionViewRef.current?.flushPendingEdits();

      const detectedTitle = detected ? detectedCallTitle(detected.app_name) : undefined;
      let seedTitle = detectedTitle ?? ACTIVE_SESSION_PLACEHOLDER.title;
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
      const path = detected
        ? await startRecording(
            null,
            detectedTitle,
            detected.bundle_id,
          )
        : await startRecording(continuePath);
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
    if (busy || stopInFlightRef.current) return;
    stopInFlightRef.current = true;
    setBusy(true);
    clearLoadingFallback();
    try {
      // Must happen before stop_recording: once the worker is torn down,
      // update_active_session rejects with "not recording", and by the time
      // this component's own unmount cleanup runs (after sessionData resets
      // to null and the active-session JSX branch disappears) it's too
      // late. See SessionView's class doc comment.
      await activeSessionViewRef.current?.flushPendingEdits();
      const stopPromise = stopRecording();

      // Capture has been handed to the backend's stop path. Reflect that
      // immediately instead of keeping the UI looking live while transcript
      // flush and audio encoding finish on a blocking worker.
      setRecordingStartedAt(null);
      setActivePath(null);
      setInterimMe("");
      setInterimThem("");
      setStatus({ kind: "saving" });
      void refreshSessions();

      // The finalization itself (transcript flush + audio encode, real
      // wall-clock time for a long call) stays in the background: `busy` is
      // released in the finally below, NOT held across this await, so the
      // user can create a new note and start recording the next call while
      // the previous one is still saving. (Holding busy here once cost the
      // owner the start of a real call.) The backend supports this: the
      // recording slot frees immediately on stop, and start_recording
      // rejects only the one file still being finalized. The epoch guards
      // the status line: "saved" from an old stop must not repaint whatever
      // a newer start/stop has put there since.
      const epoch = ++stopEpochRef.current;
      void stopPromise
        .then((path) => {
          if (stopEpochRef.current === epoch) {
            setStatus((prev) => (prev.kind === "saving" ? { kind: "saved", path } : prev));
          }
          void refreshSessions();
        })
        .catch((err) => {
          // A failed save means that call's file may be incomplete; always
          // surface it, even over a newer recording's status. `status` is
          // display-only (see sessionActive's comment), so this cannot
          // unwire the record control.
          setStatus({ kind: "error", message: String(err) });
        });
    } catch (err) {
      setStatus({ kind: "error", message: String(err) });
    } finally {
      stopInFlightRef.current = false;
      setBusy(false);
    }
  }

  // Same latest-value-through-a-ref pattern as callLifecycleRef above: the
  // permanent call-ended listener must call the current handleStop closure,
  // and the assignment happens in an effect, never during render.
  useEffect(() => {
    handleStopRef.current = handleStop;
  });

  useEffect(() => {
    const unlisten = listen<CallEndedPayload>("call-ended", (event) => {
      const lifecycle = callLifecycleRef.current;
      if (event.payload.recording_id !== lifecycle.activePath) {
        return;
      }
      // The floating banner shows the "Call seems over" suggestion; this
      // listener only drives the opt-in automatic stop.
      if (lifecycle.autoStopAfterCall) {
        void handleStopRef.current();
      }
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  // Creates a new, empty note the user can type into right away -- no
  // recording involved. Deliberately leaves `activePath`/recording state
  // untouched: this is just another (inactive) session file that happens to
  // start selected, not a substitute for `handleStart`. Recording always
  // creates its own file via `start_recording`, independent of this one.
  // Active-window shortcuts (stage 0.10): ⌘N new note, ⌘⇧N start recording,
  // ⌘, settings, ⌘K global search, ⌘F transcript find. The listener is
  // attached exactly once; it
  // reads the CURRENT handlers through a ref refreshed on every render, so
  // it never closes over stale state (`busy`, `sessionActive`, ...) the way
  // a once-captured closure would.
  const shortcutsRef = useRef({
    newNote: () => {},
    startRecording: () => {},
    openSettings: () => {},
    openSearch: () => {},
    openTranscriptFind: () => {},
    toggleAskNotes: () => {},
    toggleAskCall: () => {},
  });
  useEffect(() => {
    shortcutsRef.current = {
      newNote: () => {
        if (!appConfig?.onboarding_complete) return;
        setAskNotesOpen(false);
        void handleNewSession();
      },
      startRecording: () => {
        // Same gating as the Record button and the EmptyState row.
        if (!appConfig?.onboarding_complete || busy || sessionActive || !modelReady) return;
        setAskNotesOpen(false);
        void handleStart();
      },
      openSettings: () => setSettingsOpen(true),
      openSearch: () => {
        if (appConfig?.onboarding_complete && !settingsOpen) setSearchOpen(true);
      },
      openTranscriptFind: () => {
        if (!appConfig?.onboarding_complete) return;
        if (settingsOpen) return;
        if (selectedPath === null || askNotesOpen) {
          setSearchOpen(true);
          return;
        }
        setTranscriptFindRequest({
          id: ++transcriptFindRequestIdRef.current,
          path: selectedPath,
          query: "",
        });
      },
      // Ask your notes (⌘L): a toggle -- the selection stays put underneath
      // and comes back on close.
      toggleAskNotes: () => {
        if (!appConfig?.onboarding_complete || settingsOpen) return;
        setAskNotesOpen((open) => !open);
      },
      // Ask this call (⌘J): only meaningful with a note on screen.
      toggleAskCall: () => {
        if (!appConfig?.onboarding_complete || settingsOpen) return;
        if (selectedPath === null || askNotesOpen) return;
        setAskCallOpen((open) => !open);
      },
    };
  });
  useEffect(() => {
    function onKeyDown(e: KeyboardEvent) {
      if (!e.metaKey || e.altKey || e.ctrlKey || e.repeat) return;
      if (document.querySelector('[role="dialog"]')) return;
      const key = e.key.toLowerCase();
      if (key === "n" && e.shiftKey) {
        e.preventDefault();
        shortcutsRef.current.startRecording();
      } else if (key === "n") {
        e.preventDefault();
        shortcutsRef.current.newNote();
      } else if (key === "," && !e.shiftKey) {
        e.preventDefault();
        shortcutsRef.current.openSettings();
      } else if (key === "k" && !e.shiftKey) {
        e.preventDefault();
        shortcutsRef.current.openSearch();
      } else if (key === "f" && !e.shiftKey) {
        e.preventDefault();
        shortcutsRef.current.openTranscriptFind();
      } else if (key === "l" && !e.shiftKey) {
        e.preventDefault();
        shortcutsRef.current.toggleAskNotes();
      } else if (key === "j" && !e.shiftKey) {
        e.preventDefault();
        shortcutsRef.current.toggleAskCall();
      } else if (key === "\\" && !e.shiftKey) {
        e.preventDefault();
        setSidebarCollapsed((collapsed) => !collapsed);
      }
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  async function handleDeleteSession(path: string) {
    const title = sessions.find((s) => s.path === path)?.title ?? "this note";
    // A native confirm, not a silent delete: the files leave the folder.
    // They go to the Trash (recoverable), and the dialog says so.
    const confirmed = await ask(`Move "${title}" and its audio to the Trash?`, {
      title: "Delete note",
      kind: "warning",
      okLabel: "Move to Trash",
      cancelLabel: "Cancel",
    });
    if (!confirmed) return;
    try {
      await deleteSession(path);
      if (selectedPath === path) {
        setSelectedPath(null);
      }
      await refreshSessions();
    } catch (err) {
      setStatus({ kind: "error", message: String(err) });
    }
  }

  // The overflow menu's Enhance / Re-enhance (owner request: while testing
  // summary models the re-run must be one obvious click away, not a quiet
  // provenance-line link). Same recipe as SessionView's handleEnhance:
  // flush pending edits FIRST so the model reads what's on screen, then let
  // the enhance-progress event stream drive every visible consequence (the
  // chip, the sidebar spinner, the toast on failure, the refetch on done).
  async function handleEnhanceSelected() {
    if (selectedPath === null || selectedPath === activePath || enhancingPath !== null) return;
    try {
      await activeSessionViewRef.current?.flushPendingEdits();
      await enhanceSession(selectedPath);
    } catch (err) {
      // Already surfaced through the enhance-progress "error" event; the
      // rejection here would only duplicate that toast.
      console.error("failed to enhance session", err);
    }
  }

  // Chat citation navigation (stage 4.2): opens the cited note and centers
  // its transcript on the cited second. `contextPath` is the note a bare
  // `↩ hh:mm:ss` chip refers to (the drawer's own note); the folder-wide
  // surface never produces one. A cited file that isn't a known session
  // (the folder can be an Obsidian vault with other Markdown) opens in the
  // system editor instead.
  function handleOpenCitation(source: ChatSource, contextPath: string | null) {
    if (source.file === null) {
      if (contextPath !== null && source.timestamp) {
        setTranscriptJumpRequest({
          id: ++transcriptJumpRequestIdRef.current,
          path: contextPath,
          timestamp: source.timestamp,
        });
      }
      return;
    }
    const fileName = source.file;
    const session = sessions.find((s) => s.path.endsWith(`/${fileName}`));
    if (session) {
      setAskNotesOpen(false);
      setSelectedPath(session.path);
      if (source.timestamp) {
        setTranscriptJumpRequest({
          id: ++transcriptJumpRequestIdRef.current,
          path: session.path,
          timestamp: source.timestamp,
        });
      }
      return;
    }
    if (appConfig) {
      void openPath(`${appConfig.output_dir}/${fileName}`);
    }
  }

  // The ⌘K palette's bottom row: the typed query becomes a question on the
  // Ask your notes surface, sent as soon as it mounts.
  function handleAskAllNotes(question: string) {
    setAskNotesOpen(true);
    setAskNotesRequest({ id: ++askNotesRequestIdRef.current, question });
  }

  // The palette's "Ask this call" row: opens the drawer over the open note
  // with the question already sent (one field reaches every scope).
  function handleAskThisCall(question: string) {
    if (selectedPath === null) return;
    setAskNotesOpen(false);
    setAskCallOpen(true);
    setAskCallRequest({ id: ++askCallRequestIdRef.current, question });
  }

  async function handleNewSession() {
    if (!appConfig?.onboarding_complete || busy) return;
    setBusy(true);
    setAskNotesOpen(false);
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
  // Model readiness gets a dedicated Open settings action in the toolbar.
  // Do not repeat the same message in the status slot: with all three tabs
  // visible that duplicate was the first item to crowd the Record action at
  // the default 800 px window width. The empty state still carries the text
  // hint because it has no toolbar action.
  const showModelHint = !sessionActive && !modelReady && status.kind === "idle";
  // The empty-state screen (nothing selected and not recording)
  // hides the topbar entirely. A lone Record button floating over the
  // shortcut menu reads as clutter, so its branch renders a bare drag strip.
  const showTopbar = sessionActive || selectedPath !== null || askNotesOpen;
  // Sidebar rows with work in flight, path -> the lowercase status word
  // shown under the title (design language: quiet mono status). One word
  // for every stage of the pipeline: the person doesn't care whether the
  // note is being transcribed or summarized, only that it isn't done yet.
  const busyPaths: Record<string, string> = {};
  if (enhancingPath !== null) busyPaths[enhancingPath] = "finishing";
  if (retranscribingJob) busyPaths[retranscribingJob.path] = "finishing";
  // The "recording" state has no word of its own: the hearing indicator
  // and the red Stop button carry it.
  const statusText = status.kind === "loading-model"
    ? "starting"
    : status.kind === "saving"
      ? "finishing"
      : status.kind === "saved"
        ? "saved"
        : "";

  if (appConfig === null) {
    return (
      <div data-tauri-drag-region className="flex h-screen items-center justify-center bg-background text-[12px] text-muted-foreground">
        loading...
      </div>
    );
  }

  if (!appConfig.onboarding_complete) {
    return (
      <div className="h-screen bg-background text-foreground">
        <Onboarding
          config={appConfig}
          models={models}
          onOpenSettings={() => setSettingsOpen(true)}
          onComplete={handleModelsChanged}
        />
        <SettingsDialog
          open={settingsOpen}
          onOpenChange={setSettingsOpen}
          onModelsChanged={handleModelsChanged}
        />
      </div>
    );
  }

  return (
    <div className="flex h-screen bg-background text-foreground">
      {!sidebarCollapsed && <Sidebar
        width={sidebarWidth}
        sessions={sessions}
        selectedPath={selectedPath}
        activePath={activePath}
        busyPaths={busyPaths}
        onSelect={(path) => {
          setAskNotesOpen(false);
          setSelectedPath(path);
        }}
        onNewSession={handleNewSession}
        onCollapse={() => setSidebarCollapsed(true)}
        onOpenSearch={() => setSearchOpen(true)}
        onDelete={handleDeleteSession}
        onOpenFile={(path) => void openPath(path)}
        onRevealInFinder={(path) => void revealItemInDir(path)}
      />}
      {!sidebarCollapsed && (
        <ResizeHandle
          label="Resize sidebar"
          onDragStart={() => { dragBaseWidthRef.current = sidebarWidth; }}
          onDrag={(deltaX) =>
            setSidebarWidth(
              clampPaneWidth(
                SIDEBAR_WIDTH,
                dragBaseWidthRef.current + deltaX,
                window.innerWidth - MAIN_PANE_MIN - (askCallOpen ? askCallWidth : 0),
              ),
            )
          }
          onReset={() => setSidebarWidth(SIDEBAR_WIDTH.default)}
        />
      )}

      <div className="relative flex min-w-0 flex-1 flex-col">
        {!showTopbar && !sidebarCollapsed && <div data-tauri-drag-region className="h-11 shrink-0" />}
        {/* Sits at the very window top, level with the traffic lights
            (overlay title bar), and doubles as that region's drag handle --
            buttons inside still receive their own clicks. */}
        {(showTopbar || sidebarCollapsed) && (
        <header
          data-tauri-drag-region
          className={`flex h-14 shrink-0 items-center gap-3 pr-5 ${sidebarCollapsed ? "pl-0" : "pl-5"}`}
        >
          {sidebarCollapsed && (
            <div className="flex w-56 shrink-0 items-center justify-start gap-0.5 pl-[88px] pr-2.5">
              <Button
                variant="ghost"
                size="icon"
                className="size-6 rounded-md"
                aria-label="Show sidebar"
                title="Show sidebar (⌘\\)"
                onClick={() => setSidebarCollapsed(false)}
              >
                <PanelLeftOpen className="size-[15px]" />
              </Button>
              <Button
                variant="ghost"
                size="icon"
                className="size-6 rounded-md"
                aria-label="Search all notes"
                title="Search (⌘K)"
                onClick={() => setSearchOpen(true)}
              >
                <Search className="size-[15px]" />
              </Button>
              <Button
                variant="ghost"
                size="icon"
                className="size-6 rounded-md"
                aria-label="New note"
                title="New note"
                onClick={handleNewSession}
              >
                <SquarePen className="size-[15px]" />
              </Button>
            </div>
          )}

          {/* SessionView portals its Radix tab list here so the switcher is
              part of the 56 px toolbar while its context and mounted tab
              contents remain owned by SessionView. The mask fades the right
              edge instead of hard-clipping it: below MAIN_PANE_MIN (a window
              at its minimum with both panes open) the tabs dissolve rather
              than get cut mid-letter under Record. */}
          <div
            id="topbar-tabs-slot"
            className="flex min-w-0 flex-1 items-center overflow-hidden [mask-image:linear-gradient(to_right,black_calc(100%-28px),transparent)]"
          />
          <div
            id="topbar-transcript-find-slot"
            className="relative z-10 flex min-w-0 shrink-0 items-center"
          />

          {showTopbar && statusText && (
            <span className="min-w-0 max-w-[35%] shrink-0 truncate text-[11px] tabular-nums text-muted-foreground">
              {statusText}
            </span>
          )}

          {showTopbar && sessionActive && status.kind === "recording" && (
            <HearingIndicator status={hearing} />
          )}

          {showTopbar && showModelHint && (
            <Button variant="outline" size="sm" onClick={() => setSettingsOpen(true)}>
              Open settings
            </Button>
          )}

          {showTopbar && <RecordButton
            size="compact"
            recording={sessionActive}
            disabled={busy || (!sessionActive && !modelReady)}
            startedAt={recordingStartedAt}
            onClick={sessionActive ? handleStop : () => void handleStart()}
            // Record continues the OPEN note (Global Constraints); nothing
            // on the button itself says so, so the tooltip does.
            title={
              sessionActive
                ? "Stop recording"
                : selectedPath !== null
                  ? "Record into this note (⌘⇧N)"
                  : "Start recording (⌘⇧N)"
            }
          />}

          {/* Ask this call (stage 4.2, design 8c): the drawer's visible
              entry point, right of Record like the mockup; ⌘J is the
              shortcut, not the only way in. Filled while the drawer is
              open, mirroring the mockup's dark chat square. */}
          {showTopbar && selectedPath !== null && !askNotesOpen && (
            <Button
              variant="ghost"
              size="icon"
              aria-label="Ask this call"
              title="Ask this call (⌘J)"
              className={askCallOpen ? "bg-foreground text-background hover:bg-foreground/90 hover:text-background" : undefined}
              onClick={() => setAskCallOpen((open) => !open)}
            >
              <MessageCircleQuestion className="size-4" />
            </Button>
          )}

          {/* Global actions always live in overflow. Note-specific actions
              only appear while the selected note owns the main pane. */}
          {showTopbar && <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button variant="ghost" size="icon" aria-label="More actions">
                <Ellipsis />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              {selectedPath !== null && !askNotesOpen && (
                <>
                  <DropdownMenuItem
                    disabled={
                      viewingActiveSession ||
                      enhancingPath !== null ||
                      sessionData === null ||
                      sessionData.transcript.trim().length === 0
                    }
                    onSelect={() => void handleEnhanceSelected()}
                  >
                    <Sparkles />
                    {enhancingPath !== null
                      ? "Summarizing..."
                      : sessionData?.summary
                        ? "Summarize again"
                        : "Summarize"}
                  </DropdownMenuItem>
                  <DropdownMenuItem
                    disabled={!sessionData || sessionData.transcript.trim().length === 0}
                    onSelect={() => {
                      if (sessionData) void navigator.clipboard?.writeText(sessionData.transcript.trim()).catch(() => {});
                    }}
                  >
                    <ClipboardCopy />
                    Copy transcript
                  </DropdownMenuItem>
                  <DropdownMenuItem onSelect={() => void openPath(selectedPath)}>
                    <FileText />
                    Open file
                  </DropdownMenuItem>
                  <DropdownMenuItem onSelect={() => void revealItemInDir(selectedPath)}>
                    <Folder />
                    Show in Finder
                  </DropdownMenuItem>
                  <DropdownMenuSeparator />
                  <DropdownMenuItem
                    disabled={viewingActiveSession || selectedPath in busyPaths}
                    className="text-destructive focus:text-destructive [&_svg]:text-destructive"
                    onSelect={() => void handleDeleteSession(selectedPath)}
                  >
                    <Trash2 />
                    Move to Trash
                  </DropdownMenuItem>
                  <DropdownMenuSeparator />
                </>
              )}
              <DropdownMenuItem onSelect={() => setSettingsOpen(true)}>
                <Settings />
                Settings
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>}
        </header>
        )}

        {status.kind === "error" && (
          <div
            role="alert"
            className="absolute right-5 top-16 z-40 flex w-[min(420px,calc(100%-40px))] items-start gap-3 rounded-[9px] border border-destructive/25 bg-card p-3.5 text-card-foreground shadow-[0_10px_30px_rgb(0_0_0/0.16)]"
          >
            <CircleAlert aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-destructive" />
            <div className="min-w-0 flex-1">
              <p className="text-[12px] font-semibold text-destructive">
                Something went wrong
              </p>
              <p className="mt-1 text-[12.5px] leading-[1.45] [overflow-wrap:anywhere]">
                {status.message}
              </p>
            </div>
            <Button
              type="button"
              variant="ghost"
              size="icon-xs"
              className="-mr-1 -mt-1 text-muted-foreground hover:text-foreground"
              aria-label="Dismiss error"
              onClick={() => setStatus(sessionActive ? { kind: "recording" } : { kind: "idle" })}
            >
              <X className="size-3.5" />
            </Button>
          </div>
        )}

        {askNotesOpen ? (
          // Full-bleed on purpose: the surface owns its own insets so the
          // footer input bar can run edge to edge (design 8b).
          <AskNotesView
            sessions={sessions}
            askRequest={askNotesRequest}
            onOpenCitation={(source) => handleOpenCitation(source, null)}
          />
        ) : (
        <div className="flex min-h-0 flex-1 flex-col px-8 pb-7 pt-1">
          {selectedPath === null ? (
            <EmptyState
              onNewNote={handleNewSession}
              onStartRecording={() => void handleStart()}
              onOpenSettings={() => setSettingsOpen(true)}
              recordingDisabled={busy || sessionActive || !modelReady}
              recordingHint={showModelHint ? "speech recognition is still downloading" : undefined}
            />
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
                summary: "",
                summary_model: null,
                people: [],
              }}
              liveLines={lines}
              interimMe={interimMe}
              interimThem={interimThem}
              retranscribingJob={retranscribingJob}
              enhanceRunning={enhancingPath === selectedPath}
              enhanceProgress={enhancing?.path === selectedPath ? enhancing.message : null}
              onTitleChange={refreshSessions}
              onSaveError={(message) => setStatus({ kind: "error", message })}
              onActiveEdit={(title, notes) => {
                setActiveTitle(title);
                setActiveNotes(notes);
              }}
              transcriptFindRequest={
                transcriptFindRequest?.path === selectedPath ? transcriptFindRequest : null
              }
              transcriptJumpRequest={
                transcriptJumpRequest?.path === selectedPath ? transcriptJumpRequest : null
              }
            />
          ) : sessionDataError ? (
            <div className="flex h-full items-center justify-center">
              <div
                role="alert"
                className="flex w-full max-w-md items-start gap-3 rounded-[9px] border border-destructive/25 bg-card p-4 text-card-foreground shadow-[0_8px_24px_rgb(0_0_0/0.1)]"
              >
                <CircleAlert
                  aria-hidden="true"
                  className="mt-0.5 size-4 shrink-0 text-destructive"
                />
                <div className="min-w-0">
                  <p className="text-[12px] font-semibold text-destructive">
                    Could not open note
                  </p>
                  <p className="mt-1 text-[12.5px] leading-[1.45] [overflow-wrap:anywhere]">
                    {sessionDataError}
                  </p>
                </div>
              </div>
            </div>
          ) : sessionData === null ? (
            <p className="text-sm text-muted-foreground">loading...</p>
          ) : (
            <SessionView
              ref={activeSessionViewRef}
              key={`${selectedPath}:${reloadTick}`}
              path={selectedPath}
              isActive={false}
              initialData={sessionData}
              // SessionData carries no duration; the sidebar's scan does.
              durationSecs={sessions.find((s) => s.path === selectedPath)?.duration_secs ?? null}
              liveLines={lines}
              interimMe={interimMe}
              interimThem={interimThem}
              retranscribingJob={retranscribingJob}
              enhanceRunning={enhancingPath === selectedPath}
              enhanceProgress={enhancing?.path === selectedPath ? enhancing.message : null}
              onTitleChange={refreshSessions}
              onSaveError={(message) => setStatus({ kind: "error", message })}
              onRetranscribeStart={handleManualRetranscribeStart}
              onRetranscribeEnd={handleManualRetranscribeEnd}
              transcriptFindRequest={
                transcriptFindRequest?.path === selectedPath ? transcriptFindRequest : null
              }
              transcriptJumpRequest={
                transcriptJumpRequest?.path === selectedPath ? transcriptJumpRequest : null
              }
            />
          )}
        </div>
        )}
      </div>

      {/* Ask this call (stage 4.2): the drawer converses with the OPEN note,
          recording or not; chat is CLI-only, so a mid-call ask never touches
          the live pipeline. Hidden while Ask your notes owns the main pane. */}
      {drawerVisible && selectedPath !== null && (
        <>
          <ResizeHandle
            label="Resize Ask this call"
            onDragStart={() => { dragBaseWidthRef.current = askCallWidth; }}
            // The drawer sits on the RIGHT: dragging its left edge leftward
            // (negative delta) makes it wider.
            onDrag={(deltaX) =>
              setAskCallWidth(
                clampPaneWidth(
                  ASK_CALL_WIDTH,
                  dragBaseWidthRef.current - deltaX,
                  window.innerWidth - MAIN_PANE_MIN - (sidebarCollapsed ? 0 : sidebarWidth),
                ),
              )
            }
            onReset={() => setAskCallWidth(ASK_CALL_WIDTH.default)}
          />
          <AskCallDrawer
            width={askCallWidth}
            path={selectedPath}
            sessions={sessions}
            recordingStartedAt={selectedPath === activePath ? recordingStartedAt : null}
            askRequest={askCallRequest}
            onClose={() => setAskCallOpen(false)}
            onOpenCitation={(source) => handleOpenCitation(source, selectedPath)}
          />
        </>
      )}

      <SettingsDialog
        open={settingsOpen}
        onOpenChange={setSettingsOpen}
        onModelsChanged={handleModelsChanged}
      />
      <GlobalSearch
        open={searchOpen}
        onOpenChange={setSearchOpen}
        sessions={sessions}
        onOpenSession={(path: string, spoken?: SpokenSearchTarget) => {
          setAskNotesOpen(false);
          setSelectedPath(path);
          setTranscriptFindRequest({
            id: ++transcriptFindRequestIdRef.current,
            path,
            query: spoken?.query ?? null,
            timestamp: spoken?.timestamp,
          });
        }}
        onAskAllNotes={handleAskAllNotes}
        onAskThisCall={selectedPath !== null && !askNotesOpen ? handleAskThisCall : undefined}
      />
    </div>
  );
}

export default App;
