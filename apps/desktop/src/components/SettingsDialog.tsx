import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { ChevronLeft, Trash2 } from "lucide-react";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { Label } from "@/components/ui/label";
import { Checkbox } from "@/components/ui/checkbox";
import { AiSetup } from "@/components/AiSetup";
import { PromptsSettings } from "@/components/PromptsSettings";
import { cn } from "@/lib/utils";
import {
  deleteModel,
  downloadModel,
  getConfig,
  listEnhanceProviders,
  listModels,
  setAutoStopAfterCall,
  setDetectCalls,
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
  storageStats,
  type AppConfig,
  type DownloadProgress,
  type EnhanceProviderInfo,
  type ModelInfo,
  type StorageStats,
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
 * Formats a GB value with 3 significant figures, trailing zeros trimmed
 * (1.081 -> "1.08 GB", 3.095 -> "3.1 GB") -- matches the decimal-MB numbers
 * quoted in the README's Models section exactly, since both derive from the
 * same "divide by 1000, not 1024" convention.
 */
function formatGb(gb: number): string {
  const decimals = gb >= 10 ? 1 : 2;
  return `${parseFloat(gb.toFixed(decimals))} GB`;
}

/**
 * Formats a byte count as "487 MB" or "1.08 GB" (decimal MB and GB
 * throughout, never 1024). Small values fall through to KB: the Storage
 * section shows the notes' total, which for years of Markdown is still
 * kilobytes -- that contrast with the audio is the point of the row.
 */
function humanBytes(bytes: number): string {
  if (bytes === 0) return "0 MB";
  const mb = bytes / 1_000_000;
  if (mb >= 1000) return formatGb(mb / 1000);
  if (mb >= 1) return `${Math.round(mb)} MB`;
  return `${Math.max(1, Math.round(bytes / 1000))} KB`;
}

/**
 * "96 hours" / "45 minutes" for the Storage audio row: whole units only,
 * it's a feel for scale, not accounting.
 */
function humanHours(secs: number): string {
  const hours = secs / 3600;
  if (hours >= 2) return `${Math.round(hours)} hours`;
  const minutes = Math.round(secs / 60);
  return `${minutes} ${minutes === 1 ? "minute" : "minutes"}`;
}

/** Formats a registry `size_mb` (already in MB) the same way as `humanBytes`. */
function humanSizeMb(sizeMb: number): string {
  if (sizeMb >= 1000) return formatGb(sizeMb / 1000);
  return `${sizeMb} MB`;
}

/**
 * Real bytes a deleting `model` would free: its actual on-disk size when
 * known, falling back to the advertised `size_mb` -- a model should only
 * ever reach the delete control while downloaded (so `size_on_disk_bytes`
 * should be set), but the fallback keeps the confirm copy honest even if
 * that invariant is ever violated by a stale row.
 */
function freedBytes(model: ModelInfo): number {
  return model.size_on_disk_bytes ?? model.size_mb * 1_000_000;
}

/** macOS display convention: the home directory reads as "~". */
function displayPath(path: string): string {
  return path.replace(/^\/Users\/[^/]+/, "~");
}

/** Sum of real on-disk bytes across every currently-downloaded model in `list`. */
function totalDiskUsed(list: ModelInfo[]): number {
  return list.reduce((sum, model) => (model.downloaded ? sum + freedBytes(model) : sum), 0);
}

/**
 * Display the model's role in Storage; retired entries retain their
 * descriptive names.
 *
 */
function storageName(model: ModelInfo): string {
  if (model.retired) return model.name;
  if (model.role === "live") return "Speech recognition (live)";
  if (model.role === "retranscribe") return `Accurate transcript · ${model.name}`;
  return model.name;
}

/** The custom radio input every selection row uses. */
const RADIO_CLASS =
  "size-4 shrink-0 cursor-default appearance-none rounded-full border-[1.5px] border-muted-foreground/60 transition-all checked:border-[5px] checked:border-primary disabled:opacity-40";

/**
 * One settings row: label + helper text on the left, the control pinned
 * right. Rows stack inside a bordered group with hairlines between them.
 */
function Row({
  label,
  htmlFor,
  description,
  error,
  sub,
  dimmed,
  children,
}: {
  label: ReactNode;
  htmlFor?: string;
  description?: ReactNode;
  error?: string | null;
  /**
   * Marks this row as a sub-option of the toggle above it: indented, and
   * expected to be paired with `dimmed` bound to the parent being off. Only
   * used where the dependency is real.
   */
  sub?: boolean;
  /** Fades the whole row; the control itself must also be `disabled`. */
  dimmed?: boolean;
  children?: ReactNode;
}) {
  return (
    <div
      className={cn(
        "flex items-center justify-between gap-6 px-0.5 py-3.5",
        sub && "pl-6",
        dimmed && "opacity-45",
      )}
    >
      <div className="flex min-w-0 flex-col gap-[3px]">
        <Label className={cn("text-[13px]", sub ? "font-normal" : "font-medium")} htmlFor={htmlFor}>{label}</Label>
        {description && <span className="text-[11.5px] leading-[1.45] text-muted-foreground">{description}</span>}
        {error && <span className="text-[11px] text-destructive">{error}</span>}
      </div>
      {children && <div className="flex shrink-0 items-center gap-1.5">{children}</div>}
    </div>
  );
}

/** A bordered group of rows, the section's visual unit. */
function Group({ children }: { children: ReactNode }) {
  return <div className="divide-y divide-border border-y border-border">{children}</div>;
}

/** The mono uppercase heading above a `Group`, with an optional right-aligned figure. */
function GroupHeading({ children, right }: { children: ReactNode; right?: ReactNode }) {
  return (
    <div className="flex items-baseline justify-between gap-2">
      <h4 className="text-[11px] font-semibold text-muted-foreground">
        {children}
      </h4>
      {right && <span className="text-[11px] tabular-nums text-muted-foreground">{right}</span>}
    </div>
  );
}

/** One model's identity in the user's words: name, then a tagline/size line. */
function ModelName({
  name,
  detail,
  mutedName,
}: {
  name: string;
  detail?: ReactNode;
  mutedName?: boolean;
}) {
  return (
    <div className="flex min-w-0 flex-col">
      <span className={cn("truncate text-sm font-medium", mutedName && "text-muted-foreground")}>
        {name}
      </span>
      {detail && <span className="text-xs text-muted-foreground">{detail}</span>}
    </div>
  );
}

interface SettingsDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /**
   * Called whenever fresh config/model data is fetched (on mount, on open,
   * or after a download completes), so the main screen can re-derive its
   * "model not downloaded" gating without waiting for the dialog to close.
   */
  onModelsChanged?: (config: AppConfig, models: ModelInfo[]) => void;
}

