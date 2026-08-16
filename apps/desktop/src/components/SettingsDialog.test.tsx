import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { SettingsDialog } from "@/components/SettingsDialog";
import * as ipc from "@/lib/ipc";
import type { AppConfig, ModelInfo } from "@/lib/ipc";

vi.mock("@/lib/ipc", () => ({
  getConfig: vi.fn(),
  listModels: vi.fn().mockResolvedValue([]),
  downloadModel: vi.fn().mockResolvedValue(undefined),
  setActiveModel: vi.fn().mockResolvedValue(undefined),
  setOutputDir: vi.fn().mockResolvedValue(undefined),
  setTheme: vi.fn().mockResolvedValue(undefined),
  setKeepAudio: vi.fn().mockResolvedValue(undefined),
  setAudioFormat: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
}));

const baseConfig: AppConfig = {
  output_dir: "/tmp/calls",
  active_model: "parakeet-tdt-0.6b-v3-int8",
  theme: "system",
  keep_audio: true,
  audio_format: "aac",
};

beforeEach(() => {
  vi.mocked(ipc.getConfig).mockClear().mockResolvedValue(baseConfig);
  vi.mocked(ipc.listModels).mockClear().mockResolvedValue([]);
  vi.mocked(ipc.setKeepAudio).mockClear().mockResolvedValue(undefined);
  vi.mocked(ipc.setAudioFormat).mockClear().mockResolvedValue(undefined);
  vi.mocked(ipc.setActiveModel).mockClear().mockResolvedValue(undefined);
});

describe("SettingsDialog: keep-audio toggle (stage 0.7 Task 5)", () => {
  it("reflects config.keep_audio and calls setKeepAudio when toggled off", async () => {
    // `refresh()` re-fetches `getConfig()` after a successful `setKeepAudio`
    // call, so the mock needs to reflect the persisted value on the next
    // call the same way the real backend would -- a fixed
    // `mockResolvedValue` would silently clobber the optimistic update back
    // to `true` once `refresh()` runs.
    let persisted = { ...baseConfig };
    vi.mocked(ipc.getConfig).mockImplementation(async () => ({ ...persisted }));
    vi.mocked(ipc.setKeepAudio).mockImplementation(async (keepAudio: boolean) => {
      persisted = { ...persisted, keep_audio: keepAudio };
    });

    render(<SettingsDialog open onOpenChange={() => {}} />);

    const checkbox = await screen.findByRole("checkbox", { name: /keep call audio/i });
    expect(checkbox).toHaveAttribute("aria-checked", "true");

    fireEvent.click(checkbox);

    await waitFor(() => expect(ipc.setKeepAudio).toHaveBeenCalledWith(false));
    await waitFor(() => expect(checkbox).toHaveAttribute("aria-checked", "false"));
  });

  it("reverts the checkbox and shows an inline error when setKeepAudio rejects", async () => {
    vi.mocked(ipc.setKeepAudio).mockRejectedValueOnce(new Error("disk write failed"));

    render(<SettingsDialog open onOpenChange={() => {}} />);

    const checkbox = await screen.findByRole("checkbox", { name: /keep call audio/i });
    expect(checkbox).toHaveAttribute("aria-checked", "true");

    fireEvent.click(checkbox);

    await waitFor(() => expect(ipc.setKeepAudio).toHaveBeenCalledWith(false));
    // Reverted back to the last persisted value, not left showing the
    // optimistic (unsaved) state.
    await waitFor(() => expect(checkbox).toHaveAttribute("aria-checked", "true"));
    await screen.findByText(/disk write failed/i);
  });
});

describe("SettingsDialog: audio format toggle (stage 0.7 format change)", () => {
  it("shows AAC active by default and switches to FLAC on click", async () => {
    let persisted = { ...baseConfig };
    vi.mocked(ipc.getConfig).mockImplementation(async () => ({ ...persisted }));
    vi.mocked(ipc.setAudioFormat).mockImplementation(async (format: string) => {
      persisted = { ...persisted, audio_format: format };
    });

    render(<SettingsDialog open onOpenChange={() => {}} />);

    const aacButton = await screen.findByRole("button", { name: "AAC" });
    const flacButton = await screen.findByRole("button", { name: "FLAC" });
    expect(aacButton).toHaveAttribute("data-slot", "button");

    // Helper text states the real measured numbers, not a rounded guess.
    await screen.findByText(/17 mb per hour/i);
    await screen.findByText(/137 mb per hour/i);

    fireEvent.click(flacButton);

    await waitFor(() => expect(ipc.setAudioFormat).toHaveBeenCalledWith("flac"));
  });

  it("reverts to the previous format and shows an inline error when setAudioFormat rejects", async () => {
    vi.mocked(ipc.setAudioFormat).mockRejectedValueOnce(new Error("disk write failed"));

    render(<SettingsDialog open onOpenChange={() => {}} />);

    const flacButton = await screen.findByRole("button", { name: "FLAC" });
    fireEvent.click(flacButton);

    await waitFor(() => expect(ipc.setAudioFormat).toHaveBeenCalledWith("flac"));
    await screen.findByText(/disk write failed/i);
  });

  it("disables the format buttons when keep-audio is off", async () => {
    vi.mocked(ipc.getConfig).mockResolvedValue({ ...baseConfig, keep_audio: false });

    render(<SettingsDialog open onOpenChange={() => {}} />);

    const flacButton = await screen.findByRole("button", { name: "FLAC" });
    expect(flacButton).toBeDisabled();
  });
});

describe("SettingsDialog: whisper model row (stage 0.7 Task 5)", () => {
  const models: ModelInfo[] = [
    {
      id: "parakeet-tdt-0.6b-v3-int8",
      name: "Parakeet TDT 0.6B v3 (int8)",
      size_mb: 487,
      downloaded: true,
      active: true,
      selectable: true,
    },
    {
      id: "whisper-large-v3-turbo-q8",
      name: "Whisper large-v3-turbo (q8)",
      size_mb: 874,
      downloaded: true,
      active: false,
      selectable: false,
    },
  ];

  it("never offers Use/Active for a downloaded, non-selectable model like whisper", async () => {
    vi.mocked(ipc.listModels).mockResolvedValue(models);

    render(<SettingsDialog open onOpenChange={() => {}} />);

    await screen.findByText("Whisper large-v3-turbo (q8)");
    await screen.findByText("874 MB");

    // The live-engine row still gets its normal Active affordance.
    const parakeetRow = screen.getByText("Parakeet TDT 0.6B v3 (int8)").closest("li")!;
    expect(within(parakeetRow).getByText(/active/i)).toBeInTheDocument();

    // Whisper is downloaded but must never show Use or Active: it can't
    // become the live engine, only a plain "downloaded" marker.
    const whisperRow = screen.getByText("Whisper large-v3-turbo (q8)").closest("li")!;
    expect(within(whisperRow).queryByRole("button", { name: /use/i })).not.toBeInTheDocument();
    expect(within(whisperRow).queryByText(/^active$/i)).not.toBeInTheDocument();
    expect(within(whisperRow).getByText(/downloaded/i)).toBeInTheDocument();
  });

  it("still offers Download for whisper when it has not been downloaded yet", async () => {
    vi.mocked(ipc.listModels).mockResolvedValue([
      { ...models[1], downloaded: false },
    ]);

    render(<SettingsDialog open onOpenChange={() => {}} />);

    const downloadButton = await screen.findByRole("button", { name: /download/i });
    expect(downloadButton).toBeInTheDocument();
  });
});
