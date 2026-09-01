import { useEffect, useRef, useState, type ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import {
  ArrowRight,
  Check,
  Download,
  FolderOpen,
  LoaderCircle,
  Mic,
  RotateCcw,
  Settings,
  ShieldCheck,
  Sparkles,
  Volume2,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Progress } from "@/components/ui/progress";
import { ENHANCE_MODEL_OPTIONS } from "@/lib/enhanceModels";
import { downloadProgressPercent, mergeDownloadProgress } from "@/lib/modelProgress";
import {
  checkAudioPermissions,
  completeOnboarding,
  downloadModel,
  getConfig,
  listEnhanceProviders,
  listModels,
  setAutoEnhance,
  setEnhanceModel,
  setEnhanceProvider,
  setEnhanceRenameTitle,
  setEnhanceUseCalendar,
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
            ? "border-emerald-600/30 bg-emerald-600/10 text-emerald-700 dark:text-emerald-400"
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

function humanSize(sizeMb: number): string {
  return sizeMb >= 1024 ? `${(sizeMb / 1024).toFixed(1)} GB` : `${sizeMb} MB`;
}

export function Onboarding({ config, models, onOpenSettings, onComplete }: OnboardingProps) {
  const [step, setStep] = useState(0);
  const [permissionResult, setPermissionResult] = useState<PermissionCheckResult | null>(null);
  const [checking, setChecking] = useState(false);
  const [choosingFolder, setChoosingFolder] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [progress, setProgress] = useState<Record<string, DownloadProgress>>({});
  const [currentConfig, setCurrentConfig] = useState(config);
  const [currentModels, setCurrentModels] = useState(models);
  const [enhanceProviders, setEnhanceProviders] = useState<EnhanceProviderInfo[]>([]);
  const [finishing, setFinishing] = useState(false);
  const downloadStarted = useRef(new Set<string>());
  const permissionStepEntered = useRef(false);
  const transcriptionStepEntered = useRef(false);

  useEffect(() => setCurrentConfig(config), [config]);
  useEffect(() => setCurrentModels(models), [models]);
  useEffect(() => {
    if (step !== 3) return;
    listEnhanceProviders().then(setEnhanceProviders).catch(() => setEnhanceProviders([]));
  }, [step, config]);

  useEffect(() => {
    if (step !== 1) {
      permissionStepEntered.current = false;
      return;
    }
    if (permissionStepEntered.current) return;
    permissionStepEntered.current = true;
    void handlePermissionCheck();
  }, [step]);

  async function refreshModels() {
    const [nextConfig, nextModels] = await Promise.all([getConfig(), listModels()]);
    setCurrentConfig(nextConfig);
    setCurrentModels(nextModels);
    return [nextConfig, nextModels] as const;
  }

  useEffect(() => {
    const unlisten = listen<DownloadProgress>("model-download-progress", (event) => {
      const tracked = currentModels.some(
        (model) => model.id === event.payload.id && (model.role === "live" || model.role === "summary"),
      );
      if (!tracked) return;
      setProgress((previous) => ({
        ...previous,
        [event.payload.id]: mergeDownloadProgress(previous[event.payload.id], event.payload),
      }));
      if (event.payload.phase === "done") {
        void Promise.all([refreshModels(), listEnhanceProviders().then(setEnhanceProviders)]).finally(() => {
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
  }, [currentModels]);

  const activeModel = currentModels.find((model) => model.id === currentConfig.active_model);
  const summaryModel = currentModels.find((model) => model.role === "summary" && !model.retired);
  const activeProgress = activeModel ? progress[activeModel.id] : undefined;
  const summaryProgress = summaryModel ? progress[summaryModel.id] : undefined;
  const selectedProvider = enhanceProviders.find(
    (provider) => provider.id === currentConfig.enhance_provider,
  );
  const permissionsPassed = Boolean(
    permissionResult?.microphone.granted && permissionResult.system_audio.granted,
  );

  useEffect(() => {
    if (step !== 2) {
      transcriptionStepEntered.current = false;
      return;
    }
    if (transcriptionStepEntered.current || !activeModel || activeModel.downloaded) return;
    transcriptionStepEntered.current = true;
    void handleDownload(activeModel);
  }, [step, activeModel]);

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
    setError(null);
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

  async function updateSummaryConfig(
    patch: Partial<AppConfig>,
    persist: () => Promise<void>,
  ) {
    const previous = currentConfig;
    setError(null);
    setCurrentConfig((value) => ({ ...value, ...patch }));
    try {
      await persist();
      setCurrentConfig(await getConfig());
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

  return (
    <main className="relative flex h-screen min-w-0 flex-1 flex-col bg-background text-foreground">
      <div data-tauri-drag-region className="absolute inset-x-0 top-0 h-11" />
      <div className="relative z-10 flex h-14 shrink-0 items-center justify-end px-5">
        <Button variant="ghost" size="sm" onClick={onOpenSettings}>
          <Settings className="size-3.5" />
          Settings
        </Button>
      </div>

      <div className="flex min-h-0 flex-1 justify-center overflow-y-auto px-8 pb-10 pt-8">
        <section className="my-auto w-full max-w-[620px]">
          <div className="mb-8 flex items-center gap-2 font-mono text-[10.5px] uppercase tracking-[0.09em] text-muted-foreground">
            {[0, 1, 2, 3].map((index) => (
              <div key={index} className="flex items-center gap-2">
                <span
                  className={`flex size-5 items-center justify-center rounded-full border ${
                    index <= step ? "border-foreground/45 text-foreground" : "border-border"
                  }`}
                >
                  {index < step ? <Check className="size-3" /> : index + 1}
                </span>
                {index < 3 && <span className="h-px w-8 bg-border" />}
              </div>
            ))}
          </div>

          {step === 0 && (
            <div>
              <div className="mb-6 flex size-11 items-center justify-center rounded-full border border-border bg-card text-voice-remote-foreground">
                <ShieldCheck className="size-5" />
              </div>
              <h1 className="font-serif text-[34px] font-semibold leading-[1.12] tracking-[-0.025em]">
                Your calls stay on this Mac.
              </h1>
              <p className="mt-4 max-w-[540px] text-[15px] leading-[1.65] text-muted-foreground">
                Simstim records only when you click Record. Audio, transcripts and notes stay in
                your folder. Next, macOS will ask for the two audio permissions Simstim needs,
                then we will download the local recording model.
              </p>
              <div className="mt-7 flex items-center gap-4 rounded-[10px] border border-border bg-card/50 px-4 py-3">
                <div className="min-w-0 flex-1">
                  <p className="font-mono text-[10px] uppercase tracking-[0.07em] text-muted-foreground">
                    Notes folder
                  </p>
                  <p className="mt-1 truncate text-[13px]">{currentConfig.output_dir}</p>
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
              {error && <p role="alert" className="mt-3 text-[12px] text-destructive">{error}</p>}
              <div className="mt-8 flex justify-end">
                <Button onClick={() => setStep(1)} disabled={choosingFolder}>
                  Continue <ArrowRight className="size-4" />
                </Button>
              </div>
            </div>
          )}

          {step === 1 && (
            <div>
              <h1 className="font-serif text-[30px] font-semibold tracking-[-0.02em]">
                Allow audio access.
              </h1>
              <p className="mt-3 text-[14px] leading-[1.6] text-muted-foreground">
                macOS will ask for access to your microphone and system audio. Simstim does not
                start a recording here, and you do not need to speak or play anything.
              </p>
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
                <p className="mt-3 text-[12px] leading-relaxed text-muted-foreground">
                  Allow both permissions in System Settings, then check again.
                </p>
              )}
              {error && <p role="alert" className="mt-4 text-[12px] text-destructive">{error}</p>}
              <div className="mt-7 flex items-center justify-between">
                <Button variant="ghost" onClick={() => setStep(0)} disabled={checking}>
                  Back
                </Button>
                <div className="flex gap-2">
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => void handlePermissionCheck()}
                    disabled={checking}
                    className="text-muted-foreground"
                  >
                    {checking ? (
                      <><LoaderCircle className="size-4 animate-spin" /> Checking...</>
                    ) : (
                      <><RotateCcw className="size-3.5" /> Check again</>
                    )}
                  </Button>
                  <Button onClick={() => setStep(2)} disabled={!permissionsPassed || checking}>
                    Continue <ArrowRight className="size-4" />
                  </Button>
                </div>
              </div>
            </div>
          )}

          {step === 2 && (
            <div>
              <h1 className="font-serif text-[30px] font-semibold tracking-[-0.02em]">
                Set up transcription.
              </h1>
              <p className="mt-3 text-[14px] leading-[1.6] text-muted-foreground">
                Simstim needs one local component to turn calls into text. Its download starts
                automatically and stays entirely on this Mac.
              </p>
              <div className="mt-6 rounded-[10px] border border-border bg-card/55 p-4">
                <div className="flex items-center justify-between gap-5">
                  <div className="min-w-0">
                    <p className="text-[14px] font-semibold">
                      {activeModel?.name ?? currentConfig.active_model}
                    </p>
                    {activeModel?.technical_name && (
                      <p className="mt-0.5 font-mono text-[10px] text-muted-foreground/55">
                        {activeModel.technical_name}
                      </p>
                    )}
                    <p className="mt-1 text-[12px] text-muted-foreground">
                      {activeModel ? humanSize(activeModel.size_mb) : "Recording model"}
                    </p>
                  </div>
                  {activeModel?.downloaded ? (
                    <span className="flex shrink-0 items-center gap-1.5 font-mono text-[10.5px] uppercase tracking-[0.06em] text-emerald-700 dark:text-emerald-400">
                      <Check className="size-3.5" /> Ready
                    </span>
                  ) : activeProgress?.phase === "error" ? (
                    <Button variant="outline" size="sm" onClick={() => activeModel && void handleDownload(activeModel)} disabled={!activeModel}>
                      <Download className="size-3.5" /> Retry
                    </Button>
                  ) : (
                    <span className="flex shrink-0 items-center gap-1.5 text-[11px] text-muted-foreground">
                      <LoaderCircle className="size-3.5 animate-spin" /> Starting...
                    </span>
                  )}
                </div>
                {activeProgress && activeProgress.phase !== "error" && (
                  <div className="mt-4">
                    <Progress value={downloadProgressPercent(activeProgress)} />
                    <p className="mt-2 text-[11px] text-muted-foreground">
                      {activeProgress.total > 0 ? `${Math.round(downloadProgressPercent(activeProgress))}%` : "Starting download..."}
                    </p>
                  </div>
                )}
                {activeProgress?.phase === "error" && (
                  <p role="alert" className="mt-3 text-[12px] text-destructive">{activeProgress.message}</p>
                )}
              </div>
              {error && <p role="alert" className="mt-4 text-[12px] text-destructive">{error}</p>}
              <div className="mt-7 flex items-center justify-between">
                <Button variant="ghost" onClick={() => setStep(1)} disabled={finishing}>
                  Back
                </Button>
                <Button onClick={() => setStep(3)} disabled={!activeModel?.downloaded}>
                  Continue <ArrowRight className="size-4" />
                </Button>
              </div>
            </div>
          )}

          {step === 3 && (
            <div>
              <div className="mb-5 flex size-10 items-center justify-center rounded-full border border-border bg-card text-voice-remote-foreground">
                <Sparkles className="size-4.5" />
              </div>
              <h1 className="font-serif text-[30px] font-semibold tracking-[-0.02em]">
                Set up summaries.
              </h1>
              <p className="mt-3 text-[14px] leading-[1.6] text-muted-foreground">
                Pick an on-device model or one of your installed CLI subscriptions. This step is
                optional, and every choice remains available in Settings.
              </p>

              <div className="mt-5 flex flex-col gap-2">
                {enhanceProviders.map((provider) => {
                  const selected = currentConfig.enhance_provider === provider.id;
                  const isLocal = provider.id === "local";
                  return (
                    <div
                      key={provider.id}
                      className={`rounded-[9px] border px-3.5 py-3 ${selected ? "border-foreground/35 bg-card" : "border-border bg-card/40"}`}
                    >
                      <div className="flex items-center gap-3">
                        <input
                          type="radio"
                          name="onboarding-enhance-provider"
                          aria-label={provider.name}
                          checked={selected}
                          disabled={!provider.available}
                          onChange={() => void updateSummaryConfig(
                            { enhance_provider: provider.id },
                            () => setEnhanceProvider(provider.id),
                          )}
                          className="size-4 shrink-0 appearance-none rounded-full border-[1.5px] border-muted-foreground/60 checked:border-[5px] checked:border-primary disabled:opacity-40"
                        />
                        <div className="min-w-0 flex-1">
                          <p className="text-[13px] font-semibold">{provider.name}</p>
                          {isLocal && summaryModel?.technical_name && (
                            <p className="mt-0.5 truncate font-mono text-[10px] text-muted-foreground/55">
                              {summaryModel.technical_name}
                            </p>
                          )}
                          <p className="mt-0.5 truncate text-[11.5px] text-muted-foreground">
                            {provider.available
                              ? provider.version ?? "Ready"
                              : isLocal
                                ? "Model not downloaded"
                                : `Not installed: ${provider.id} CLI not found`}
                          </p>
                        </div>
                        {isLocal && summaryModel && !summaryModel.downloaded &&
                          (!summaryProgress || summaryProgress.phase === "error") && (
                            <Button
                              variant="outline"
                              size="sm"
                              onClick={() => void handleDownload(summaryModel)}
                            >
                              <Download className="size-3.5" /> {humanSize(summaryModel.size_mb)}
                            </Button>
                          )}
                        {!isLocal && selected && provider.available && (
                          <select
                            aria-label={`${provider.name} model`}
                            value={currentConfig.enhance_models[provider.id] ?? ""}
                            onChange={(event) => {
                              const model = event.target.value;
                              void updateSummaryConfig(
                                {
                                  enhance_models: {
                                    ...currentConfig.enhance_models,
                                    [provider.id]: model,
                                  },
                                },
                                () => setEnhanceModel(provider.id, model),
                              );
                            }}
                            className="h-8 rounded-md border border-input bg-transparent px-2 text-xs outline-none focus-visible:border-ring"
                          >
                            {(ENHANCE_MODEL_OPTIONS[provider.id] ?? [{ value: "", label: "Default" }]).map(
                              (option) => <option key={option.value} value={option.value}>{option.label}</option>,
                            )}
                          </select>
                        )}
                      </div>
                      {isLocal && summaryProgress && summaryProgress.phase !== "error" && (
                        <div className="mt-3 pl-7">
                          <Progress
                            value={downloadProgressPercent(summaryProgress)}
                          />
                          <p className="mt-1.5 text-[10.5px] text-muted-foreground">
                            {summaryProgress.total > 0
                              ? `${Math.round(downloadProgressPercent(summaryProgress))}%`
                              : "Starting download..."}
                          </p>
                        </div>
                      )}
                      {isLocal && summaryProgress?.phase === "error" && (
                        <p role="alert" className="mt-2 pl-7 text-[11px] text-destructive">
                          {summaryProgress.message}
                        </p>
                      )}
                    </div>
                  );
                })}
                {enhanceProviders.length === 0 && (
                  <div className="rounded-[9px] border border-border bg-card/40 px-3.5 py-3 text-[12px] text-muted-foreground">
                    Checking available summary providers...
                  </div>
                )}
              </div>

              <div className="mt-4 divide-y divide-border rounded-[9px] border border-border bg-card/40 px-3.5">
                <label htmlFor="onboarding-auto-enhance" className="flex items-center justify-between gap-4 py-3">
                  <span>
                    <span className="block text-[12.5px] font-medium">Summarize automatically</span>
                    <span className="mt-0.5 block text-[11px] text-muted-foreground">Runs after each finished call.</span>
                  </span>
                  <Checkbox
                    id="onboarding-auto-enhance"
                    variant="switch"
                    checked={currentConfig.auto_enhance}
                    disabled={!selectedProvider?.available}
                    onCheckedChange={(checked) => void updateSummaryConfig(
                      { auto_enhance: checked === true },
                      () => setAutoEnhance(checked === true),
                    )}
                  />
                </label>
                <label htmlFor="onboarding-rename-title" className="flex items-center justify-between gap-4 py-3">
                  <span>
                    <span className="block text-[12.5px] font-medium">Let summaries title notes</span>
                    <span className="mt-0.5 block text-[11px] text-muted-foreground">Never changes a title you typed.</span>
                  </span>
                  <Checkbox
                    id="onboarding-rename-title"
                    variant="switch"
                    checked={currentConfig.enhance_rename_title}
                    onCheckedChange={(checked) => void updateSummaryConfig(
                      { enhance_rename_title: checked === true },
                      () => setEnhanceRenameTitle(checked === true),
                    )}
                  />
                </label>
                <label htmlFor="onboarding-calendar" className="flex items-center justify-between gap-4 py-3">
                  <span>
                    <span className="block text-[12.5px] font-medium">Use calendar context</span>
                    <span className="mt-0.5 block text-[11px] text-muted-foreground">Cloud providers only, through your Claude connectors.</span>
                  </span>
                  <Checkbox
                    id="onboarding-calendar"
                    variant="switch"
                    checked={currentConfig.enhance_use_calendar}
                    disabled={currentConfig.enhance_provider === "local"}
                    onCheckedChange={(checked) => void updateSummaryConfig(
                      { enhance_use_calendar: checked === true },
                      () => setEnhanceUseCalendar(checked === true),
                    )}
                  />
                </label>
              </div>

              {error && <p role="alert" className="mt-3 text-[12px] text-destructive">{error}</p>}
              <div className="mt-6 flex items-center justify-between">
                <Button variant="ghost" onClick={() => setStep(2)} disabled={finishing}>
                  Back
                </Button>
                <Button onClick={() => void handleFinish()} disabled={finishing}>
                  {finishing ? <><LoaderCircle className="size-4 animate-spin" /> Finishing...</> : "Start using Simstim"}
                </Button>
              </div>
            </div>
          )}
        </section>
      </div>
    </main>
  );
}
