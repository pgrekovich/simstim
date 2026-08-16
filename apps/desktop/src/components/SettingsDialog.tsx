import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { Trash2 } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { Label } from "@/components/ui/label";
import { Checkbox } from "@/components/ui/checkbox";
import { cn } from "@/lib/utils";
import {
  deleteModel,
  downloadModel,
  getConfig,
  listModels,
  setActiveModel,
  setAudioFormat,
  setKeepAudio,
  setOutputDir,
  setRetranscribeModel,
  setTheme,
  type AppConfig,
  type DownloadProgress,
  type ModelInfo,
} from "@/lib/ipc";
import { applyTheme } from "@/lib/theme";

const THEME_OPTIONS: { value: string; label: string }[] = [
  { value: "system", label: "System" },
  { value: "light", label: "Light" },
  { value: "dark", label: "Dark" },
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
 * Download/extract/error progress for one model row, shared between the
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
        <Progress value={progress.total > 0 ? (progress.received / progress.total) * 100 : 0} />
        <span className="text-xs text-muted-foreground">
          {humanBytes(progress.received)} / {progress.total > 0 ? humanBytes(progress.total) : "?"}
        </span>
      </div>
    );
  }
  if (progress.phase === "extract") {
    return (
      <div className="flex flex-col gap-1">
        <Progress value={100} className={cn("[&>*]:animate-pulse")} />
        <span className="text-xs text-muted-foreground">extracting...</span>
      </div>
    );
  }
  if (progress.phase === "error") {
    return <span className="text-xs text-destructive">{progress.message}</span>;
  }
  return null;
}

