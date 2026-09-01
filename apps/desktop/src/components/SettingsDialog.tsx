import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { ChevronLeft, Trash2 } from "lucide-react";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { Label } from "@/components/ui/label";
import { Checkbox } from "@/components/ui/checkbox";
import { cn } from "@/lib/utils";
import {
  deleteModel,
  downloadModel,
  getConfig,
  listEnhanceProviders,
  listModels,
  setActiveModel,
  setAudioFormat,
  setAutoRetranscribe,
  setAutoStopAfterCall,
  setDetectCalls,
  setDeleteAudioAfterRetranscribe,
  setAutoEnhance,
  setEnhanceModel,
  setEnhanceProvider,
  setEnhanceRenameTitle,
  setEnhanceUseCalendar,
  setKeepAudio,
  setOutputDir,
  setRetranscribeModel,
  setShowInDock,
  setTheme,
  type AppConfig,
  type DownloadProgress,
  type EnhanceProviderInfo,
  type ModelInfo,
} from "@/lib/ipc";
import { applyTheme } from "@/lib/theme";
import { ENHANCE_MODEL_OPTIONS } from "@/lib/enhanceModels";
import { downloadProgressPercent, mergeDownloadProgress } from "@/lib/modelProgress";

const THEME_OPTIONS: { value: string; label: string }[] = [
  { value: "light", label: "Light" },
  { value: "dark", label: "Dark" },
  { value: "system", label: "Auto" },
];

/**
 * Stage-0.7 format change: AAC is the default (owner decision after a
 * measured codec benchmark), FLAC stays available as a lossless opt-in. The
 * numbers in the helper text below are the actual measured ones, not
 * estimates, so the size/quality tradeoff is visible before someone flips
 * the toggle.
 */
const AUDIO_FORMAT_OPTIONS: { value: string; label: string }[] = [
  { value: "aac", label: "AAC" },
  { value: "flac", label: "FLAC" },
];

/**
 * Formats a GB value with 3 significant figures, trailing zeros trimmed
 * (1.081 -> "1.08 GB", 3.095 -> "3.1 GB") -- matches the decimal-MB numbers
 * quoted in the README's Models section exactly, since both derive from the
 * same "divide by 1000, not 1024" convention.
 */
function formatGb(gb: number): string {
  const decimals = gb >= 10 ? 1 : 2;
  return `${parseFloat(gb.toFixed(decimals))} GB`;
}

/** Formats a byte count as "487 MB" or "1.08 GB" (decimal MB and GB throughout, never 1024). */
function humanBytes(bytes: number): string {
  const mb = bytes / 1_000_000;
  if (mb >= 1000) return formatGb(mb / 1000);
  return `${Math.round(mb)} MB`;
}

/** Formats a registry `size_mb` (already in MB) the same way as `humanBytes`. */
function humanSizeMb(sizeMb: number): string {
  if (sizeMb >= 1000) return formatGb(sizeMb / 1000);
  return `${sizeMb} MB`;
}

/**
 * Real bytes a deleting `model` would free: its actual on-disk size when
 * known, falling back to the advertised `size_mb` (stage 0.8) -- a model
 * should only ever reach the delete control while downloaded (so
 * `size_on_disk_bytes` should be set), but the fallback keeps the confirm
 * copy honest even if that invariant is ever violated by a stale row.
 */
function freedBytes(model: ModelInfo): number {
  return model.size_on_disk_bytes ?? model.size_mb * 1_000_000;
}

/** Sum of real on-disk bytes across every currently-downloaded model in `list`. */
function totalDiskUsed(list: ModelInfo[]): number {
  return list.reduce((sum, model) => (model.downloaded ? sum + freedBytes(model) : sum), 0);
}


/** The custom radio input every model/provider selection row uses. */
const RADIO_CLASS =
  "size-4 shrink-0 cursor-default appearance-none rounded-full border-[1.5px] border-muted-foreground/60 transition-all checked:border-[5px] checked:border-primary disabled:opacity-40";

/**
 * The left-rail pages; a nav click swaps which section is visible.
 *
 * Two pages, grouped by what the user is doing rather than by subsystem
 * (owner request, refined during e2e): General holds ALL behavior -- the
 * app, everything about a call (detection, audio retention), what happens
 * after it ends, and how summaries behave. Models is a pure download
 * center plus per-function selection (live, re-transcribe, summaries):
 * no behavior toggles live there.
 */
const NAV_SECTIONS = [
  { id: "settings-general", label: "General" },
  { id: "settings-models", label: "Models" },
] as const;

/**
 * One Raycast-style settings row: label + helper text on the left, the
 * control pinned right. Rows stack inside a bordered group card with
 * hairlines between them (`divide-y` on the card).
 */
function Row({
  label,
  htmlFor,
  description,
  error,
  children,
}: {
  label: ReactNode;
  htmlFor?: string;
  description?: ReactNode;
  error?: string | null;
  children?: ReactNode;
}) {
  return (
    <div className="flex items-center justify-between gap-6 px-0.5 py-3.5">
      <div className="flex min-w-0 flex-col gap-[3px]">
        <Label className="text-[13px] font-medium" htmlFor={htmlFor}>{label}</Label>
        {description && <span className="text-[11.5px] leading-[1.45] text-muted-foreground">{description}</span>}
        {error && <span className="font-mono text-[10.5px] text-destructive">{error}</span>}
      </div>
      {children && <div className="flex shrink-0 items-center gap-1.5">{children}</div>}
    </div>
  );
}

/** A bordered group of rows, the section's visual unit. */
function Group({ children }: { children: ReactNode }) {
  return <div className="divide-y divide-border border-y border-border">{children}</div>;
}

interface SettingsDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /**
   * Called whenever fresh config/model data is fetched (on mount, on open,
   * or after a download/activation completes), so the main screen can
   * re-derive its "model not downloaded" gating without waiting for the
   * dialog to close.
   */
  onModelsChanged?: (config: AppConfig, models: ModelInfo[]) => void;
}

/**
 * Download/error progress for one model row, shared between the
 * Recording and Re-transcribe groups (stage 0.8 model manager). Only
 * rendered while the model isn't downloaded yet -- once `download_model`
 * finishes, `list_models` reports `downloaded: true` and this returns null,
 * which is what lets the row settle into its normal (radio/Use/delete)
 * controls instead of a stale progress bar.
 */
