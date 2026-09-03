import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { Onboarding } from "@/components/Onboarding";
import * as ipc from "@/lib/ipc";
import type { AppConfig, ModelInfo } from "@/lib/ipc";

vi.mock("@/lib/ipc", () => ({
  checkAudioPermissions: vi.fn(),
  completeOnboarding: vi.fn().mockResolvedValue(undefined),
  downloadModel: vi.fn().mockResolvedValue(undefined),
  getConfig: vi.fn(),
  listEnhanceProviders: vi.fn(),
  listModels: vi.fn(),
  setAutoEnhance: vi.fn().mockResolvedValue(undefined),
  setEnhanceModel: vi.fn().mockResolvedValue(undefined),
  setEnhanceProvider: vi.fn().mockResolvedValue(undefined),
  setEnhanceRenameTitle: vi.fn().mockResolvedValue(undefined),
  setEnhanceUseCalendar: vi.fn().mockResolvedValue(undefined),
  setOutputDir: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

const config: AppConfig = {
  output_dir: "/Users/pavel/Calls",
  active_model: "live",
  onboarding_complete: false,
  theme: "system",
  show_in_dock: true,
  keep_audio: true,
  audio_format: "aac",
  retranscribe_model: "whisper",
  auto_retranscribe: false,
  delete_audio_after_retranscribe: false,
  enhance_provider: "local",
  enhance_models: {},
  auto_enhance: false,
  enhance_rename_title: false,
  enhance_use_calendar: false,
  chat_provider: "claude",
  chat_model: "",
  detect_calls: true,
  auto_stop_after_call: false,
};

function model(downloaded: boolean): ModelInfo {
  return {
    id: "live",
    name: "Live transcription",
    technical_name: "Parakeet TDT 0.6B v3 (int8)",
    tagline: "required for recording",
    size_mb: 671,
    downloaded,
    active: true,
    role: "live",
    size_on_disk_bytes: downloaded ? 500_000_000 : null,
    retired: false,
  };
}

function permissions(microphone: boolean, systemAudio: boolean) {
  return {
    microphone: { granted: microphone, error: null },
    system_audio: { granted: systemAudio, error: null },
  };
}

function emitDownload(payload: ipc.DownloadProgress) {
  const call = vi.mocked(listen).mock.calls.find(([name]) => name === "model-download-progress");
  if (!call) throw new Error("download listener was not registered");
  act(() => {
    (call[1] as (event: { payload: ipc.DownloadProgress }) => void)({ payload });
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(listen).mockImplementation(() => Promise.resolve(() => {}));
  vi.mocked(ipc.getConfig).mockResolvedValue(config);
  vi.mocked(ipc.listModels).mockResolvedValue([model(false)]);
  vi.mocked(open).mockResolvedValue(null);
  vi.mocked(ipc.listEnhanceProviders).mockResolvedValue([
    { id: "local", name: "On this Mac", available: false, version: null },
    { id: "claude", name: "Claude Code", available: false, version: null },
    { id: "codex", name: "Codex", available: false, version: null },
  ]);
});

describe("Onboarding", () => {
  it("checks permissions automatically on entry and supports retry", async () => {
    vi.mocked(ipc.checkAudioPermissions)
      .mockResolvedValueOnce(permissions(true, false))
      .mockResolvedValueOnce(permissions(true, true));
    render(
      <Onboarding config={config} models={[model(false)]} onOpenSettings={() => {}} onComplete={() => {}} />,
    );

    expect(screen.getByRole("button", { name: /settings/i })).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: /^continue$/i }));
    expect(await screen.findByText("Needs permission")).toBeVisible();
    expect(ipc.checkAudioPermissions).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: /continue/i })).toBeDisabled();

    fireEvent.click(screen.getByRole("button", { name: /check again/i }));
    await waitFor(() => expect(screen.getAllByText("Allowed")).toHaveLength(2));
    expect(screen.getByRole("button", { name: /continue/i })).toBeEnabled();
  });

  it("lets the user choose the notes folder before continuing", async () => {
    vi.mocked(open).mockResolvedValue("/Users/pavel/Documents/simstim");
    vi.mocked(ipc.getConfig).mockResolvedValue({
      ...config,
      output_dir: "/Users/pavel/Documents/simstim",
    });
    render(
      <Onboarding config={config} models={[model(false)]} onOpenSettings={() => {}} onComplete={() => {}} />,
    );

    fireEvent.click(screen.getByRole("button", { name: /choose/i }));

    await waitFor(() => {
      expect(ipc.setOutputDir).toHaveBeenCalledWith("/Users/pavel/Documents/simstim");
    });
    expect(await screen.findByText("/Users/pavel/Documents/simstim")).toBeVisible();
  });

  it("uses the shared model download and completes after its done event", async () => {
    vi.mocked(ipc.checkAudioPermissions).mockResolvedValue(permissions(true, true));
    vi.mocked(ipc.listModels).mockResolvedValue([model(true)]);
    vi.mocked(ipc.getConfig)
      .mockResolvedValueOnce(config)
      .mockResolvedValue({ ...config, onboarding_complete: true });
    const onComplete = vi.fn();
    render(
      <Onboarding config={config} models={[model(false)]} onOpenSettings={() => {}} onComplete={onComplete} />,
    );

    fireEvent.click(screen.getByRole("button", { name: /^continue$/i }));
    await waitFor(() => expect(screen.getAllByText("Allowed")).toHaveLength(2));
    fireEvent.click(screen.getByRole("button", { name: /continue/i }));
    await waitFor(() => expect(ipc.downloadModel).toHaveBeenCalledWith("live"));
    expect(screen.getByText("Live transcription")).toBeVisible();
    const technicalName = screen.getByText("Parakeet TDT 0.6B v3 (int8)");
    expect(technicalName).toHaveClass("text-[10px]", "text-muted-foreground/55");

    emitDownload({ id: "live", phase: "done", received: 1, total: 1 });
    const continueButton = await screen.findByRole("button", { name: /continue/i });
    await waitFor(() => expect(continueButton).toBeEnabled());
    fireEvent.click(continueButton);
    const finish = await screen.findByRole("button", { name: "Start using Simstim" });
    fireEvent.click(finish);

    await waitFor(() => expect(ipc.completeOnboarding).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(onComplete).toHaveBeenCalled());
  });

  it("keeps a manual retry only after the automatic transcription download fails", async () => {
    vi.mocked(ipc.checkAudioPermissions).mockResolvedValue(permissions(true, true));
    vi.mocked(ipc.downloadModel)
      .mockRejectedValueOnce(new Error("network unavailable"))
      .mockResolvedValueOnce(undefined);
    render(
      <Onboarding config={config} models={[model(false)]} onOpenSettings={() => {}} onComplete={() => {}} />,
    );

    fireEvent.click(screen.getByRole("button", { name: /^continue$/i }));
    await waitFor(() => expect(screen.getAllByText("Allowed")).toHaveLength(2));
    fireEvent.click(screen.getByRole("button", { name: /continue/i }));

    const retry = await screen.findByRole("button", { name: /retry/i });
    expect(ipc.downloadModel).toHaveBeenCalledTimes(1);
    fireEvent.click(retry);
    await waitFor(() => expect(ipc.downloadModel).toHaveBeenCalledTimes(2));
  });

  it("configures the summary provider and opt-in behavior", async () => {
    let persisted = { ...config };
    vi.mocked(ipc.getConfig).mockImplementation(async () => ({ ...persisted }));
    vi.mocked(ipc.checkAudioPermissions).mockResolvedValue(permissions(true, true));
    vi.mocked(ipc.listEnhanceProviders).mockResolvedValue([
      { id: "local", name: "On this Mac", available: false, version: null },
      { id: "claude", name: "Claude Code", available: true, version: "2.1.0" },
      { id: "codex", name: "Codex", available: false, version: null },
    ]);
    vi.mocked(ipc.setEnhanceProvider).mockImplementation(async (provider: string) => {
      persisted = { ...persisted, enhance_provider: provider };
    });
    vi.mocked(ipc.setEnhanceModel).mockImplementation(async (provider: string, value: string) => {
      persisted = {
        ...persisted,
        enhance_models: { ...persisted.enhance_models, [provider]: value },
      };
    });
    vi.mocked(ipc.setAutoEnhance).mockImplementation(async (enabled: boolean) => {
      persisted = { ...persisted, auto_enhance: enabled };
    });
    vi.mocked(ipc.setEnhanceRenameTitle).mockImplementation(async (enabled: boolean) => {
      persisted = { ...persisted, enhance_rename_title: enabled };
    });
    vi.mocked(ipc.setEnhanceUseCalendar).mockImplementation(async (enabled: boolean) => {
      persisted = { ...persisted, enhance_use_calendar: enabled };
    });

    render(
      <Onboarding config={config} models={[model(true)]} onOpenSettings={() => {}} onComplete={() => {}} />,
    );
    fireEvent.click(screen.getByRole("button", { name: /^continue$/i }));
    await waitFor(() => expect(screen.getAllByText("Allowed")).toHaveLength(2));
    fireEvent.click(screen.getByRole("button", { name: /continue/i }));
    fireEvent.click(screen.getByRole("button", { name: /continue/i }));

    const claude = await screen.findByRole("radio", { name: "Claude Code" });
    fireEvent.click(claude);
    await waitFor(() => expect(ipc.setEnhanceProvider).toHaveBeenCalledWith("claude"));
    fireEvent.change(screen.getByRole("combobox", { name: "Claude Code model" }), {
      target: { value: "sonnet" },
    });
    fireEvent.click(screen.getByRole("checkbox", { name: /summarize automatically/i }));
    fireEvent.click(screen.getByRole("checkbox", { name: /let summaries title notes/i }));
    fireEvent.click(screen.getByRole("checkbox", { name: /use calendar context/i }));

    await waitFor(() => expect(ipc.setEnhanceModel).toHaveBeenCalledWith("claude", "sonnet"));
    await waitFor(() => expect(ipc.setAutoEnhance).toHaveBeenCalledWith(true));
    await waitFor(() => expect(ipc.setEnhanceRenameTitle).toHaveBeenCalledWith(true));
    await waitFor(() => expect(ipc.setEnhanceUseCalendar).toHaveBeenCalledWith(true));
  });
});
