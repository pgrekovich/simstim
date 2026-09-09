import { useEffect, useRef, useState, type ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import {
  ArrowRight,
  Check,
  FolderOpen,
  LoaderCircle,
  Mic,
  RotateCcw,
  Settings,
  Volume2,
} from "lucide-react";
import { AiSetup } from "@/components/AiSetup";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Progress } from "@/components/ui/progress";
import { mergeDownloadProgress } from "@/lib/modelProgress";
import {
  checkAudioPermissions,
  completeOnboarding,
  downloadModel,
  getConfig,
  listEnhanceProviders,
  listModels,
  setAutoEnhance,
  setEnhanceProvider,
  setOutputDir,
  type AppConfig,
  type DownloadProgress,
  type EnhanceProviderInfo,
  type ModelInfo,
  type PermissionCheck,
  type PermissionCheckResult,
} from "@/lib/ipc";

interface OnboardingProps {
  config: AppConfig;
  models: ModelInfo[];
  onOpenSettings: () => void;
  onComplete: (config: AppConfig, models: ModelInfo[]) => void;
}

function PermissionResult({
  label,
  detail,
  icon,
  result,
  checking,
}: {
  label: string;
  detail: string;
  icon: ReactNode;
  result?: PermissionCheck;
  checking: boolean;
}) {
  const passed = result?.granted === true;
  return (
    <div className="flex min-h-24 items-center gap-4 rounded-[10px] border border-border bg-card/55 px-4 py-3.5">
      <div
        className={`flex size-9 shrink-0 items-center justify-center rounded-full border ${
          passed
            ? "border-success/30 bg-success/10 text-success"
            : "border-border bg-background/70 text-muted-foreground"
        }`}
      >
        {passed ? <Check className="size-4" /> : icon}
      </div>
      <div className="min-w-0 flex-1">
        <p className="text-[14px] font-semibold">{label}</p>
        <p className="mt-0.5 text-[12px] leading-[1.45] text-muted-foreground">
          {checking
            ? "Requesting access..."
            : passed
              ? "Allowed"
              : result
                ? result.error ? "Could not request access" : "Needs permission"
                : detail}
        </p>
      </div>
    </div>
  );
}

/**
 * The speech models the app needs, downloaded here without ceremony: the
 * live one (required before the first recording) and the accurate pass's
 * default whisper model (the automatic pass after each call needs it; it
 * may still be downloading when onboarding finishes, which is fine -- the
 * pass skips quietly until it lands). Retired entries are never touched.
 */
function modelsToFetch(models: ModelInfo[], config: AppConfig): ModelInfo[] {
  return models.filter(
    (model) =>
      !model.retired &&
      !model.downloaded &&
      (model.id === config.active_model || model.id === config.retranscribe_model),
  );
}

/** macOS display convention: the home directory reads as "~". */
function displayPath(path: string): string {
  return path.replace(/^\/Users\/[^/]+/, "~");
}

/** One number for every in-flight download: bytes received over bytes total. */
function combinedPercent(progress: Record<string, DownloadProgress>): number | null {
  const entries = Object.values(progress).filter((p) => p.phase !== "error");
  if (entries.length === 0) return null;
  const total = entries.reduce((sum, p) => sum + p.total, 0);
  if (total === 0) return 0;
  const received = entries.reduce((sum, p) => sum + p.received, 0);
  return Math.min(100, Math.round((received / total) * 100));
}

/**
 * One screen (2026-09-04 product ruling: the old four steps asked a new user
 * to configure an AI stack before recording anything). The promise, the
 * folder, the two macOS permissions, and one honest line about summaries.
 * The speech models start downloading the moment the screen appears.
 */
