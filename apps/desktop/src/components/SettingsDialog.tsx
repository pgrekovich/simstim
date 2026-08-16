import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
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
  downloadModel,
  getConfig,
  listModels,
  setActiveModel,
  setAudioFormat,
  setKeepAudio,
  setOutputDir,
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

/** Formats a byte count as "487 MB" or "1.2 GB" (one decimal above 1024 MB). */
function humanBytes(bytes: number): string {
  const mb = bytes / 1_000_000;
  if (mb >= 1024) return `${(mb / 1024).toFixed(1)} GB`;
  return `${Math.round(mb)} MB`;
}

/** Formats a registry `size_mb` (already in MB) the same way as `humanBytes`. */
function humanSizeMb(sizeMb: number): string {
  if (sizeMb >= 1024) return `${(sizeMb / 1024).toFixed(1)} GB`;
  return `${sizeMb} MB`;
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

export function SettingsDialog({ open: isOpen, onOpenChange, onModelsChanged }: SettingsDialogProps) {
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [choosingDir, setChoosingDir] = useState(false);
  const [themeError, setThemeError] = useState<string | null>(null);
  const [folderError, setFolderError] = useState<string | null>(null);
  const [keepAudioError, setKeepAudioError] = useState<string | null>(null);
  const [audioFormatError, setAudioFormatError] = useState<string | null>(null);
  const [progress, setProgress] = useState<Record<string, DownloadProgress>>({});
  const [busyIds, setBusyIds] = useState<Set<string>>(new Set());
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
  useEffect(() => {
    if (!isOpen) return;
    refresh().catch(() => {});
  }, [isOpen, refresh]);

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

  return (
    <Dialog open={isOpen} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>Settings</DialogTitle>
        </DialogHeader>

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

        <section className="flex flex-col gap-2">
          <h3 className="text-sm font-semibold">Models</h3>
          <ul className="flex flex-col gap-3">
            {models.map((model) => {
              const p = progress[model.id];
              const busy = busyIds.has(model.id);
              return (
                <li key={model.id} className="flex flex-col gap-1.5 rounded-md border p-3">
                  <div className="flex items-center justify-between gap-3">
                    <div className="flex flex-col">
                      <span className="text-sm font-medium">{model.name}</span>
                      <span className="text-xs text-muted-foreground">{humanSizeMb(model.size_mb)}</span>
                    </div>
                    {/* Whisper (selectable: false) is a re-transcribe-only engine: it
                        never offers Use/Active, just a plain downloaded marker. Driven
                        by the registry's `selectable` field, not a hardcoded model id. */}
                    {model.downloaded && model.selectable && model.active && (
                      <span className="text-sm text-muted-foreground">Active</span>
                    )}
                    {model.downloaded && model.selectable && !model.active && (
                      <Button variant="outline" size="sm" disabled={busy} onClick={() => handleUse(model.id)}>
                        Use
                      </Button>
                    )}
                    {model.downloaded && !model.selectable && (
                      <span className="text-sm text-muted-foreground">Downloaded</span>
                    )}
                    {!model.downloaded && (!p || p.phase === "error") && (
                      <Button variant="outline" size="sm" onClick={() => handleDownload(model.id)}>
                        Download
                      </Button>
                    )}
                  </div>

                  {!model.downloaded && p && p.phase === "download" && (
                    <div className="flex flex-col gap-1">
                      <Progress value={p.total > 0 ? (p.received / p.total) * 100 : 0} />
                      <span className="text-xs text-muted-foreground">
                        {humanBytes(p.received)} / {p.total > 0 ? humanBytes(p.total) : "?"}
                      </span>
                    </div>
                  )}

                  {!model.downloaded && p && p.phase === "extract" && (
                    <div className="flex flex-col gap-1">
                      <Progress value={100} className={cn("[&>*]:animate-pulse")} />
                      <span className="text-xs text-muted-foreground">extracting...</span>
                    </div>
                  )}

                  {!model.downloaded && p && p.phase === "error" && (
                    <span className="text-xs text-destructive">{p.message}</span>
                  )}
                </li>
              );
            })}
          </ul>
        </section>
      </DialogContent>
    </Dialog>
  );
}