function ModelProgress({ progress }: { progress: DownloadProgress | undefined }) {
  if (!progress) return null;
  if (progress.phase === "download") {
    return (
      <div className="flex flex-col gap-1">
        <Progress value={downloadProgressPercent(progress)} />
        <span className="text-xs text-muted-foreground">
          {humanBytes(progress.received)} / {progress.total > 0 ? humanBytes(progress.total) : "?"}
        </span>
      </div>
    );
  }
  if (progress.phase === "error") {
    return <span className="text-xs text-destructive">{progress.message}</span>;
  }
  return null;
}

/**
 * The download / delete / "Delete N MB?" confirm control cluster one model
 * row's right side carries (stage 0.8 semantics, unchanged). Shared between
 * the re-transcribe rows and the local summary provider row instead of two
 * copy-pasted ~40-line blocks. The caller decides where `ModelProgress` and
 * the per-row delete error render (their placement differs per list).
 */
function ModelActions({
  model,
  progress,
  busy,
  confirming,
  onDownload,
  onDeleteClick,
  onConfirmDelete,
  onCancelDelete,
}: {
  model: ModelInfo;
  progress: DownloadProgress | undefined;
  busy: boolean;
  confirming: boolean;
  onDownload: (id: string) => void;
  onDeleteClick: (id: string) => void;
  onConfirmDelete: (id: string) => void;
  onCancelDelete: () => void;
}) {
  return (
    <>
      {!model.downloaded && !model.retired && (!progress || progress.phase === "error") && (
        <Button variant="outline" size="sm" onClick={() => onDownload(model.id)}>
          Download
        </Button>
      )}
      {model.downloaded && !confirming && (
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label={`Delete ${model.name}`}
          disabled={busy}
          className="text-muted-foreground hover:bg-destructive/10 hover:text-destructive"
          onClick={() => onDeleteClick(model.id)}
        >
          <Trash2 />
        </Button>
      )}
      {model.downloaded && confirming && (
        <>
          <Button
            variant="destructive"
            size="sm"
            disabled={busy}
            onClick={() => onConfirmDelete(model.id)}
          >
            Delete {humanBytes(freedBytes(model))}?
          </Button>
          <Button variant="ghost" size="sm" disabled={busy} onClick={onCancelDelete}>
            Cancel
          </Button>
        </>
      )}
    </>
  );
}