export function Onboarding({ config, models, onOpenSettings, onComplete }: OnboardingProps) {
  const [permissionResult, setPermissionResult] = useState<PermissionCheckResult | null>(null);
  const [checking, setChecking] = useState(false);
  const [choosingFolder, setChoosingFolder] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [progress, setProgress] = useState<Record<string, DownloadProgress>>({});
  const [currentConfig, setCurrentConfig] = useState(config);
  const [currentModels, setCurrentModels] = useState(models);
  const [enhanceProviders, setEnhanceProviders] = useState<EnhanceProviderInfo[] | null>(null);
  const [finishing, setFinishing] = useState(false);
  const downloadStarted = useRef(new Set<string>());
  const permissionsChecked = useRef(false);

  useEffect(() => setCurrentConfig(config), [config]);
  useEffect(() => setCurrentModels(models), [models]);

  useEffect(() => {
    listEnhanceProviders().then(setEnhanceProviders).catch(() => setEnhanceProviders([]));
  }, []);

  // macOS shows its permission prompts as soon as the screen appears; the
  // user is already here, reading why.
  useEffect(() => {
    if (permissionsChecked.current) return;
    permissionsChecked.current = true;
    void handlePermissionCheck();
  }, []);

  async function refreshModels() {
    const [nextConfig, nextModels] = await Promise.all([getConfig(), listModels()]);
    setCurrentConfig(nextConfig);
    setCurrentModels(nextModels);
    return [nextConfig, nextModels] as const;
  }

  useEffect(() => {
    const unlisten = listen<DownloadProgress>("model-download-progress", (event) => {
      setProgress((previous) => ({
        ...previous,
        [event.payload.id]: mergeDownloadProgress(previous[event.payload.id], event.payload),
      }));
      if (event.payload.phase === "done") {
        void refreshModels().finally(() => {
          setProgress((previous) => {
            const next = { ...previous };
            delete next[event.payload.id];
            return next;
          });
          downloadStarted.current.delete(event.payload.id);
        });
      }
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  const activeModel = currentModels.find((model) => model.id === currentConfig.active_model);
  const permissionsPassed = Boolean(
    permissionResult?.microphone.granted && permissionResult.system_audio.granted,
  );
  // The first installed CLI wins; the user's saved choice wins over that.
  const summaryProvider =
    enhanceProviders?.find(
      (provider) => provider.available && provider.id === currentConfig.enhance_provider,
    ) ?? enhanceProviders?.find((provider) => provider.available);
  const downloadPercent = combinedPercent(progress);
  const failed = Object.values(progress).find((p) => p.phase === "error");

  // Downloads start on their own, once the model list is known.
  useEffect(() => {
    for (const model of modelsToFetch(currentModels, currentConfig)) {
      void handleDownload(model);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentModels, currentConfig.active_model, currentConfig.retranscribe_model]);

  // The saved provider may not be the one installed here (a fresh config
  // says "claude" on a Mac that only has codex): follow what's installed.
  useEffect(() => {
    if (!summaryProvider || summaryProvider.id === currentConfig.enhance_provider) return;
    void setEnhanceProvider(summaryProvider.id)
      .then(() => setCurrentConfig((value) => ({ ...value, enhance_provider: summaryProvider.id })))
      .catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [summaryProvider?.id]);

  async function handlePermissionCheck() {
    if (checking) return;
    setChecking(true);
    setError(null);
    setPermissionResult(null);
    try {
      setPermissionResult(await checkAudioPermissions());
    } catch (err) {
      setError(String(err));
    } finally {
      setChecking(false);
    }
  }

  async function handleChooseFolder() {
    if (choosingFolder) return;
    setChoosingFolder(true);
    setError(null);
    try {
      const selected = await open({ directory: true });
      if (typeof selected === "string") {
        await setOutputDir(selected);
        setCurrentConfig(await getConfig());
      }
    } catch (err) {
      setError(String(err));
    } finally {
      setChoosingFolder(false);
    }
  }

  async function handleDownload(model: ModelInfo) {
    if (downloadStarted.current.has(model.id)) return;
    downloadStarted.current.add(model.id);
    setProgress((previous) => ({
      ...previous,
      [model.id]: { id: model.id, phase: "download", received: 0, total: 0 },
    }));
    try {
      await downloadModel(model.id);
    } catch (err) {
      const message = String(err);
      if (!message.includes("already downloading")) {
        setProgress((previous) => ({
          ...previous,
          [model.id]: { id: model.id, phase: "error", received: 0, total: 0, message },
        }));
        downloadStarted.current.delete(model.id);
      }
    }
  }

  function handleRetryDownloads() {
    setProgress((previous) =>
      Object.fromEntries(Object.entries(previous).filter(([, p]) => p.phase !== "error")),
    );
    for (const model of modelsToFetch(currentModels, currentConfig)) {
      void handleDownload(model);
    }
  }

  async function handleToggleSummaries(enabled: boolean) {
    const previous = currentConfig;
    setError(null);
    setCurrentConfig((value) => ({ ...value, auto_enhance: enabled }));
    try {
      await setAutoEnhance(enabled);
    } catch (err) {
      setCurrentConfig(previous);
      setError(String(err));
    }
  }

  async function handleFinish() {
    if (!activeModel?.downloaded || !permissionsPassed || finishing) return;
    setFinishing(true);
    setError(null);
    try {
      await completeOnboarding();
      const [nextConfig, nextModels] = await refreshModels();
      onComplete(nextConfig, nextModels);
    } catch (err) {
      setError(String(err));
      setFinishing(false);
    }
  }

  const ready = permissionsPassed && Boolean(activeModel?.downloaded);

  return (
    <main className="relative flex h-screen min-w-0 flex-1 flex-col bg-background text-foreground">
      <div data-tauri-drag-region className="absolute inset-x-0 top-0 h-11" />
      <div className="relative z-10 flex h-14 shrink-0 items-center justify-end px-5">
        <Button variant="ghost" size="sm" onClick={onOpenSettings}>
          <Settings className="size-3.5" />
          Settings
        </Button>
      </div>

      <div className="flex min-h-0 flex-1 justify-center overflow-y-auto px-8 pb-10 pt-4">
        <section className="my-auto w-full max-w-[620px]">
          <h1 className="font-serif text-[34px] font-semibold leading-[1.12] tracking-[-0.025em]">
            Your audio stays on this Mac.
          </h1>
          <p className="mt-4 text-[15px] leading-[1.65] text-muted-foreground">
            Simstim records only when you click Record. Every call becomes a plain Markdown
            note, with its audio, in a folder you own. Audio is never uploaded.
          </p>

          <div className="mt-7 flex items-center gap-4 rounded-[10px] border border-border bg-card/50 px-4 py-3">
            <div className="min-w-0 flex-1">
              <p className="text-[11px] font-semibold text-muted-foreground">
                Notes folder
              </p>
              <p className="mt-1 truncate text-[13px]" title={currentConfig.output_dir}>
                {displayPath(currentConfig.output_dir)}
              </p>
            </div>
            <Button
              variant="outline"
              size="sm"
              onClick={() => void handleChooseFolder()}
              disabled={choosingFolder}
            >
              {choosingFolder ? (
                <LoaderCircle className="size-3.5 animate-spin" />
              ) : (
                <FolderOpen className="size-3.5" />
              )}
              Choose
            </Button>
          </div>

          <div className="mt-6 grid grid-cols-2 gap-3">
            <PermissionResult
              label="Microphone"
              detail="Waiting for macOS"
              icon={<Mic className="size-4" />}
              result={permissionResult?.microphone}
              checking={checking}
            />
            <PermissionResult
              label="System audio"
              detail="Waiting for macOS"
              icon={<Volume2 className="size-4" />}
              result={permissionResult?.system_audio}
              checking={checking}
            />
          </div>
          {permissionResult && !permissionsPassed && (
            <div className="mt-3 flex items-center justify-between gap-3">
              <p className="text-[12px] leading-relaxed text-muted-foreground">
                Allow both in System Settings, then check again. Simstim never records here.
              </p>
              <Button
                variant="ghost"
                size="sm"
                onClick={() => void handlePermissionCheck()}
                disabled={checking}
                className="shrink-0 text-muted-foreground"
              >
                {checking ? (
                  <><LoaderCircle className="size-4 animate-spin" /> Checking...</>
                ) : (
                  <><RotateCcw className="size-3.5" /> Check again</>
                )}
              </Button>
            </div>
          )}

          {/* Summaries: the one choice that involves text leaving the Mac,
              so it is a visible switch here rather than a buried default. */}
          <div className="mt-6 rounded-[10px] border border-border bg-card/40 px-4 py-3">
            {enhanceProviders === null ? (
              <p className="text-[12.5px] text-muted-foreground">Looking for an AI assistant on this Mac...</p>
            ) : summaryProvider ? (
              <label htmlFor="onboarding-summaries" className="flex items-center justify-between gap-4">
                <span>
                  <span className="block text-[13px] font-medium">
                    Summaries by {summaryProvider.name}
                  </span>
                  <span className="mt-0.5 block text-[11.5px] leading-[1.5] text-muted-foreground">
                    After each call, {summaryProvider.name} writes decisions and action items
                    by sending the note's text to your AI provider. Never audio. Turn this off to keep text local.
                  </span>
                </span>
                <Checkbox
                  id="onboarding-summaries"
                  variant="switch"
                  checked={currentConfig.auto_enhance}
                  onCheckedChange={(checked) => void handleToggleSummaries(checked === true)}
                />
              </label>
            ) : (
              <AiSetup providers={enhanceProviders} onConnected={async (provider) => {
                const fresh = await listEnhanceProviders();
                if (fresh.some((p) => p.id === provider && p.available)) {
                  await setEnhanceProvider(provider);
                  setCurrentConfig((cfg) => ({ ...cfg, enhance_provider: provider }));
                }
                setEnhanceProviders(fresh);
              }} />
            )}
          </div>

          {error && <p role="alert" className="mt-3 text-[12px] text-destructive">{error}</p>}

          {/* Footer: the speech models' one quiet status line on the left (no
              names, no sizes), Start on the right -- one row, so neither
              floats alone. */}
          <div className="mt-8 flex items-center justify-between gap-6">
            <div className="min-w-0 flex-1">
            {activeModel?.downloaded && downloadPercent === null ? (
              <p className="flex items-center gap-1.5 text-[12px] text-muted-foreground">
                <Check className="size-3.5 text-success" /> Speech recognition ready
              </p>
            ) : failed ? (
              <div className="flex items-center justify-between gap-3">
                <p role="alert" className="text-[12px] text-destructive">
                  Could not download the speech models. {failed.message}
                </p>
                <Button variant="outline" size="sm" onClick={handleRetryDownloads}>
                  <RotateCcw className="size-3.5" /> Retry
                </Button>
              </div>
            ) : (
              <div>
                <p className="flex items-center gap-1.5 text-[12px] text-muted-foreground">
                  <LoaderCircle className="size-3.5 animate-spin" />
                  Preparing speech recognition{downloadPercent !== null && downloadPercent > 0 ? ` · ${downloadPercent}%` : "..."}
                </p>
                {downloadPercent !== null && <Progress className="mt-2 max-w-[320px]" value={downloadPercent} />}
              </div>
            )}
          </div>
            <Button onClick={() => void handleFinish()} disabled={!ready || finishing}>
              {finishing ? (
                <><LoaderCircle className="size-4 animate-spin" /> Finishing...</>
              ) : (
                <>Start <ArrowRight className="size-4" /></>
              )}
            </Button>
          </div>
        </section>
      </div>
    </main>
  );
}