/**
 * Download/error progress for one model row. Only rendered while the model
 * isn't downloaded yet -- once `download_model` finishes, `list_models`
 * reports `downloaded: true` and this returns null, which is what lets the
 * row settle into its normal controls instead of a stale progress bar.
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
 * The delete / "Delete N MB?" confirm control cluster a Storage row's right
 * side carries.
 *
 */
function ModelActions({
  model,
  busy,
  confirming,
  onDeleteClick,
  onConfirmDelete,
  onCancelDelete,
}: {
  model: ModelInfo;
  busy: boolean;
  confirming: boolean;
  onDeleteClick: (id: string) => void;
  onConfirmDelete: (id: string) => void;
  onCancelDelete: () => void;
}) {
  return (
    <>
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

/**
 * One settings page for appearance, recording, transcription, optional AI,
 * prompts and storage. Advanced file-only settings are documented
 * separately.
 *
 */
export function SettingsDialog({ open: isOpen, onOpenChange, onModelsChanged }: SettingsDialogProps) {
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [models, setModels] = useState<ModelInfo[]>([]);
  // The Storage figures. Fetched on every open (one cheap pass over the
  // local notes folder); `null` renders "...".
  const [storage, setStorage] = useState<StorageStats | null>(null);
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
  const [detectCallsError, setDetectCallsError] = useState<string | null>(null);
  const [autoStopAfterCallError, setAutoStopAfterCallError] = useState<string | null>(null);
  const [retranscribeError, setRetranscribeError] = useState<string | null>(null);
  const [progress, setProgress] = useState<Record<string, DownloadProgress>>({});
  const [busyIds, setBusyIds] = useState<Set<string>>(new Set());
  // Id of the row currently showing the inline "Delete N MB?" confirm step,
  // at most one at a time. A double-click on the same confirm button just
  // calls `deleteModel` twice, which the backend already treats as a no-op
  // the second time (`remove_model_files` is idempotent).
  const [deleteConfirmId, setDeleteConfirmId] = useState<string | null>(null);
  const [deleteErrors, setDeleteErrors] = useState<Record<string, string>>({});
  const onModelsChangedRef = useRef(onModelsChanged);
  onModelsChangedRef.current = onModelsChanged;
  // Synchronous (ref, not state) guard against a double-click firing
  // `downloadModel` twice before the first progress event or re-render has
  // landed.
  const inFlightDownloadsRef = useRef<Set<string>>(new Set());

  const refresh = useCallback(async () => {
    const [freshConfig, freshModels] = await Promise.all([getConfig(), listModels()]);
    setConfig(freshConfig);
    setModels(freshModels);
    // Disarm pending delete confirmation on refresh so a changed row cannot retain stale
    // approval.
    setDeleteConfirmId(null);
    setDeleteErrors({});
    onModelsChangedRef.current?.(freshConfig, freshModels);
    return freshModels;
  }, []);

  // Initial load: this component is mounted for the app's whole lifetime
  // (App always renders it, visibility is just `open`), so this also primes
  // the main screen's gating state on first paint.
  useEffect(() => {
    refresh().catch(() => {});
  }, [refresh]);

  // Re-sync every time the dialog opens: a download can finish while it was
  // closed; the CLI lineup can change; storage figures move.
  useEffect(() => {
    if (!isOpen) return;
    refresh().catch(() => {});
    listEnhanceProviders()
      .then(setEnhanceProviders)
      .catch(() => {});
    storageStats()
      .then(setStorage)
      .catch(() => {});
  }, [isOpen, refresh]);

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
   * Persists one provider's model override. An empty value clears the
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
 * Select the CLI shared by summaries and both chat views.
 *
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

  // Closing without choosing -- the natural way to back out of an armed
  // delete -- must disarm it immediately.
  useEffect(() => {
    if (isOpen) return;
    setDeleteConfirmId(null);
    setDeleteErrors({});
  }, [isOpen]);

  // Global listener, mounted regardless of `isOpen`: downloads continue in
  // Rust even if the dialog is closed, so progress must keep being tracked
  // and the model list must refetch on completion.
  useEffect(() => {
    const unlisten = listen<DownloadProgress>("model-download-progress", (event) => {
      const payload = event.payload;
      setProgress((prev) => ({
        ...prev,
        [payload.id]: mergeDownloadProgress(prev[payload.id], payload),
      }));
      if (payload.phase === "done") {
        // Refetch before dropping the progress entry: deleting it first would leave a
        // gap where the row ly renders a "Download" button for an already-finished
        // download.
        refresh()
          .catch(() => {})
          .finally(() => {
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
    // Apply immediately so the toggle feels instant; roll both the class and
    // the highlighted button back if the save rejects.
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
    setConfig((prev) => (prev ? { ...prev, keep_audio: keepAudio } : prev));
    try {
      await setKeepAudio(keepAudio);
      await refresh();
    } catch (err) {
      setConfig((prev) => (prev ? { ...prev, keep_audio: previous } : prev));
      setKeepAudioError(String(err));
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

  /**
   * Selects `id` as the accurate-pass model: only ever called for an
   * already-downloaded model (the radio isn't offered otherwise). No
   * optimistic update -- waiting for `refresh()` avoids a flicker back to
   * the old selection if the call rejects.
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
    // A placeholder "download" entry in the same tick as the click, so the
    // Download button disappears immediately rather than after the first
    // real progress event.
    setProgress((prev) => ({
      ...prev,
      [id]: { id, phase: "download", received: 0, total: 0 },
    }));
    try {
      await downloadModel(id);
    } catch (err) {
      const message = String(err);
      // The backend's concurrency-guard rejection means a real download for
      // this id is already in flight; don't clobber its live progress.
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

  async function handleConfirmDelete(id: string) {
    setBusyIds((prev) => new Set(prev).add(id));
    try {
      await deleteModel(id);
      await refresh();
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

  const liveModel = models.find((m) => m.role === "live" && m.active) ?? models.find((m) => m.role === "live");
  const retranscribeModels = models.filter((m) => m.role === "retranscribe" && !m.retired);
  const selectedRetranscribeModel = models.find((m) => m.id === config?.retranscribe_model);
  // The configured accurate model is a retired entry still on disk: distinct
  // from "missing" below, since re-downloading it is impossible.
  const retranscribeModelRetired = Boolean(selectedRetranscribeModel?.retired);
  const retranscribeModelMissing =
    Boolean(config?.retranscribe_model) &&
    !retranscribeModelRetired &&
    !models.some((m) => m.id === config?.retranscribe_model && m.downloaded);
  const availableProviders = enhanceProviders.filter((provider) => provider.available);
  const selectedProvider = enhanceProviders.find((provider) => provider.id === config?.enhance_provider);
  const summariesPossible = availableProviders.length > 0;

  return (
    <Dialog open={isOpen} onOpenChange={onOpenChange}>
      {/* Full-window settings: one centered, scrolling column. */}
      <DialogContent
        showCloseButton={false}
        className="top-0 left-0 flex h-full w-full max-w-none translate-x-0 translate-y-0 flex-col gap-0 rounded-none border-0 p-0 sm:max-w-none"
      >
        {/* Same overlay-title-bar drag strip as the main window: the
            traffic lights float over the left corner, so the back button
            starts after them. */}
        <header data-tauri-drag-region className="flex h-14 shrink-0 items-center justify-between gap-6 pl-[88px] pr-8">
          <div className="flex items-center gap-3">
            <button
              type="button"
              onClick={() => onOpenChange(false)}
              className="flex h-7 items-center gap-1.5 rounded-[7px] px-2 text-left text-[12px] text-quiet-foreground hover:bg-foreground/[0.06] hover:text-foreground"
            >
              <ChevronLeft className="size-3.5" />
              Notes
            </button>
            <DialogTitle className="font-serif text-[17px] font-semibold tracking-[-0.005em]">
              Settings
            </DialogTitle>
          </div>
          <span className="text-[11px] tabular-nums text-muted-foreground">saved automatically</span>
        </header>
        <div className="min-h-0 flex-1 overflow-y-auto">
          <div className="mx-auto flex w-full max-w-[640px] flex-col gap-10 px-8 pb-12 pt-2">
            <section id="settings-general" className="flex flex-col gap-1.5">
              <GroupHeading>General</GroupHeading>
              <Group>
                <Row label="Appearance" error={themeError}>
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
                  description="Off keeps Simstim in the menu bar only."
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
                      className="block max-w-72 truncate"
                      title={config?.output_dir ?? ""}
                    >
                      {config ? displayPath(config.output_dir) : "..."}
                    </span>
                  }
                  error={folderError}
                >
                  <Button variant="outline" size="sm" disabled={choosingDir} onClick={handleChooseFolder}>
                    Change
                  </Button>
                </Row>
              </Group>
            </section>

            {/* Calls: when capture is offered, and whether audio stays. The
                auto-stop sub-row is a real dependency of detection. */}
            <section id="settings-recording" className="flex flex-col gap-1.5">
              <GroupHeading>Calls</GroupHeading>
              <Group>
                <Row
                  label="Suggest recording when a call starts"
                  htmlFor="detect-calls"
                  description="Watches microphone use in known call apps. Recording never starts without your click."
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
                  sub
                  dimmed={!config?.detect_calls}
                  label="Stop when the call ends"
                  htmlFor="auto-stop-after-call"
                  description="Waits for the call app to go quiet."
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
                <Row
                  label="Keep audio recordings"
                  htmlFor="keep-audio"
                  description="Saved next to each note. Needed for playback and the accurate transcript."
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
              </Group>
            </section>

            {/* Transcription: the accurate pass runs on its own after every
                call; the one decision is which model reads the audio. */}
            <section id="settings-transcription" className="flex flex-col gap-1.5">
              <GroupHeading>Accurate transcript</GroupHeading>
              <p className="max-w-[62ch] text-[11.5px] leading-[1.55] text-muted-foreground">
                The live transcript follows the call as it happens. A few minutes after a call
                ends, Simstim re-reads the saved audio with a bigger model: cleaner wording,
                better names and numbers, much stronger non-English speech.
                {config && !config.keep_audio && ' Needs "Keep audio recordings" on.'}
              </p>
              {retranscribeError && <span className="text-xs text-destructive">{retranscribeError}</span>}
              {retranscribeModelRetired && (
                <span className="text-xs text-muted-foreground">
                  The selected model is no longer offered. Pick another.
                </span>
              )}
              {retranscribeModelMissing && (
                <span className="text-xs text-muted-foreground">
                  The selected model is not downloaded. Pick another or download it again.
                </span>
              )}
              <ul className="flex flex-col divide-y divide-border border-y border-border">
                {retranscribeModels.map((model) => {
                  const p = progress[model.id];
                  const busy = busyIds.has(model.id);
                  return (
                    <li key={model.id} className="flex flex-col gap-2 px-0.5 py-3.5">
                      <div className="flex items-center justify-between gap-3">
                        <div className="flex min-w-0 items-center gap-2.5">
                          {model.downloaded && (
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
                          <ModelName
                            name={model.name}
                            detail={
                              model.tagline
                                ? `${model.tagline} · ${humanSizeMb(model.size_mb)}`
                                : humanSizeMb(model.size_mb)
                            }
                          />
                        </div>
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
                {/* The live model only surfaces while it is missing: there
                    is nothing to configure about it once it's on disk. */}
                {liveModel && !liveModel.downloaded && (
                  <li className="flex flex-col gap-2 px-0.5 py-3.5">
                    <div className="flex items-center justify-between gap-3">
                      <ModelName
                        name="Live speech recognition"
                        detail={`Needed to record · ${humanSizeMb(liveModel.size_mb)}`}
                      />
                      {(!progress[liveModel.id] || progress[liveModel.id].phase === "error") && (
                        <Button variant="outline" size="sm" onClick={() => handleDownload(liveModel.id)}>
                          Download
                        </Button>
                      )}
                    </div>
                    <ModelProgress progress={progress[liveModel.id]} />
                  </li>
                )}
              </ul>
            </section>

            {/* Summaries: who writes them, and what they may touch. */}
            <section id="settings-summaries" className="flex flex-col gap-1.5">
              <GroupHeading>Summaries</GroupHeading>
              <p className="max-w-[62ch] text-[11.5px] leading-[1.55] text-muted-foreground">
                Use Claude or ChatGPT with your existing account. Summaries and
                questions send text to that provider, never audio. Your provider's usage
                limits apply. Turn off automatic summaries to keep text local; your files
                also work with any assistant you use separately.
              </p>
              {isOpen && <AiSetup providers={enhanceProviders} onConnected={async (provider) => {
                const fresh = await listEnhanceProviders();
                setEnhanceProviders(fresh);
                if (fresh.some((p) => p.id === provider && p.available)) {
                  await handleSetEnhanceProvider(provider);
                }
              }} />}
              <Group>
                {!summariesPossible ? (
                  <Row
                    label="Connect an AI assistant"
                    description="Recording and transcription also work without AI."
                  />
                ) : availableProviders.length === 1 ? (
                  <Row
                    label={`Written by ${availableProviders[0].name}`}
                    description={(availableProviders[0].version ?? "installed").replace(
                      ` (${availableProviders[0].name})`,
                      "",
                    )}
                    error={enhanceProviderError}
                  >
                    <select
                      aria-label={`${availableProviders[0].name} model`}
                      value={config?.enhance_models?.[availableProviders[0].id] ?? ""}
                      onChange={(e) => handleSetEnhanceModel(availableProviders[0].id, e.target.value)}
                      className="h-7 rounded-md border border-input bg-transparent px-2 text-xs outline-none focus-visible:border-ring"
                    >
                      {(ENHANCE_MODEL_OPTIONS[availableProviders[0].id] ?? [{ value: "", label: "Default" }]).map(
                        (option) => (
                          <option key={option.value} value={option.value}>
                            {option.label}
                          </option>
                        )
                      )}
                    </select>
                  </Row>
                ) : (
                  availableProviders.map((provider) => (
                    <Row
                      key={provider.id}
                      label={provider.name}
                      description={(provider.version ?? "installed").replace(` (${provider.name})`, "")}
                      error={config?.enhance_provider === provider.id ? enhanceProviderError : null}
                    >
                      {config?.enhance_provider === provider.id && (
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
                        onChange={() => handleSetEnhanceProvider(provider.id)}
                        className={RADIO_CLASS}
                      />
                    </Row>
                  ))
                )}
                <Row
                  label="Summarize after each call"
                  htmlFor="auto-enhance"
                  description="Decisions and action items, written below your notes."
                  error={autoEnhanceError}
                  dimmed={!summariesPossible}
                >
                  <Checkbox
                    variant="switch"
                    id="auto-enhance"
                    checked={config?.auto_enhance ?? false}
                    disabled={!config || !summariesPossible}
                    onCheckedChange={(checked) => handleSetAutoEnhance(checked === true)}
                  />
                </Row>
                <Row
                  label="Let summaries title notes"
                  htmlFor="enhance-rename-title"
                  description={'Only titles Simstim made up ("Untitled", "Zoom call 14:03"). Yours are never touched.'}
                  error={renameTitleError}
                  dimmed={!summariesPossible}
                >
                  <Checkbox
                    variant="switch"
                    id="enhance-rename-title"
                    checked={config?.enhance_rename_title ?? false}
                    disabled={!config || !summariesPossible}
                    onCheckedChange={(checked) => handleSetEnhanceRenameTitle(checked === true)}
                  />
                </Row>
                <Row
                  label="Use calendar for context"
                  htmlFor="enhance-use-calendar"
                  description="Names the meeting and its people from the overlapping calendar event, through your Claude connectors."
                  error={useCalendarError}
                  dimmed={!summariesPossible}
                >
                  <Checkbox
                    variant="switch"
                    id="enhance-use-calendar"
                    checked={config?.enhance_use_calendar ?? false}
                    disabled={!config || !summariesPossible}
                    onCheckedChange={(checked) => handleSetEnhanceUseCalendar(checked === true)}
                  />
                </Row>
              </Group>
              {selectedProvider && !selectedProvider.available && summariesPossible && (
                <span className="text-xs text-muted-foreground">
                  {selectedProvider.name} is not installed anymore; pick the assistant above.
                </span>
              )}
            </section>

            {/*
 * Prompt settings edit and save prompts.toml.
 *
 */}
            <PromptsSettings open={isOpen} />

            {/* Storage: the user's own data first (kilobytes of Markdown next
                to gigabytes of audio is the point), then the models, which
                is also where deletion lives. */}
            <section id="settings-storage" className="flex flex-col gap-7">
              <div className="flex flex-col gap-1.5">
                <GroupHeading
                  right={storage ? humanBytes(storage.notes_bytes + storage.audio_bytes) : null}
                >
                  Your calls
                </GroupHeading>
                <Group>
                  <Row
                    label="Notes"
                    description={
                      storage
                        ? `${storage.note_count} ${storage.note_count === 1 ? "call" : "calls"}, one Markdown file each`
                        : "..."
                    }
                  >
                    <span className="text-[11px] tabular-nums text-muted-foreground">
                      {storage ? humanBytes(storage.notes_bytes) : "..."}
                    </span>
                  </Row>
                  <Row
                    label="Call audio"
                    description={
                      storage
                        ? storage.audio_bytes > 0
                          ? humanHours(storage.audio_secs)
                          : "No audio kept"
                        : "..."
                    }
                  >
                    <span className="text-[11px] tabular-nums text-muted-foreground">
                      {storage ? humanBytes(storage.audio_bytes) : "..."}
                    </span>
                  </Row>
                </Group>
                <p className="text-[11px] leading-[1.5] text-muted-foreground">
                  Everything above lives in{" "}
                  <span>{config ? displayPath(config.output_dir) : "the notes folder"}</span>{" "}
                  and is yours to move, sync or delete.
                </p>
              </div>

              <div className="flex flex-col gap-1.5">
                <GroupHeading right={humanBytes(totalDiskUsed(models))}>Speech models</GroupHeading>
                <ul className="flex flex-col divide-y divide-border border-y border-border">
                  {models.filter((model) => model.downloaded).map((model) => {
                    const busy = busyIds.has(model.id);
                    return (
                      <li key={model.id} className="flex flex-col gap-2 px-0.5 py-3.5">
                        <div className="flex items-center justify-between gap-3">
                          <ModelName
                            name={storageName(model)}
                            detail={model.retired ? "(no longer offered)" : undefined}
                            mutedName={model.retired}
                          />
                          <div className="flex shrink-0 items-center gap-1.5">
                            <span className="text-[11px] tabular-nums text-muted-foreground">
                              {humanBytes(freedBytes(model))}
                            </span>
                            {/* The live model never offers deletion: it is
                                what records the call. */}
                            {model.role !== "live" && (
                              <ModelActions
                                model={model}
                                busy={busy}
                                confirming={deleteConfirmId === model.id}
                                onDeleteClick={handleDeleteClick}
                                onConfirmDelete={handleConfirmDelete}
                                onCancelDelete={handleCancelDelete}
                              />
                            )}
                          </div>
                        </div>
                        {deleteErrors[model.id] && (
                          <span className="text-xs text-destructive">{deleteErrors[model.id]}</span>
                        )}
                      </li>
                    );
                  })}
                  {!models.some((model) => model.downloaded) && (
                    <li>
                      <Row
                        label="No models on disk yet"
                        description="They download on their own the first time they are needed."
                      />
                    </li>
                  )}
                </ul>
              </div>
            </section>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