export function SettingsDialog({ open: isOpen, onOpenChange, onModelsChanged }: SettingsDialogProps) {
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [choosingDir, setChoosingDir] = useState(false);
  const [themeError, setThemeError] = useState<string | null>(null);
  const [folderError, setFolderError] = useState<string | null>(null);
  const [keepAudioError, setKeepAudioError] = useState<string | null>(null);
  const [audioFormatError, setAudioFormatError] = useState<string | null>(null);
  const [retranscribeError, setRetranscribeError] = useState<string | null>(null);
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
  }, [isOpen, refresh]);

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
      setProgress((prev) => ({ ...prev, [payload.id]: payload }));
      if (payload.phase === "done") {
        // Refetch (and let the row flip to "Active"/"Use" via the fresh
        // `model.downloaded`) before dropping the progress entry: deleting
        // it first would leave a gap, with `model.downloaded` still stale
        // (false) and no progress entry, where the row briefly renders a
        // "Download" button for an already-finished download.
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
    try {
      await setActiveModel(id);
      await refresh();
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
  // True once a retranscribe model is configured but no longer selectable:
  // either it's not downloaded (e.g. it was just deleted from this same
  // screen), or it's a retired entry that's still on disk (stage 0.8.x
  // curation -- retired rows never render a radio, so a config pointing at
  // one leaves the group with nothing checked too). Every radio in the group
  // reflects `model.downloaded && !model.retired`, so this is the one case
  // where none of them ends up checked and the group would otherwise look
  // broken with no explanation.
  const retranscribeModelMissing =
    Boolean(config?.retranscribe_model) &&
    !models.some((m) => m.id === config?.retranscribe_model && m.downloaded && !m.retired);

  return (
    <Dialog open={isOpen} onOpenChange={onOpenChange}>
      {/* The dialog outgrew a single screen once models arrived, so it is a
          fixed-height column: a static header, then a scrolling body. Without
          the max height the content simply overflowed the window and the top
          and bottom were clipped with no way to reach them. */}
      <DialogContent className="flex max-h-[85vh] flex-col gap-0 overflow-hidden p-0 sm:max-w-xl">
        <DialogHeader className="border-b px-6 py-4">
          <DialogTitle>Settings</DialogTitle>
        </DialogHeader>

        <div className="flex min-h-0 flex-1 flex-col gap-6 overflow-y-auto px-6 py-5">
          <section className="flex flex-col gap-2">
            <h3 className="text-sm font-semibold">Appearance</h3>
            <div className="flex gap-1.5">
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
            </div>
            {themeError && <span className="text-xs text-destructive">{themeError}</span>}
          </section>

          <section className="flex flex-col gap-2">
            <h3 className="text-sm font-semibold">Output</h3>
            <Label>Output folder</Label>
            <div className="flex items-center gap-2">
              <span
                className="min-w-0 flex-1 truncate rounded-md border bg-input/30 px-2 py-1.5 font-mono text-sm"
                style={{ direction: "rtl", textAlign: "left" }}
                title={config?.output_dir ?? ""}
              >
                {config?.output_dir ?? "..."}
              </span>
              <Button variant="outline" size="sm" disabled={choosingDir} onClick={handleChooseFolder}>
                Choose folder
              </Button>
            </div>
            {folderError && <span className="text-xs text-destructive">{folderError}</span>}

            <div className="flex items-start gap-2 pt-2">
              <Checkbox
                id="keep-audio"
                checked={config?.keep_audio ?? false}
                disabled={!config}
                onCheckedChange={(checked) => handleSetKeepAudio(checked === true)}
              />
              <div className="flex flex-col gap-0.5">
                <Label htmlFor="keep-audio">Keep call audio</Label>
                <span className="text-xs text-muted-foreground">
                  Stores a stereo audio file next to each transcript. Re-transcribing needs it.
                </span>
              </div>
            </div>
            {keepAudioError && <span className="text-xs text-destructive">{keepAudioError}</span>}

            <div className="flex flex-col gap-1.5 pt-1">
              <Label>Audio format</Label>
              <div className="flex gap-1.5">
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
              </div>
              <span className="text-xs text-muted-foreground">
                AAC: about 17 MB per hour, small quality loss (default). FLAC: lossless, about 137 MB per hour.
              </span>
              {audioFormatError && <span className="text-xs text-destructive">{audioFormatError}</span>}
            </div>
          </section>

          <section className="flex flex-col gap-3">
            <h3 className="text-sm font-semibold">Models</h3>

            {/* Recording group (stage 0.8): every `role === "live"` entry.
                Parakeet is the only one today and stays the only one a future
                entry could add to without this section changing -- it's
                driven by `role`, not a hardcoded id. Live models never get a
                delete control here (Global Constraints: deleting the model
                that drives recording would break it). */}
            <div className="flex flex-col gap-1.5">
              <div className="flex items-baseline justify-between gap-2">
                <h4 className="text-xs font-semibold tracking-wide text-muted-foreground uppercase">
                  Recording
                </h4>
                <span className="text-xs text-muted-foreground">
                  Recording models: {humanBytes(totalDiskUsed(liveModels))} on disk
                </span>
              </div>
              <ul className="flex flex-col gap-3">
                {liveModels.map((model) => {
                  const p = progress[model.id];
                  const busy = busyIds.has(model.id);
                  return (
                    <li key={model.id} className="flex flex-col gap-1.5 rounded-md border p-3">
                      <div className="flex items-center justify-between gap-3">
                        <div className="flex flex-col">
                          <span className="text-sm font-medium">{model.name}</span>
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

            {/* Re-transcribe group (stage 0.8): every `role === "retranscribe"`
                entry (the whisper family). Radio-style selection among
                downloaded models only reflects `config.retranscribe_model`;
                every downloaded row also gets a delete control to reclaim
                disk. */}
            <div className="flex flex-col gap-1.5">
              <div className="flex items-baseline justify-between gap-2">
                <h4 className="text-xs font-semibold tracking-wide text-muted-foreground uppercase">
                  Re-transcribe
                </h4>
                <span className="text-xs text-muted-foreground">
                  Re-transcribe models: {humanBytes(totalDiskUsed(retranscribeModels))} on disk
                </span>
              </div>
              <span className="text-xs text-muted-foreground">
                Turbo is faster (about 12x realtime measured for this file on this machine).
                Large-v3 is slower but the strongest on non-English speech.
              </span>
              {retranscribeError && <span className="text-xs text-destructive">{retranscribeError}</span>}
              {retranscribeModelMissing && (
                <span className="text-xs text-muted-foreground">
                  Selected model is not downloaded. Pick another or download it again.
                </span>
              )}
              <ul className="flex flex-col gap-3">
                {retranscribeModels.map((model) => {
                  const p = progress[model.id];
                  const busy = busyIds.has(model.id);
                  const confirming = deleteConfirmId === model.id;
                  // Retired (stage 0.8.x curation): this row only exists at
                  // all because its files are still on disk (`list_models`
                  // hides an absent retired entry outright, see
                  // `models::list_models_at`'s docs) -- the owner needs to
                  // see it so it can be deleted, but it can never be
                  // downloaded or selected again. No radio, no tagline, no
                  // Download button; just the name, size, a muted "no longer
                  // offered" note, and delete.
                  return (
                    <li key={model.id} className="flex flex-col gap-1.5 rounded-md border p-3">
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
                              className="size-4 shrink-0 accent-primary"
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
                          {!model.downloaded && !model.retired && (!p || p.phase === "error") && (
                            <Button variant="outline" size="sm" onClick={() => handleDownload(model.id)}>
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
                              onClick={() => handleDeleteClick(model.id)}
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
                                onClick={() => handleConfirmDelete(model.id)}
                              >
                                Delete {humanBytes(freedBytes(model))}?
                              </Button>
                              <Button variant="ghost" size="sm" disabled={busy} onClick={handleCancelDelete}>
                                Cancel
                              </Button>
                            </>
                          )}
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
          </section>
        </div>
      </DialogContent>
    </Dialog>
  );
}
