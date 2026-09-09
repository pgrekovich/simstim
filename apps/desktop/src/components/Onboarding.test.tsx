import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { Onboarding } from "@/components/Onboarding";
import * as ipc from "@/lib/ipc";
import type { AppConfig, ModelInfo } from "@/lib/ipc";

vi.mock("@/lib/ipc", () => ({
  aiSetupStatus: vi.fn().mockResolvedValue(null),
  setupAi: vi.fn().mockResolvedValue(undefined),
  cancelAiSetup: vi.fn().mockResolvedValue(undefined),
  getPrompts: vi.fn().mockResolvedValue({ summary_instructions: "", summary_styles: [], quick_questions: [] }),
  savePrompts: vi.fn().mockResolvedValue({ summary_instructions: "", summary_styles: [], quick_questions: [] }),
  restoreDefaultPrompts: vi.fn().mockResolvedValue({ summary_instructions: "", summary_styles: [], quick_questions: [] }),
  checkAudioPermissions: vi.fn(),
  completeOnboarding: vi.fn().mockResolvedValue(undefined),
  downloadModel: vi.fn().mockResolvedValue(undefined),
  getConfig: vi.fn(),
  listEnhanceProviders: vi.fn(),
  listModels: vi.fn(),
  setAutoEnhance: vi.fn().mockResolvedValue(undefined),
  setEnhanceProvider: vi.fn().mockResolvedValue(undefined),
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
  auto_retranscribe: true,
  delete_audio_after_retranscribe: false,
  enhance_provider: "claude",
  enhance_models: {},
  auto_enhance: true,
  enhance_rename_title: true,
  enhance_use_calendar: true,
  detect_calls: true,
  auto_stop_after_call: false,
};

function liveModel(downloaded: boolean): ModelInfo {
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

function whisperModel(downloaded: boolean): ModelInfo {
  return {
    id: "whisper",
    name: "Balanced",
    technical_name: "Whisper large-v3-turbo (q8)",
    tagline: null,
    size_mb: 874,
    downloaded,
    active: false,
    role: "retranscribe",
    size_on_disk_bytes: downloaded ? 900_000_000 : null,
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

function renderOnboarding(models: ModelInfo[], onComplete = () => {}) {
  return render(
    <Onboarding config={config} models={models} onOpenSettings={() => {}} onComplete={onComplete} />,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(listen).mockImplementation(() => Promise.resolve(() => {}));
  vi.mocked(ipc.getConfig).mockResolvedValue(config);
  vi.mocked(ipc.listModels).mockResolvedValue([liveModel(false), whisperModel(false)]);
  vi.mocked(open).mockResolvedValue(null);
  vi.mocked(ipc.checkAudioPermissions).mockResolvedValue(permissions(true, true));
  vi.mocked(ipc.listEnhanceProviders).mockResolvedValue([
    { id: "claude", name: "Claude Code", available: false, version: null },
    { id: "codex", name: "Codex", available: false, version: null },
  ]);
});

describe("Onboarding (one screen)", () => {
  it("asks macOS for both permissions on entry and offers a retry until both pass", async () => {
    vi.mocked(ipc.checkAudioPermissions)
      .mockResolvedValueOnce(permissions(true, false))
      .mockResolvedValueOnce(permissions(true, true));
    renderOnboarding([liveModel(true), whisperModel(true)]);

    expect(await screen.findByText("Needs permission")).toBeVisible();
    expect(ipc.checkAudioPermissions).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: /^start$/i })).toBeDisabled();

    fireEvent.click(screen.getByRole("button", { name: /check again/i }));
    await waitFor(() => expect(screen.getAllByText("Allowed")).toHaveLength(2));
    expect(screen.getByRole("button", { name: /^start$/i })).toBeEnabled();
  });

  it("lets the user choose the notes folder", async () => {
    vi.mocked(open).mockResolvedValue("/Users/pavel/Documents/simstim");
    vi.mocked(ipc.getConfig).mockResolvedValue({
      ...config,
      output_dir: "/Users/pavel/Documents/simstim",
    });
    renderOnboarding([liveModel(true), whisperModel(true)]);

    fireEvent.click(screen.getByRole("button", { name: /choose/i }));

    await waitFor(() => {
      expect(ipc.setOutputDir).toHaveBeenCalledWith("/Users/pavel/Documents/simstim");
    });
    expect(await screen.findByText("~/Documents/simstim")).toBeVisible();
  });

  it("downloads both speech models on its own, without naming them, and completes once the live one lands", async () => {
    vi.mocked(ipc.listModels).mockResolvedValue([liveModel(true), whisperModel(false)]);
    vi.mocked(ipc.getConfig)
      .mockResolvedValueOnce(config)
      .mockResolvedValue({ ...config, onboarding_complete: true });
    const onComplete = vi.fn();
    renderOnboarding([liveModel(false), whisperModel(false)], onComplete);

    await waitFor(() => expect(ipc.downloadModel).toHaveBeenCalledWith("live"));
    await waitFor(() => expect(ipc.downloadModel).toHaveBeenCalledWith("whisper"));
    // No engine names or sizes on the first screen: one quiet progress line.
    expect(screen.queryByText(/parakeet/i)).not.toBeInTheDocument();
    expect(screen.queryByText(/whisper/i)).not.toBeInTheDocument();
    expect(screen.getByText(/preparing speech recognition/i)).toBeVisible();
    expect(screen.getByRole("button", { name: /^start$/i })).toBeDisabled();

    emitDownload({ id: "live", phase: "download", received: 50, total: 100 });
    emitDownload({ id: "whisper", phase: "download", received: 0, total: 100 });
    expect(await screen.findByText(/preparing speech recognition · 25%/i)).toBeVisible();

    // The live model is enough to start; the accurate pass's model may
    // keep downloading behind the main window.
    emitDownload({ id: "live", phase: "done", received: 100, total: 100 });
    const start = screen.getByRole("button", { name: /^start$/i });
    await waitFor(() => expect(start).toBeEnabled());
    fireEvent.click(start);

    await waitFor(() => expect(ipc.completeOnboarding).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(onComplete).toHaveBeenCalled());
  });

  it("offers a retry when a download fails", async () => {
    vi.mocked(ipc.downloadModel)
      .mockRejectedValueOnce(new Error("network unavailable"))
      .mockResolvedValue(undefined);
    renderOnboarding([liveModel(false), whisperModel(true)]);

    const retry = await screen.findByRole("button", { name: /retry/i });
    expect(screen.getByRole("alert")).toHaveTextContent("network unavailable");
    fireEvent.click(retry);
    await waitFor(() => expect(ipc.downloadModel).toHaveBeenCalledTimes(2));
  });

  it("shows the installed assistant as one switch that is on by default, and follows what is installed", async () => {
    vi.mocked(ipc.listEnhanceProviders).mockResolvedValue([
      { id: "claude", name: "Claude Code", available: false, version: null },
      { id: "codex", name: "Codex", available: true, version: "0.9.0" },
    ]);
    renderOnboarding([liveModel(true), whisperModel(true)]);

    const toggle = await screen.findByRole("checkbox", { name: /summaries by codex/i });
    expect(toggle).toBeChecked();
    // The saved provider was claude; only codex is here, so the config follows.
    await waitFor(() => expect(ipc.setEnhanceProvider).toHaveBeenCalledWith("codex"));
    expect(screen.getByText(/never audio. turn this off to keep text local/i)).toBeVisible();

    fireEvent.click(toggle);
    await waitFor(() => expect(ipc.setAutoEnhance).toHaveBeenCalledWith(false));
    // No radios, no model menus, no extra toggles on the first screen.
    expect(screen.queryByRole("radio")).not.toBeInTheDocument();
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
  });

  it("explains summaries plainly when no assistant is installed, without a switch", async () => {
    renderOnboarding([liveModel(true), whisperModel(true)]);

    expect(await screen.findByRole("button", { name: "Connect Claude" })).toBeVisible();
    expect(screen.getByRole("button", { name: "Connect ChatGPT" })).toBeVisible();
    expect(ipc.setupAi).not.toHaveBeenCalled();
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
    expect(ipc.setEnhanceProvider).not.toHaveBeenCalled();
  });
});