export function SettingsDialog({ open: isOpen, onOpenChange, onModelsChanged }: SettingsDialogProps) {
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [activeSection, setActiveSection] = useState<string>(NAV_SECTIONS[0].id);
  const [enhanceProviders, setEnhanceProviders] = useState<EnhanceProviderInfo[]>([]);
  const [enhanceProviderError, setEnhanceProviderError] = useState<string | null>(null);
  const [autoEnhanceError, setAutoEnhanceError] = useState<string | null>(null);
  const [renameTitleError, setRenameTitleError] = useState<string | null>(null);
  const [useCalendarError, setUseCalendarError] = useState<string | null>(null);
  const [choosingDir, setChoosingDir] = useState(false);
  const [themeError, setThemeError] = useState<string | null>(null);
  const [showInDockError, setShowInDockError] = useState<string | null>(null);
  const [folderError, setFolderError] = useState<string | null>(null);
  const [keepAudioError, setKeepAudioError] = useState<string | null>(null);
  const [audioFormatError, setAudioFormatError] = useState<string | null>(null);
  const [autoRetranscribeError, setAutoRetranscribeError] = useState<string | null>(null);
  const [deleteAudioAfterError, setDeleteAudioAfterError] = useState<string | null>(null);
  const [detectCallsError, setDetectCallsError] = useState<string | null>(null);
  const [autoStopAfterCallError, setAutoStopAfterCallError] = useState<string | null>(null);
  const [retranscribeError, setRetranscribeError] = useState<string | null>(null);
  const [liveModelError, setLiveModelError] = useState<string | null>(null);
  const [progress, setProgress] = useState<Record<string, DownloadProgress>>({});
  const [busyIds, setBusyIds] = useState<Set<string>>(new Set());
  // Id of the row currently showing the inline "Delete N MB?" confirm step
  // (stage 0.8), at most one at a time. A small ref-free piece of state is
  // enough here: unlike `inFlightDownloadsRef` below, a double-click on the
  // same confirm button just calls `deleteModel` twice, which the backend
  // already treats as a no-op the second time (`remove_model_files` is
  // idempotent) rather than an error.
  const [deleteConfirmId, setDeleteConfirmId] = useState<string | null>(null);
  const [deleteErrors, setDeleteErrors] = useState<Record<string, string>>({});
  const onModelsChangedRef = useRef(onModelsChanged);
  onModelsChangedRef.current = onModelsChanged;
  // Synchronous (ref, not state) guard against a double-click firing
  // `downloadModel` twice before the first progress event or re-render has
  // landed: state updates alone aren't enough since two click events can, in
  // principle, both run before React commits the first one.
  const inFlightDownloadsRef = useRef<Set<string>>(new Set());
  // The one scroll container both pages share (sections are CSS-hidden, not
  // remounted): switching pages must not inherit the previous page's scroll
  // position, so the nav click resets it.
  const contentScrollRef = useRef<HTMLDivElement>(null);

  const refresh = useCallback(async () => {
    const [freshConfig, freshModels] = await Promise.all([getConfig(), listModels()]);
    setConfig(freshConfig);
    setModels(freshModels);
    // Drop any armed delete confirmation (stage 0.8 security-review fix): a
    // background refresh -- a download finishing while the dialog is
    // closed, or the periodic re-sync below -- must not leave a row parked
    // one click from `deleteModel` with no fresh gate. Rearming always
    // requires a deliberate new click on the trash icon after this.
    setDeleteConfirmId(null);
    setDeleteErrors({});
    onModelsChangedRef.current?.(freshConfig, freshModels);
    return freshModels;
  }, []);

  // Initial load: this component is mounted for the app's whole lifetime
  // (App always renders it, visibility is just `open`), so this also
  // primes the main screen's gating state on first paint.
  useEffect(() => {
    refresh().catch(() => {
      // Leave config/models empty; the dialog just shows nothing to manage
      // rather than a broken screen.
    });
  }, [refresh]);

  // Re-sync every time the dialog opens: a download can finish while it was
  // closed, and the row state (downloaded/active) needs to reflect that.
  // `refresh()` also clears any armed delete confirmation, so re-opening
  // never surfaces a stale "Delete N MB?" button from before it was closed.
  useEffect(() => {
    if (!isOpen) return;
    refresh().catch(() => {});
    // Provider availability re-probes on every open too: installing the
    // claude/codex CLI while the app runs should be picked up without a
    // restart.
    listEnhanceProviders()
      .then(setEnhanceProviders)
      .catch(() => {});
  }, [isOpen, refresh]);

  /** Toggles auto-enhance (stage 0.11). Optimistic with rollback like the rest. */
  async function handleSetAutoEnhance(autoEnhance: boolean) {
    const previous = config?.auto_enhance;
    if (previous === undefined || autoEnhance === previous) return;
    setAutoEnhanceError(null);
    setConfig((prev) => (prev ? { ...prev, auto_enhance: autoEnhance } : prev));
    try {
      await setAutoEnhance(autoEnhance);
    } catch (err) {
      setConfig((prev) => (prev ? { ...prev, auto_enhance: previous } : prev));
      setAutoEnhanceError(String(err));
    }
  }

  /** Toggles model-proposed titles. Optimistic with rollback like the rest. */
  async function handleSetEnhanceRenameTitle(enabled: boolean) {
    const previous = config?.enhance_rename_title;
    if (previous === undefined || enabled === previous) return;
    setRenameTitleError(null);
    setConfig((prev) => (prev ? { ...prev, enhance_rename_title: enabled } : prev));
    try {
      await setEnhanceRenameTitle(enabled);
    } catch (err) {
      setConfig((prev) => (prev ? { ...prev, enhance_rename_title: previous } : prev));
      setRenameTitleError(String(err));
    }
  }

  /** Toggles calendar context for Claude Code. Optimistic with rollback like the rest. */
  async function handleSetEnhanceUseCalendar(enabled: boolean) {
    const previous = config?.enhance_use_calendar;
    if (previous === undefined || enabled === previous) return;
    setUseCalendarError(null);
    setConfig((prev) => (prev ? { ...prev, enhance_use_calendar: enabled } : prev));
    try {
      await setEnhanceUseCalendar(enabled);
    } catch (err) {
      setConfig((prev) => (prev ? { ...prev, enhance_use_calendar: previous } : prev));
      setUseCalendarError(String(err));
    }
  }

  /**
   * Persists one provider's model override on blur/Enter (stage 0.11).
   * Optimistic with rollback like the rest; an empty value clears the
   * override back to the CLI's default.
   */
  async function handleSetEnhanceModel(provider: string, model: string) {
    const previous = config?.enhance_models?.[provider] ?? "";
    const next = model.trim();
    if (previous === next || !config) return;
    setEnhanceProviderError(null);
    setConfig((prev) =>
      prev ? { ...prev, enhance_models: { ...prev.enhance_models, [provider]: next } } : prev
    );
    try {
      await setEnhanceModel(provider, next);
    } catch (err) {
      setConfig((prev) =>
        prev
          ? { ...prev, enhance_models: { ...prev.enhance_models, [provider]: previous } }
          : prev
      );
      setEnhanceProviderError(String(err));
    }
  }

  /**
   * Picks which CLI `enhanceSession` runs (stage 0.11). Optimistic with
   * rollback, same as every other row here.
   */
  async function handleSetEnhanceProvider(provider: string) {
    const previous = config?.enhance_provider;
    if (provider === previous || previous === undefined) return;
    setEnhanceProviderError(null);
    setConfig((prev) => (prev ? { ...prev, enhance_provider: provider } : prev));
    try {
      await setEnhanceProvider(provider);
    } catch (err) {
      setConfig((prev) => (prev ? { ...prev, enhance_provider: previous } : prev));
      setEnhanceProviderError(String(err));
    }
  }

  // Belt-and-suspenders on top of `refresh()`'s own reset: the dialog is
  // mounted for the app's whole lifetime and only ever hides via `isOpen`
  // (see the comment above), so closing it without choosing -- the natural
  // way to back out of an armed delete -- must disarm it immediately rather
  // than leaving a live "Delete N MB?" button sitting there until the next
  // refresh happens to run.
  useEffect(() => {
    if (isOpen) return;
    setDeleteConfirmId(null);
    setDeleteErrors({});
  }, [isOpen]);

  // Global listener, mounted regardless of `isOpen`: downloads continue in
  // Rust even if the dialog is closed, so progress must keep being tracked
  // and the model list must refetch on completion no matter what the dialog
  // is doing at that moment.
  useEffect(() => {
    const unlisten = listen<DownloadProgress>("model-download-progress", (event) => {
      const payload = event.payload;
      setProgress((prev) => ({
        ...prev,
        [payload.id]: mergeDownloadProgress(prev[payload.id], payload),
      }));
      if (payload.phase === "done") {
        // Refetch (and let the row flip to "Active"/"Use" via the fresh
        // `model.downloaded`) before dropping the progress entry: deleting
        // it first would leave a gap, with `model.downloaded` still stale
        // (false) and no progress entry, where the row briefly renders a
        // "Download" button for an already-finished download.
        refresh()
          .catch(() => {})
          .finally(() => {
            listEnhanceProviders()
              .then(setEnhanceProviders)
              .catch(() => {});
            setProgress((prev) => {
              const next = { ...prev };
              delete next[payload.id];
              return next;
            });
          });
      }
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, [refresh]);

  async function handleSetTheme(theme: string) {
    const previous = config?.theme;
    if (theme === previous || previous === undefined) return;
    setThemeError(null);
    // Apply immediately (no waiting on the round-trip) so the toggle feels
    // instant; the optimistic `config` update below keeps the active button
    // in sync even though `refresh()` isn't called here. If the IPC call
    // below rejects (e.g. Config::save disk error), both are rolled back so
    // the applied class and the highlighted button never diverge from what
    // actually got persisted.
    applyTheme(theme);
    setConfig((prev) => (prev ? { ...prev, theme } : prev));
    try {
      await setTheme(theme);
    } catch (err) {
      applyTheme(previous);
      setConfig((prev) => (prev ? { ...prev, theme: previous } : prev));
      setThemeError(String(err));
    }
  }

  async function handleSetShowInDock(showInDock: boolean) {
    const previous = config?.show_in_dock;
    if (previous === undefined || showInDock === previous) return;
    setShowInDockError(null);
    setConfig((prev) => (prev ? { ...prev, show_in_dock: showInDock } : prev));
    try {
      await setShowInDock(showInDock);
    } catch (err) {
      setConfig((prev) => (prev ? { ...prev, show_in_dock: previous } : prev));
      setShowInDockError(String(err));
    }
  }

  async function handleSetKeepAudio(keepAudio: boolean) {
    const previous = config?.keep_audio;
    if (previous === undefined || keepAudio === previous) return;
    setKeepAudioError(null);
    // Optimistic update (same pattern as `handleSetTheme`): flip the
    // checkbox immediately, then persist. On failure both the checkbox and
    // the underlying config revert to `previous` rather than leaving the UI
    // showing a state that was never actually saved.
    setConfig((prev) => (prev ? { ...prev, keep_audio: keepAudio } : prev));
    try {
      await setKeepAudio(keepAudio);
      await refresh();
    } catch (err) {
      setConfig((prev) => (prev ? { ...prev, keep_audio: previous } : prev));
      setKeepAudioError(String(err));
    }
  }

  async function handleSetAudioFormat(format: string) {
    const previous = config?.audio_format;
    if (format === previous || previous === undefined) return;
    setAudioFormatError(null);
    // Same optimistic-update-with-rollback pattern as `handleSetTheme`/
    // `handleSetKeepAudio`: only affects FUTURE recordings, so there's
    // nothing to re-render besides the button state itself.
    setConfig((prev) => (prev ? { ...prev, audio_format: format } : prev));
    try {
      await setAudioFormat(format);
    } catch (err) {
      setConfig((prev) => (prev ? { ...prev, audio_format: previous } : prev));
      setAudioFormatError(String(err));
    }
  }

  /**
   * Toggles whether `stop_recording` auto-queues a re-transcribe job for the
   * session that just stopped (stage 0.9). Same optimistic-update-with-
   * rollback pattern as `handleSetKeepAudio`/`handleSetAudioFormat`: only
   * ever offered while `keep_audio` is on (see the Checkbox's `disabled`
   * prop below), but the backend re-checks that too.
   */
  async function handleSetAutoRetranscribe(autoRetranscribe: boolean) {
    const previous = config?.auto_retranscribe;
    if (previous === undefined || autoRetranscribe === previous) return;
    setAutoRetranscribeError(null);
    setConfig((prev) => (prev ? { ...prev, auto_retranscribe: autoRetranscribe } : prev));
    try {
      await setAutoRetranscribe(autoRetranscribe);
    } catch (err) {
      setConfig((prev) => (prev ? { ...prev, auto_retranscribe: previous } : prev));
      setAutoRetranscribeError(String(err));
    }
  }

  /**
   * Toggles whether a successful re-transcribe (manual or automatic, per
   * Global Constraints) deletes the session's audio afterward (stage 0.9).
   * Same optimistic-update-with-rollback pattern as the other rows in this
   * section.
   */
  async function handleSetDeleteAudioAfterRetranscribe(deleteAudioAfter: boolean) {
    const previous = config?.delete_audio_after_retranscribe;
    if (previous === undefined || deleteAudioAfter === previous) return;
    setDeleteAudioAfterError(null);
    setConfig((prev) => (prev ? { ...prev, delete_audio_after_retranscribe: deleteAudioAfter } : prev));
    try {
      await setDeleteAudioAfterRetranscribe(deleteAudioAfter);
    } catch (err) {
      setConfig((prev) => (prev ? { ...prev, delete_audio_after_retranscribe: previous } : prev));
      setDeleteAudioAfterError(String(err));
    }
  }

  async function handleSetDetectCalls(enabled: boolean) {
    const previous = config?.detect_calls;
    if (previous === undefined || enabled === previous) return;
    setDetectCallsError(null);
    setConfig((prev) => (prev ? { ...prev, detect_calls: enabled } : prev));
    try {
      await setDetectCalls(enabled);
      await refresh();
    } catch (err) {
      setConfig((prev) => (prev ? { ...prev, detect_calls: previous } : prev));
      setDetectCallsError(String(err));
    }
  }

  async function handleSetAutoStopAfterCall(enabled: boolean) {
    const previous = config?.auto_stop_after_call;
    if (previous === undefined || enabled === previous) return;
    setAutoStopAfterCallError(null);
    setConfig((prev) => (prev ? { ...prev, auto_stop_after_call: enabled } : prev));
    try {
      await setAutoStopAfterCall(enabled);
      await refresh();
    } catch (err) {
      setConfig((prev) => (prev ? { ...prev, auto_stop_after_call: previous } : prev));
      setAutoStopAfterCallError(String(err));
    }
  }

  async function handleChooseFolder() {
    setChoosingDir(true);
    setFolderError(null);
    try {
      const selected = await open({ directory: true });
      if (typeof selected === "string") {
        await setOutputDir(selected);
        await refresh();
      }
    } catch (err) {
      setFolderError(String(err));
    } finally {
      setChoosingDir(false);
    }
  }

  async function handleUse(id: string) {
    setBusyIds((prev) => new Set(prev).add(id));
    setLiveModelError(null);
    try {
      await setActiveModel(id);
      await refresh();
    } catch (err) {
      // Every other control surfaces its rejection inline; this one used to
      // swallow it as an unhandled rejection (silent no-op for the user).
      setLiveModelError(String(err));
    } finally {
      setBusyIds((prev) => {
        const next = new Set(prev);
        next.delete(id);
        return next;
      });
    }
  }

  /**
   * Selects `id` as the re-transcribe model (stage 0.8): only ever called
   * for an already-downloaded model (the radio control isn't offered
   * otherwise). No optimistic update -- `config.retranscribe_model` only
   * drives which radio is checked, so waiting for `refresh()` avoids a
   * flicker back to the old selection if the call rejects.
   */
  async function handleSetRetranscribeModel(id: string) {
    if (config?.retranscribe_model === id) return;
    setBusyIds((prev) => new Set(prev).add(id));
    setRetranscribeError(null);
    try {
      await setRetranscribeModel(id);
      await refresh();
    } catch (err) {
      setRetranscribeError(String(err));
    } finally {
      setBusyIds((prev) => {
        const next = new Set(prev);
        next.delete(id);
        return next;
      });
    }
  }

  async function handleDownload(id: string) {
    if (inFlightDownloadsRef.current.has(id)) return;
    inFlightDownloadsRef.current.add(id);
    // Set a placeholder "download" progress entry synchronously (in the same
    // tick as the click) so the Download button disappears immediately,
    // rather than waiting for the first real progress event (which can be
    // seconds away on a slow connection) - that gap is what let a double
    // click fire `downloadModel` twice.
    setProgress((prev) => ({
      ...prev,
      [id]: { id, phase: "download", received: 0, total: 0 },
    }));
    try {
      await downloadModel(id);
    } catch (err) {
      const message = String(err);
      // A rejection carrying the backend's concurrency-guard message means
      // a real download for this id is already in flight (and its progress
      // events are landing via the global listener) - don't clobber that
      // live state with a false "error" row.
      if (!message.includes("already downloading")) {
        setProgress((prev) => ({
          ...prev,
          [id]: { id, phase: "error", received: 0, total: 0, message },
        }));
      }
    } finally {
      inFlightDownloadsRef.current.delete(id);
    }
  }

  /** First click on a row's trash icon: shows the "Delete N MB?" confirm step. */
  function handleDeleteClick(id: string) {
    setDeleteErrors((prev) => {
      const next = { ...prev };
      delete next[id];
      return next;
    });
    setDeleteConfirmId(id);
  }

  function handleCancelDelete() {
    setDeleteConfirmId(null);
  }

  /** Second click, on the confirm button itself: actually deletes the model. */
  async function handleConfirmDelete(id: string) {
    setBusyIds((prev) => new Set(prev).add(id));
    try {
      await deleteModel(id);
      await refresh();
      listEnhanceProviders()
        .then(setEnhanceProviders)
        .catch(() => {});
      setDeleteConfirmId(null);
    } catch (err) {
      setDeleteErrors((prev) => ({ ...prev, [id]: String(err) }));
    } finally {
      setBusyIds((prev) => {
        const next = new Set(prev);
        next.delete(id);
        return next;
      });
    }
  }

  const liveModels = models.filter((m) => m.role === "live");
  const retranscribeModels = models.filter((m) => m.role === "retranscribe");
  const summaryModels = models.filter((m) => m.role === "summary");
  const selectedRetranscribeModel = models.find((m) => m.id === config?.retranscribe_model);
  // True when the configured re-transcribe model is a retired entry that's
  // still on disk (stage 0.8.x curation): distinct from "missing" below,
  // since the file genuinely IS there and re-downloading it is impossible
  // (the backend rejects retired ids) -- the generic "not downloaded...
  // download it again" copy would contradict the "(no longer offered)"
  // label on the row right below it. Retired rows never render a radio, so
  // a config pointing at one also leaves the group with nothing checked;
  // this gives that state its own accurate explanation.
  const retranscribeModelRetired = Boolean(selectedRetranscribeModel?.retired);
  // True once a retranscribe model is configured but genuinely not on disk
  // (e.g. it was just deleted from this same screen, or names an id
  // `list_models` doesn't currently report at all) -- excludes the
  // retired-but-downloaded case above, which gets its own distinct message.
  const retranscribeModelMissing =
    Boolean(config?.retranscribe_model) &&
    !retranscribeModelRetired &&
    !models.some((m) => m.id === config?.retranscribe_model && m.downloaded);
  const activeSectionLabel =
    NAV_SECTIONS.find((section) => section.id === activeSection)?.label ?? "General";

  return (
    <Dialog open={isOpen} onOpenChange={onOpenChange}>
      {/* Full-window settings (stage 0.10, Raycast-style): a left rail of
          sections and a scrolling content column, sized like the main
          window so there's room for the features to come. Each nav item is
          its own page; inactive sections are CSS-hidden, not unmounted, so
          a download's progress keeps rendering wherever you navigate. */}
      <DialogContent
        showCloseButton={false}
        className="top-0 left-0 flex h-full w-full max-w-none translate-x-0 translate-y-0 flex-col gap-0 rounded-none border-0 p-0 sm:max-w-none"
      >
        <div className="flex min-h-0 flex-1">
          <nav className="flex w-56 shrink-0 flex-col border-r border-sidebar-border bg-sidebar px-3 pb-3">
            {/* Same overlay-title-bar drag strip as the main window: the
                traffic lights float over this corner. */}
            <div data-tauri-drag-region className="h-14 shrink-0" />
            <button
              type="button"
              onClick={() => onOpenChange(false)}
              className="flex h-7 items-center gap-2 rounded-[7px] px-2 text-left text-[12px] text-quiet-foreground hover:bg-foreground/[0.06] hover:text-foreground"
            >
              <ChevronLeft className="size-3.5" />
              Notes
            </button>
            <p className="px-2 pb-2 pt-3.5 font-mono text-[10px] font-medium uppercase tracking-[0.08em] text-muted-foreground">
              Settings
            </p>
            {NAV_SECTIONS.map(({ id, label }) => (
              <button
                key={id}
                type="button"
                onClick={() => {
                  setActiveSection(id);
                  // A plain property write, not scrollTo(): jsdom (the test
                  // environment) does not implement scrolling methods and
                  // throws from them, which fails vitest after the suite as
                  // an unhandled onClick error (v0.7.0's first CI run).
                  if (contentScrollRef.current) {
                    contentScrollRef.current.scrollTop = 0;
                  }
                }}
                className={cn(
                  "flex h-[30px] items-center rounded-[7px] px-2.5 text-left text-[12.5px] transition-colors duration-150",
                  activeSection === id
                    ? "bg-foreground font-medium text-background"
                    : "text-quiet-foreground hover:bg-foreground/[0.06] hover:text-foreground"
                )}
              >
                {label}
              </button>
            ))}
          </nav>

          <div className="flex min-h-0 min-w-0 flex-1 flex-col">
            <header data-tauri-drag-region className="flex h-14 shrink-0 items-center justify-between gap-6 px-8">
              <DialogTitle className="font-serif text-[17px] font-semibold tracking-[-0.005em]">
                {activeSectionLabel}
              </DialogTitle>
              <span className="font-mono text-[10.5px] text-muted-foreground">saved automatically</span>
            </header>
            <div ref={contentScrollRef} className="min-h-0 flex-1 overflow-y-auto">
              <div className="flex w-full flex-col px-8 pb-10 pt-1.5">
                {/* General: app behavior plus everything about a call --
                    detection lives here (it decides when recording is
                    offered, nothing to do with audio encoding), followed by
                    what gets kept afterwards. */}
                <section id="settings-general" className={cn("flex flex-col gap-6", activeSection !== "settings-general" && "hidden")}>
                  <div className="flex flex-col gap-1.5">
                  <h4 className="font-mono text-[10px] font-medium uppercase tracking-[0.08em] text-muted-foreground">App</h4>
                  <Group>
                    <Row label="Theme" error={themeError}>
                      {THEME_OPTIONS.map((option) => (
                        <Button
                          key={option.value}
                          variant={config?.theme === option.value ? "default" : "outline"}
                          size="sm"
                          onClick={() => handleSetTheme(option.value)}
                        >
                          {option.label}
                        </Button>
                      ))}
                    </Row>
                    <Row
                      label="Show in Dock"
                      htmlFor="show-in-dock"
                      description="Turn this off to keep Simstim in the menu bar only."
                      error={showInDockError}
                    >
                      <Checkbox
                        variant="switch"
                        id="show-in-dock"
                        checked={config?.show_in_dock ?? true}
                        disabled={!config}
                        onCheckedChange={(checked) => handleSetShowInDock(checked === true)}
                      />
                    </Row>
                    <Row
                      label="Notes folder"
                      description={
                        <span
                          className="block max-w-72 truncate font-mono"
                          style={{ direction: "rtl", textAlign: "left" }}
                          title={config?.output_dir ?? ""}
                        >
                          {config?.output_dir ?? "..."}
                        </span>
                      }
                      error={folderError}
                    >
                      <Button variant="outline" size="sm" disabled={choosingDir} onClick={handleChooseFolder}>
                        Change
                      </Button>
                    </Row>
                  </Group>
                  </div>
                  <div className="flex flex-col gap-1.5">
                  <h4 className="font-mono text-[10px] font-medium uppercase tracking-[0.08em] text-muted-foreground">Call detection</h4>
                  <Group>
                    <Row
                      label="Offer to record when a call starts"
                      htmlFor="detect-calls"
                      description="Watches microphone activity from known calling apps. Recording still needs your click."
                      error={detectCallsError}
                    >
                      <Checkbox
                        variant="switch"
                        id="detect-calls"
                        checked={config?.detect_calls ?? true}
                        disabled={!config}
                        onCheckedChange={(checked) => handleSetDetectCalls(checked === true)}
                      />
                    </Row>
                    <Row
                      label="Stop automatically when the call ends"
                      htmlFor="auto-stop-after-call"
                      description="Waits for the call app to release audio and for the remote channel to stay silent."
                      error={autoStopAfterCallError}
                    >
                      <Checkbox
                        variant="switch"
                        id="auto-stop-after-call"
                        checked={config?.auto_stop_after_call ?? false}
                        disabled={!config || !config.detect_calls}
                        onCheckedChange={(checked) => handleSetAutoStopAfterCall(checked === true)}
                      />
                    </Row>
                  </Group>
                  </div>
                  <div className="flex flex-col gap-1.5">
                  <h4 className="font-mono text-[10px] font-medium uppercase tracking-[0.08em] text-muted-foreground">Call audio</h4>
                  <Group>
                    <Row
                      label="Keep call audio"
                      htmlFor="keep-audio"
                      description="Stores a stereo audio file next to each transcript. Re-transcribing needs it."
                      error={keepAudioError}
                    >
                      <Checkbox
                        variant="switch"
                        id="keep-audio"
                        checked={config?.keep_audio ?? true}
                        disabled={!config}
                        onCheckedChange={(checked) => handleSetKeepAudio(checked === true)}
                      />
                    </Row>
                    <Row
                      label="Audio format"
                      description="AAC: about 17 MB per hour, small quality loss (default). FLAC: lossless, about 137 MB per hour."
                      error={audioFormatError}
                    >
                      {AUDIO_FORMAT_OPTIONS.map((option) => (
                        <Button
                          key={option.value}
                          variant={config?.audio_format === option.value ? "default" : "outline"}
                          size="sm"
                          disabled={!config || !config.keep_audio}
                          onClick={() => handleSetAudioFormat(option.value)}
                        >
                          {option.label}
                        </Button>
                      ))}
                    </Row>
                  </Group>
                  </div>
                  {/* What the app does on its own once a call ends. These
                      configure behavior, not models, so they live here
                      (owner request): Models stays a pure download center
                      plus per-function selection. */}
                  <div className="flex flex-col gap-1.5">
                  <h4 className="font-mono text-[10px] font-medium uppercase tracking-[0.08em] text-muted-foreground">After a call</h4>
                  <Group>
                    {/* Stage 0.9 (fix-review correction): only the AUTO
                        toggle needs `keep_audio` on -- it fires right after
                        a recording stops, so it's useless without an audio
                        file having just been kept. The delete toggle is NOT
                        gated the same way: delete-after applies to ANY
                        successful re-transcribe, including a MANUAL one on
                        an old session that still has audio from back when
                        `keep_audio` was on, even if it's off right now. */}
                    <Row
                      label="Re-transcribe automatically after stopping"
                      htmlFor="auto-retranscribe"
                      description={
                        <>
                          Runs the selected whisper model on the finished call in the background.
                          {config && !config.keep_audio && " Needs call audio."}
                        </>
                      }
                      error={autoRetranscribeError}
                    >
                      <Checkbox
                        variant="switch"
                        id="auto-retranscribe"
                        checked={config?.auto_retranscribe ?? false}
                        disabled={!config || !config.keep_audio}
                        onCheckedChange={(checked) => handleSetAutoRetranscribe(checked === true)}
                      />
                    </Row>
                    <Row
                      label="Delete the audio after re-transcribing"
                      htmlFor="delete-audio-after-retranscribe"
                      description={
                        'Removes the recording once a re-transcribe succeeds - manual or automatic, it always applies, regardless of whether "Keep call audio" is currently on (an old session recorded with it on can still be re-transcribed manually) - and that call can never be re-transcribed again.'
                      }
                      error={deleteAudioAfterError}
                    >
                      <Checkbox
                        variant="switch"
                        id="delete-audio-after-retranscribe"
                        checked={config?.delete_audio_after_retranscribe ?? false}
                        disabled={!config}
                        onCheckedChange={(checked) => handleSetDeleteAudioAfterRetranscribe(checked === true)}
                      />
                    </Row>
                    <Row
                      label="Enhance automatically after each call"
                      htmlFor="auto-enhance"
                      description={'Fires once the call stops (after the automatic re-transcribe when that is on) and after any successful re-transcribe. With empty notes it writes a summary with key points and action items.'}
                      error={autoEnhanceError}
                    >
                      <Checkbox
                        variant="switch"
                        id="auto-enhance"
                        checked={config?.auto_enhance ?? false}
                        disabled={!config}
                        onCheckedChange={(checked) => handleSetAutoEnhance(checked === true)}
                      />
                    </Row>
                  </Group>
                  </div>
                  <div className="flex flex-col gap-1.5">
                  <h4 className="font-mono text-[10px] font-medium uppercase tracking-[0.08em] text-muted-foreground">Summaries</h4>
                  <Group>
                    <Row
                      label="Let it title notes"
                      htmlFor="enhance-rename-title"
                      description={'Replaces automatic titles ("Untitled", "Zoom call 14:03") with one proposed by the summary. Titles you typed yourself are never touched.'}
                      error={renameTitleError}
                    >
                      <Checkbox
                        variant="switch"
                        id="enhance-rename-title"
                        checked={config?.enhance_rename_title ?? false}
                        disabled={!config}
                        onCheckedChange={(checked) => handleSetEnhanceRenameTitle(checked === true)}
                      />
                    </Row>
                    <Row
                      label="Use your calendar for context"
                      htmlFor="enhance-use-calendar"
                      description="Looks up the meeting that overlapped the call through your own calendar connectors, via the Claude Code CLI, and hands its name and participants to whichever cloud provider summarizes. A found event names the note with the event's own name, verbatim, when title renaming is on. Not offered for the local provider: local means no cloud call at all."
                      error={useCalendarError}
                    >
                      <Checkbox
                        variant="switch"
                        id="enhance-use-calendar"
                        checked={config?.enhance_use_calendar ?? false}
                        disabled={!config || config.enhance_provider === "local"}
                        onCheckedChange={(checked) => handleSetEnhanceUseCalendar(checked === true)}
                      />
                    </Row>
                  </Group>
                  </div>
                </section>

                {/* Models: a pure download center plus per-function
                    selection (owner request) -- which model records live,
                    which re-transcribes, which provider summarizes. All
                    behavior toggles live on General. */}
                <section id="settings-models" className={cn("flex flex-col gap-7", activeSection !== "settings-models" && "hidden")}>
            {/* Live transcription (stage 0.8): every `role === "live"` entry.
                Parakeet is the only one today and stays the only one a future
                entry could add to without this section changing -- it's
                driven by `role`, not a hardcoded id. Live models never get a
                delete control here (Global Constraints: deleting the model
                that drives recording would break it). */}
            <div className="flex flex-col gap-1.5">
              <div className="flex items-baseline justify-between gap-2">
                <h4 className="font-mono text-[10px] font-medium uppercase tracking-[0.08em] text-muted-foreground">
                  Live transcription
                </h4>
                <span className="font-mono text-[10.5px] text-muted-foreground">
                  {humanBytes(totalDiskUsed(liveModels))} on disk
                </span>
              </div>
              {liveModelError && <span className="text-xs text-destructive">{liveModelError}</span>}
              <ul className="flex flex-col divide-y divide-border border-y border-border">
                {liveModels.map((model) => {
                  const p = progress[model.id];
                  const busy = busyIds.has(model.id);
                  return (
                    <li key={model.id} className="flex flex-col gap-2 px-0.5 py-3.5">
                      <div className="flex items-center justify-between gap-3">
                        <div className="flex flex-col">
                          <span className="text-sm font-medium">{model.name}</span>
                          {model.technical_name && (
                            <span className="font-mono text-[10px] text-muted-foreground/55">
                              {model.technical_name}
                            </span>
                          )}
                          <span className="text-xs text-muted-foreground">{humanSizeMb(model.size_mb)}</span>
                        </div>
                        {model.downloaded && model.active && (
                          <span className="text-sm text-muted-foreground">Active</span>
                        )}
                        {model.downloaded && !model.active && (
                          <Button variant="outline" size="sm" disabled={busy} onClick={() => handleUse(model.id)}>
                            Use
                          </Button>
                        )}
                        {!model.downloaded && (!p || p.phase === "error") && (
                          <Button variant="outline" size="sm" onClick={() => handleDownload(model.id)}>
                            Download
                          </Button>
                        )}
                      </div>
                      {!model.downloaded && <ModelProgress progress={p} />}
                    </li>
                  );
                })}
              </ul>
            </div>

            {/* Re-transcribe (stage 0.8): every `role === "retranscribe"`
                entry (the whisper family). Radio-style selection among
                downloaded models only reflects `config.retranscribe_model`;
                every downloaded row also gets a delete control to reclaim
                disk. The two automation toggles that configure this pass sit
                in the same list, under the models they run. */}
            <div className="flex flex-col gap-1.5">
              <div className="flex items-baseline justify-between gap-2">
                <h4 className="font-mono text-[10px] font-medium uppercase tracking-[0.08em] text-muted-foreground">
                  Re-transcribe
                </h4>
                <span className="font-mono text-[10.5px] text-muted-foreground">
                  {humanBytes(totalDiskUsed(retranscribeModels))} on disk
                </span>
              </div>
              <span className="text-xs text-muted-foreground">
                Turbo is faster (about 12x realtime measured for this file on this machine).
                Large-v3 is slower but the strongest on non-English speech.
              </span>
              {retranscribeError && <span className="text-xs text-destructive">{retranscribeError}</span>}
              {retranscribeModelRetired && (
                <span className="text-xs text-muted-foreground">
                  Selected model is no longer offered. Pick another model.
                </span>
              )}
              {retranscribeModelMissing && (
                <span className="text-xs text-muted-foreground">
                  Selected model is not downloaded. Pick another or download it again.
                </span>
              )}
              <ul className="flex flex-col divide-y divide-border border-y border-border">
                {retranscribeModels.map((model) => {
                  const p = progress[model.id];
                  const busy = busyIds.has(model.id);
                  // Retired (stage 0.8.x curation): this row only exists at
                  // all because its files are still on disk (`list_models`
                  // hides an absent retired entry outright, see
                  // `models::list_models_at`'s docs) -- the owner needs to
                  // see it so it can be deleted, but it can never be
                  // downloaded or selected again. No radio, no tagline, no
                  // Download button; just the name, size, a muted "no longer
                  // offered" note, and delete.
                  return (
                    <li key={model.id} className="flex flex-col gap-2 px-0.5 py-3.5">
                      <div className="flex items-center justify-between gap-3">
                        <div className="flex min-w-0 items-center gap-2.5">
                          {model.downloaded && !model.retired && (
                            <input
                              type="radio"
                              name="retranscribe-model"
                              aria-label={model.name}
                              checked={config?.retranscribe_model === model.id}
                              disabled={busy}
                              onChange={() => handleSetRetranscribeModel(model.id)}
                              className={RADIO_CLASS}
                            />
                          )}
                          <div className="flex min-w-0 flex-col">
                            <span
                              className={cn(
                                "truncate text-sm font-medium",
                                model.retired && "text-muted-foreground",
                              )}
                            >
                              {model.name}
                            </span>
                            {model.tagline && !model.retired && (
                              <span className="text-xs text-muted-foreground">{model.tagline}</span>
                            )}
                            <span className="text-xs text-muted-foreground">{humanSizeMb(model.size_mb)}</span>
                            {model.retired && (
                              <span className="text-xs text-muted-foreground">(no longer offered)</span>
                            )}
                          </div>
                        </div>

                        <div className="flex shrink-0 items-center gap-1.5">
                          <ModelActions
                            model={model}
                            progress={p}
                            busy={busy}
                            confirming={deleteConfirmId === model.id}
                            onDownload={handleDownload}
                            onDeleteClick={handleDeleteClick}
                            onConfirmDelete={handleConfirmDelete}
                            onCancelDelete={handleCancelDelete}
                          />
                        </div>
                      </div>
                      {!model.downloaded && <ModelProgress progress={p} />}
                      {deleteErrors[model.id] && (
                        <span className="text-xs text-destructive">{deleteErrors[model.id]}</span>
                      )}
                    </li>
                  );
                })}
              </ul>
            </div>

            {/* Summaries: the provider picker IS the model manager (owner
                request: download, selection and configuration on one page).
                The local row carries Qwen's download/progress/delete controls
                inline, next to its own selection radio, instead of pointing
                at a different tab; the cloud CLI rows keep their model
                override menus; the automation toggle sits below them. */}
            <div className="flex flex-col gap-1.5">
              <div className="flex items-baseline justify-between gap-2">
                <h4 className="font-mono text-[10px] font-medium uppercase tracking-[0.08em] text-muted-foreground">
                  Summaries
                </h4>
                <span className="font-mono text-[10.5px] text-muted-foreground">
                  {humanBytes(totalDiskUsed(summaryModels))} on disk
                </span>
              </div>
              <span className="text-xs text-muted-foreground">
                The local model runs entirely on this Mac: notes and transcripts stay private.
                Claude Code and Codex are optional cloud providers: they send only the note's
                text and transcript, and only when you click Enhance or enable automatic
                summaries.
              </span>
              <ul className="flex flex-col divide-y divide-border border-y border-border">
                {summaryModels.map((model) => {
                  const p = progress[model.id];
                  const busy = busyIds.has(model.id);
                  const localProvider = enhanceProviders.find((provider) => provider.id === "local");
                  const name = localProvider?.name ?? model.name;
                  return (
                    <li key={model.id} className="flex flex-col gap-2 px-0.5 py-3.5">
                      <div className="flex items-center justify-between gap-3">
                        <div className="flex min-w-0 flex-col">
                          <span className="truncate text-sm font-medium">{name}</span>
                          {model.technical_name && (
                            <span className="font-mono text-[10px] text-muted-foreground/55">
                              {model.technical_name}
                            </span>
                          )}
                          {model.tagline && (
                            <span className="text-xs text-muted-foreground">{model.tagline}</span>
                          )}
                          <span className="text-xs text-muted-foreground">
                            {humanSizeMb(model.size_mb)}
                            {model.downloaded && " · on-device, Metal"}
                          </span>
                        </div>
                        <div className="flex shrink-0 items-center gap-1.5">
                          <ModelActions
                            model={model}
                            progress={p}
                            busy={busy}
                            confirming={deleteConfirmId === model.id}
                            onDownload={handleDownload}
                            onDeleteClick={handleDeleteClick}
                            onConfirmDelete={handleConfirmDelete}
                            onCancelDelete={handleCancelDelete}
                          />
                          {!model.retired && (
                            <input
                              type="radio"
                              name="enhance-provider"
                              aria-label={name}
                              checked={config?.enhance_provider === "local"}
                              disabled={!localProvider?.available}
                              onChange={() => handleSetEnhanceProvider("local")}
                              className={RADIO_CLASS}
                            />
                          )}
                        </div>
                      </div>
                      {!model.downloaded && <ModelProgress progress={p} />}
                      {deleteErrors[model.id] && (
                        <span className="text-xs text-destructive">{deleteErrors[model.id]}</span>
                      )}
                      {config?.enhance_provider === "local" && enhanceProviderError && (
                        <span className="text-xs text-destructive">{enhanceProviderError}</span>
                      )}
                    </li>
                  );
                })}
                {enhanceProviders
                  .filter((provider) => provider.id !== "local")
                  .map((provider) => (
                    <li key={provider.id}>
                      <Row
                        label={provider.name}
                        description={
                          provider.available
                            ? provider.version ?? "installed"
                            : `not installed (no \`${provider.id}\` CLI found)`
                        }
                        error={
                          config?.enhance_provider === provider.id ? enhanceProviderError : null
                        }
                      >
                        {provider.available && (
                          <select
                            aria-label={`${provider.name} model`}
                            value={config?.enhance_models?.[provider.id] ?? ""}
                            onChange={(e) => handleSetEnhanceModel(provider.id, e.target.value)}
                            className="h-7 rounded-md border border-input bg-transparent px-2 text-xs outline-none focus-visible:border-ring"
                          >
                            {(ENHANCE_MODEL_OPTIONS[provider.id] ?? [{ value: "", label: "Default" }]).map(
                              (option) => (
                                <option key={option.value} value={option.value}>
                                  {option.label}
                                </option>
                              )
                            )}
                          </select>
                        )}
                        <input
                          type="radio"
                          name="enhance-provider"
                          aria-label={provider.name}
                          checked={config?.enhance_provider === provider.id}
                          disabled={!provider.available}
                          onChange={() => handleSetEnhanceProvider(provider.id)}
                          className={RADIO_CLASS}
                        />
                      </Row>
                    </li>
                  ))}
                {enhanceProviders.length === 0 && (
                  <li>
                    <Row
                      label="Cloud providers not probed yet"
                      description="Reopen Settings to check for the claude and codex CLIs."
                    />
                  </li>
                )}
              </ul>
            </div>
                </section>
              </div>
            </div>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
