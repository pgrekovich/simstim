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
  setRetranscribeModel: vi.fn().mockResolvedValue(undefined),
  deleteModel: vi.fn().mockResolvedValue(undefined),
  setOutputDir: vi.fn().mockResolvedValue(undefined),
  setTheme: vi.fn().mockResolvedValue(undefined),
  setKeepAudio: vi.fn().mockResolvedValue(undefined),
  setAudioFormat: vi.fn().mockResolvedValue(undefined),
  setAutoRetranscribe: vi.fn().mockResolvedValue(undefined),
  setDeleteAudioAfterRetranscribe: vi.fn().mockResolvedValue(undefined),
  setDetectCalls: vi.fn().mockResolvedValue(undefined),
  setEnhanceProvider: vi.fn().mockResolvedValue(undefined),
  setEnhanceModel: vi.fn().mockResolvedValue(undefined),
  setAutoEnhance: vi.fn().mockResolvedValue(undefined),
  listEnhanceProviders: vi.fn().mockResolvedValue([
    { id: "claude", name: "Claude Code", available: true, version: "2.0.0 (Claude Code)" },
    { id: "codex", name: "Codex", available: false, version: null },
  ]),
  setAutoStopAfterCall: vi.fn().mockResolvedValue(undefined),
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
  retranscribe_model: "whisper-large-v3-turbo-q8",
  auto_retranscribe: false,
  delete_audio_after_retranscribe: false,
  enhance_provider: "claude",
  enhance_models: {},
  auto_enhance: false,
  detect_calls: true,
  auto_stop_after_call: false,
};

beforeEach(() => {
  vi.mocked(ipc.getConfig).mockClear().mockResolvedValue(baseConfig);
  vi.mocked(ipc.listModels).mockClear().mockResolvedValue([]);
  vi.mocked(ipc.setKeepAudio).mockClear().mockResolvedValue(undefined);
  vi.mocked(ipc.setAudioFormat).mockClear().mockResolvedValue(undefined);
  vi.mocked(ipc.setAutoRetranscribe).mockClear().mockResolvedValue(undefined);
  vi.mocked(ipc.setDeleteAudioAfterRetranscribe).mockClear().mockResolvedValue(undefined);
  vi.mocked(ipc.setDetectCalls).mockClear().mockResolvedValue(undefined);
  vi.mocked(ipc.setAutoStopAfterCall).mockClear().mockResolvedValue(undefined);
  vi.mocked(ipc.setActiveModel).mockClear().mockResolvedValue(undefined);
  vi.mocked(ipc.setRetranscribeModel).mockClear().mockResolvedValue(undefined);
  vi.mocked(ipc.deleteModel).mockClear().mockResolvedValue(undefined);
});

describe("SettingsDialog: call detection (stage 1.0)", () => {
  it("persists detection and enables the dependent auto-stop row", async () => {
    let persisted = { ...baseConfig };
    vi.mocked(ipc.getConfig).mockImplementation(async () => ({ ...persisted }));
    vi.mocked(ipc.setDetectCalls).mockImplementation(async (enabled: boolean) => {
      persisted = { ...persisted, detect_calls: enabled };
    });
    vi.mocked(ipc.setAutoStopAfterCall).mockImplementation(async (enabled: boolean) => {
      persisted = { ...persisted, auto_stop_after_call: enabled };
    });

    render(<SettingsDialog open onOpenChange={() => {}} />);
    const detect = await screen.findByRole("checkbox", {
      name: /offer to record when a call starts/i,
    });
    const autoStop = screen.getByRole("checkbox", {
      name: /stop automatically when the call ends/i,
    });
    expect(detect).toHaveAttribute("aria-checked", "true");
    expect(autoStop).not.toBeDisabled();

    fireEvent.click(autoStop);
    await waitFor(() => expect(ipc.setAutoStopAfterCall).toHaveBeenCalledWith(true));
    await waitFor(() => expect(autoStop).toHaveAttribute("aria-checked", "true"));
  });

  it("rolls detection back and shows the persistence error", async () => {
    vi.mocked(ipc.setDetectCalls).mockRejectedValueOnce(new Error("config write failed"));
    render(<SettingsDialog open onOpenChange={() => {}} />);
    const detect = await screen.findByRole("checkbox", {
      name: /offer to record when a call starts/i,
    });

    fireEvent.click(detect);
    await waitFor(() => expect(ipc.setDetectCalls).toHaveBeenCalledWith(false));
    await waitFor(() => expect(detect).toHaveAttribute("aria-checked", "true"));
    expect(await screen.findByText(/config write failed/i)).toBeVisible();
  });
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

describe("SettingsDialog: auto re-transcribe and audio cleanup toggles (stage 0.9 Task 2)", () => {
  it("reflects auto_retranscribe and calls setAutoRetranscribe when toggled on", async () => {
    let persisted = { ...baseConfig };
    vi.mocked(ipc.getConfig).mockImplementation(async () => ({ ...persisted }));
    vi.mocked(ipc.setAutoRetranscribe).mockImplementation(async (autoRetranscribe: boolean) => {
      persisted = { ...persisted, auto_retranscribe: autoRetranscribe };
    });

    render(<SettingsDialog open onOpenChange={() => {}} />);

    const checkbox = await screen.findByRole("checkbox", {
      name: /re-transcribe automatically after stopping/i,
    });
    expect(checkbox).toHaveAttribute("aria-checked", "false");

    fireEvent.click(checkbox);

    await waitFor(() => expect(ipc.setAutoRetranscribe).toHaveBeenCalledWith(true));
    await waitFor(() => expect(checkbox).toHaveAttribute("aria-checked", "true"));
  });

  it("reverts the auto re-transcribe checkbox and shows an inline error when setAutoRetranscribe rejects", async () => {
    vi.mocked(ipc.setAutoRetranscribe).mockRejectedValueOnce(new Error("disk write failed"));

    render(<SettingsDialog open onOpenChange={() => {}} />);

    const checkbox = await screen.findByRole("checkbox", {
      name: /re-transcribe automatically after stopping/i,
    });
    fireEvent.click(checkbox);

    await waitFor(() => expect(ipc.setAutoRetranscribe).toHaveBeenCalledWith(true));
    await waitFor(() => expect(checkbox).toHaveAttribute("aria-checked", "false"));
    await screen.findByText(/disk write failed/i);
  });

  it("reflects delete_audio_after_retranscribe and calls setDeleteAudioAfterRetranscribe when toggled on", async () => {
    let persisted = { ...baseConfig };
    vi.mocked(ipc.getConfig).mockImplementation(async () => ({ ...persisted }));
    vi.mocked(ipc.setDeleteAudioAfterRetranscribe).mockImplementation(async (deleteAudioAfter: boolean) => {
      persisted = { ...persisted, delete_audio_after_retranscribe: deleteAudioAfter };
    });

    render(<SettingsDialog open onOpenChange={() => {}} />);

    const checkbox = await screen.findByRole("checkbox", {
      name: /delete the audio after re-transcribing/i,
    });
    expect(checkbox).toHaveAttribute("aria-checked", "false");

    fireEvent.click(checkbox);

    await waitFor(() => expect(ipc.setDeleteAudioAfterRetranscribe).toHaveBeenCalledWith(true));
    await waitFor(() => expect(checkbox).toHaveAttribute("aria-checked", "true"));
  });

  it("reverts the delete-audio checkbox and shows an inline error when setDeleteAudioAfterRetranscribe rejects", async () => {
    vi.mocked(ipc.setDeleteAudioAfterRetranscribe).mockRejectedValueOnce(new Error("disk write failed"));

    render(<SettingsDialog open onOpenChange={() => {}} />);

    const checkbox = await screen.findByRole("checkbox", {
      name: /delete the audio after re-transcribing/i,
    });
    fireEvent.click(checkbox);

    await waitFor(() => expect(ipc.setDeleteAudioAfterRetranscribe).toHaveBeenCalledWith(true));
    await waitFor(() => expect(checkbox).toHaveAttribute("aria-checked", "false"));
    await screen.findByText(/disk write failed/i);
  });

  it("renders the delete toggle's irreversibility copy plainly, for every re-transcribe not just automatic", async () => {
    render(<SettingsDialog open onOpenChange={() => {}} />);

    await screen.findByText(/removes the recording once a re-transcribe succeeds/i);
    await screen.findByText(/manual or automatic/i);
    await screen.findByText(/can never be re-transcribed again/i);
  });

  it("disables only the auto toggle (with a muted note) when keep_audio is off, and leaves delete enabled", async () => {
    // Fix-review correction: only the AUTO trigger actually needs
    // `keep_audio` on (it fires right after a stop, so it's useless without
    // an audio file just having been kept). The delete toggle applies to ANY
    // successful re-transcribe, including a manual one on an old session
    // that still has audio from back when `keep_audio` was on -- so it must
    // stay enabled regardless of the current setting.
    vi.mocked(ipc.getConfig).mockResolvedValue({ ...baseConfig, keep_audio: false });

    render(<SettingsDialog open onOpenChange={() => {}} />);

    const autoCheckbox = await screen.findByRole("checkbox", {
      name: /re-transcribe automatically after stopping/i,
    });
    const deleteCheckbox = await screen.findByRole("checkbox", {
      name: /delete the audio after re-transcribing/i,
    });
    expect(autoCheckbox).toBeDisabled();
    expect(deleteCheckbox).not.toBeDisabled();

    // Exactly one "Needs call audio" note -- on the auto row only.
    const notes = screen.getAllByText(/needs call audio/i);
    expect(notes).toHaveLength(1);
  });

  it("enables both toggles when keep_audio is on", async () => {
    render(<SettingsDialog open onOpenChange={() => {}} />);

    const autoCheckbox = await screen.findByRole("checkbox", {
      name: /re-transcribe automatically after stopping/i,
    });
    const deleteCheckbox = await screen.findByRole("checkbox", {
      name: /delete the audio after re-transcribing/i,
    });
    expect(autoCheckbox).not.toBeDisabled();
    expect(deleteCheckbox).not.toBeDisabled();
    expect(screen.queryByText(/needs call audio/i)).not.toBeInTheDocument();
  });

  it("keeps the delete toggle enabled even when keep_audio is off, so an old session's audio can still be deleted after a manual re-transcribe", async () => {
    vi.mocked(ipc.getConfig).mockResolvedValue({ ...baseConfig, keep_audio: false });

    render(<SettingsDialog open onOpenChange={() => {}} />);

    const deleteCheckbox = await screen.findByRole("checkbox", {
      name: /delete the audio after re-transcribing/i,
    });
    expect(deleteCheckbox).not.toBeDisabled();
    expect(deleteCheckbox).not.toHaveAttribute("aria-checked", "true");
  });
});

describe("SettingsDialog: whisper model row (stage 0.7 Task 5)", () => {
  const models: ModelInfo[] = [
    {
      id: "parakeet-tdt-0.6b-v3-int8",
      name: "Parakeet TDT 0.6B v3 (int8)",
      tagline: null,
      size_mb: 487,
      downloaded: true,
      active: true,
      role: "live",
      size_on_disk_bytes: 487_000_000,
      retired: false,
    },
    {
      id: "whisper-large-v3-turbo-q8",
      name: "Whisper turbo",
      tagline: "faster, slightly weaker on non-English",
      size_mb: 874,
      downloaded: true,
      active: false,
      role: "retranscribe",
      size_on_disk_bytes: 874_188_075,
      retired: false,
    },
  ];

  it("never offers Use/Active for a downloaded, non-live model like whisper", async () => {
    vi.mocked(ipc.listModels).mockResolvedValue(models);

    render(<SettingsDialog open onOpenChange={() => {}} />);

    await screen.findByText("Whisper turbo");
    await screen.findByText("874 MB");

    // The live-engine row still gets its normal Active affordance.
    const parakeetRow = screen.getByText("Parakeet TDT 0.6B v3 (int8)").closest("li")!;
    expect(within(parakeetRow).getByText(/active/i)).toBeInTheDocument();

    // Whisper is downloaded but must never show Use or Active: it can't
    // become the live engine. Being downloaded now surfaces as a selection
    // radio plus a delete control instead (stage 0.8 model manager).
    const whisperRow = screen.getByText("Whisper turbo").closest("li")!;
    expect(within(whisperRow).queryByRole("button", { name: /use/i })).not.toBeInTheDocument();
    expect(within(whisperRow).queryByText(/^active$/i)).not.toBeInTheDocument();
    expect(within(whisperRow).getByRole("radio")).toBeInTheDocument();
    expect(within(whisperRow).getByRole("button", { name: /delete/i })).toBeInTheDocument();
  });

  it("still offers Download for whisper when it has not been downloaded yet", async () => {
    vi.mocked(ipc.listModels).mockResolvedValue([
      { ...models[1], downloaded: false, size_on_disk_bytes: null },
    ]);

    render(<SettingsDialog open onOpenChange={() => {}} />);

    const downloadButton = await screen.findByRole("button", { name: /download/i });
    expect(downloadButton).toBeInTheDocument();
  });
});

describe("SettingsDialog: model manager (stage 0.8 Task 2)", () => {
  const parakeet: ModelInfo = {
    id: "parakeet-tdt-0.6b-v3-int8",
    name: "Parakeet TDT 0.6B v3 (int8)",
    tagline: null,
    size_mb: 487,
    downloaded: true,
    active: true,
    role: "live",
    size_on_disk_bytes: 487_000_000,
    retired: false,
  };

  const whisperQ8: ModelInfo = {
    id: "whisper-large-v3-turbo-q8",
    name: "Whisper turbo",
    tagline: "faster, slightly weaker on non-English",
    size_mb: 874,
    downloaded: true,
    active: false,
    role: "retranscribe",
    size_on_disk_bytes: 874_188_075,
    retired: false,
  };

  const whisperLargeV3: ModelInfo = {
    id: "whisper-large-v3",
    name: "Whisper large-v3",
    tagline: "best quality, slower",
    size_mb: 3095,
    downloaded: true,
    active: false,
    role: "retranscribe",
    size_on_disk_bytes: 3_095_033_483,
    retired: false,
  };

  const whisperLargeV3NotDownloaded: ModelInfo = {
    ...whisperLargeV3,
    downloaded: false,
    size_on_disk_bytes: null,
  };

  // Retired (stage 0.8.x curation): `whisper-large-v3-q5` was curated out of
  // the offered lineup, but stays visible here because a copy is still on
  // disk (`list_models` only ever sends a retired entry in that case -- an
  // absent one is hidden outright, so a fixture with `downloaded: false`
  // would never actually arrive from the real backend).
  const whisperLargeQ5Retired: ModelInfo = {
    id: "whisper-large-v3-q5",
    name: "Whisper large-v3 (older version)",
    tagline: null,
    size_mb: 1081,
    downloaded: true,
    active: false,
    role: "retranscribe",
    size_on_disk_bytes: 1_100_000_000,
    retired: true,
  };

  it("selecting a downloaded re-transcribe model calls setRetranscribeModel and reflects the selection", async () => {
    let persisted = { ...baseConfig, retranscribe_model: whisperQ8.id };
    vi.mocked(ipc.getConfig).mockImplementation(async () => ({ ...persisted }));
    vi.mocked(ipc.listModels).mockResolvedValue([parakeet, whisperQ8, whisperLargeV3]);
    vi.mocked(ipc.setRetranscribeModel).mockImplementation(async (id: string) => {
      persisted = { ...persisted, retranscribe_model: id };
    });

    render(<SettingsDialog open onOpenChange={() => {}} />);

    const currentRadio = await screen.findByRole("radio", { name: "Whisper turbo" });
    expect(currentRadio).toBeChecked();

    const otherRadio = screen.getByRole("radio", { name: "Whisper large-v3" });
    expect(otherRadio).not.toBeChecked();

    fireEvent.click(otherRadio);

    await waitFor(() => expect(ipc.setRetranscribeModel).toHaveBeenCalledWith(whisperLargeV3.id));
    await waitFor(() => expect(otherRadio).toBeChecked());
    expect(currentRadio).not.toBeChecked();
  });

  it("renders a retired model (still on disk) with size and delete only, no radio, no download, no tagline", async () => {
    vi.mocked(ipc.listModels).mockResolvedValue([parakeet, whisperQ8, whisperLargeQ5Retired]);

    render(<SettingsDialog open onOpenChange={() => {}} />);

    const row = (await screen.findByText(/whisper large-v3 \(older version\)/i)).closest("li")!;
    expect(within(row).queryByRole("radio")).not.toBeInTheDocument();
    expect(within(row).queryByRole("button", { name: /download/i })).not.toBeInTheDocument();
    expect(within(row).getByRole("button", { name: /delete/i })).toBeInTheDocument();
    expect(within(row).getByText(/no longer offered/i)).toBeInTheDocument();
    expect(within(row).getByText("1.08 GB")).toBeInTheDocument();

    // The still-offered rows are unaffected by a retired sibling being present.
    expect(screen.getByRole("radio", { name: "Whisper turbo" })).toBeInTheDocument();
  });

  it("requires confirmation before deleting, and states the real freed size", async () => {
    vi.mocked(ipc.listModels).mockResolvedValue([parakeet, whisperQ8]);

    render(<SettingsDialog open onOpenChange={() => {}} />);

    const row = (await screen.findByText("Whisper turbo")).closest("li")!;
    const deleteButton = within(row).getByRole("button", { name: /delete whisper turbo/i });

    fireEvent.click(deleteButton);
    expect(ipc.deleteModel).not.toHaveBeenCalled();

    // 874_188_075 bytes (the real on-disk size, not the advertised size_mb)
    // rounds to 874 MB.
    const confirmButton = await within(row).findByRole("button", { name: /delete 874 mb\?/i });
    fireEvent.click(confirmButton);

    await waitFor(() => expect(ipc.deleteModel).toHaveBeenCalledWith(whisperQ8.id));
  });

  it("renders no delete control on the live model row", async () => {
    vi.mocked(ipc.listModels).mockResolvedValue([parakeet, whisperQ8]);

    render(<SettingsDialog open onOpenChange={() => {}} />);

    const parakeetRow = (await screen.findByText("Parakeet TDT 0.6B v3 (int8)")).closest("li")!;
    expect(within(parakeetRow).queryByRole("button", { name: /delete/i })).not.toBeInTheDocument();
  });

  it("offers Download and no selection control for a not-downloaded re-transcribe model", async () => {
    vi.mocked(ipc.listModels).mockResolvedValue([parakeet, whisperLargeV3NotDownloaded]);

    render(<SettingsDialog open onOpenChange={() => {}} />);

    const row = (await screen.findByText("Whisper large-v3")).closest("li")!;
    expect(within(row).getByRole("button", { name: /download/i })).toBeInTheDocument();
    expect(within(row).queryByRole("radio")).not.toBeInTheDocument();
    expect(within(row).queryByRole("button", { name: /delete/i })).not.toBeInTheDocument();
  });

  it("leaves the row not-downloaded after deleting the currently selected re-transcribe model", async () => {
    let downloaded = true;
    vi.mocked(ipc.getConfig).mockResolvedValue({ ...baseConfig, retranscribe_model: whisperQ8.id });
    vi.mocked(ipc.listModels).mockImplementation(async () => [
      parakeet,
      {
        ...whisperQ8,
        downloaded,
        size_on_disk_bytes: downloaded ? whisperQ8.size_on_disk_bytes : null,
      },
    ]);
    vi.mocked(ipc.deleteModel).mockImplementation(async () => {
      downloaded = false;
    });

    render(<SettingsDialog open onOpenChange={() => {}} />);

    const row = (await screen.findByText("Whisper turbo")).closest("li")!;
    fireEvent.click(within(row).getByRole("button", { name: /delete whisper turbo/i }));
    fireEvent.click(await within(row).findByRole("button", { name: /delete 874 mb\?/i }));

    await waitFor(() => expect(ipc.deleteModel).toHaveBeenCalledWith(whisperQ8.id));
    await waitFor(() => {
      const refreshedRow = screen.getByText("Whisper turbo").closest("li")!;
      expect(within(refreshedRow).getByRole("button", { name: /download/i })).toBeInTheDocument();
      expect(within(refreshedRow).queryByRole("radio")).not.toBeInTheDocument();
    });
  });

  it("formats GB sizes decimally, matching the README (1081 MB -> 1.08 GB, 3095 MB -> 3.1 GB)", async () => {
    vi.mocked(ipc.listModels).mockResolvedValue([parakeet, whisperLargeQ5Retired, whisperLargeV3]);

    render(<SettingsDialog open onOpenChange={() => {}} />);

    // `humanSizeMb` (the advertised size_mb, decimal MB throughout) -- shown
    // on both a retired row (whisperLargeQ5Retired) and a still-offered one
    // (whisperLargeV3), since size formatting doesn't depend on retired status.
    await screen.findByText("1.08 GB");
    await screen.findByText("3.1 GB");
  });

  it("shows a muted note in the Re-transcribe group when the configured model is not downloaded", async () => {
    vi.mocked(ipc.getConfig).mockResolvedValue({ ...baseConfig, retranscribe_model: whisperQ8.id });
    vi.mocked(ipc.listModels).mockResolvedValue([
      parakeet,
      { ...whisperQ8, downloaded: false, size_on_disk_bytes: null },
    ]);

    render(<SettingsDialog open onOpenChange={() => {}} />);

    await screen.findByText(/selected model is not downloaded/i);
  });

  it("shows no missing-model note when the configured re-transcribe model is downloaded", async () => {
    vi.mocked(ipc.getConfig).mockResolvedValue({ ...baseConfig, retranscribe_model: whisperQ8.id });
    vi.mocked(ipc.listModels).mockResolvedValue([parakeet, whisperQ8]);

    render(<SettingsDialog open onOpenChange={() => {}} />);

    await screen.findByText("Whisper turbo");
    expect(screen.queryByText(/selected model is not downloaded/i)).not.toBeInTheDocument();
  });

  it("shows a distinct 'no longer offered' note (not the download one) when the configured model is retired but still on disk", async () => {
    // Review finding: the config can point at a retired model that IS still
    // downloaded (e.g. a legacy config from before this model was curated
    // out). The generic "not downloaded... download it again" copy would be
    // false in that case (it's downloaded) and impossible advice (retired
    // ids can never be re-downloaded), and would contradict the
    // "(no longer offered)" label on the row itself.
    vi.mocked(ipc.getConfig).mockResolvedValue({
      ...baseConfig,
      retranscribe_model: whisperLargeQ5Retired.id,
    });
    vi.mocked(ipc.listModels).mockResolvedValue([parakeet, whisperQ8, whisperLargeQ5Retired]);

    render(<SettingsDialog open onOpenChange={() => {}} />);

    await screen.findByText(/selected model is no longer offered/i);
    expect(screen.queryByText(/selected model is not downloaded/i)).not.toBeInTheDocument();
  });

  it("shows total disk used near each group heading, labeled with its own scope", async () => {
    vi.mocked(ipc.listModels).mockResolvedValue([parakeet, whisperQ8, whisperLargeV3NotDownloaded]);

    render(<SettingsDialog open onOpenChange={() => {}} />);

    // "Live model", not "Recording": that word now names both a nav-rail
    // item and a settings section.
    await screen.findByRole("heading", { name: "Live model" });
    // Recording: only parakeet, 487_000_000 bytes -> 487 MB.
    // Re-transcribe: only whisperQ8 is downloaded, 874_188_075 bytes -> 874 MB
    // (the not-downloaded large-v3 contributes nothing). Each total names its
    // own group so neither reads as a whole-app total.
    await screen.findByText(/recording models: 487 mb on disk/i);
    await screen.findByText(/re-transcribe models: 874 mb on disk/i);
  });

  it("disarms an armed delete confirmation when the dialog closes and reopens", async () => {
    // Security-review finding: the dialog stays mounted for the app's whole
    // lifetime (only `open` toggles visibility) and `deleteConfirmId` used to
    // survive that, so arming delete on a model, closing without choosing
    // (the natural way to back out), and reopening later left the row one
    // click from `deleteModel` with no fresh gate.
    vi.mocked(ipc.listModels).mockResolvedValue([parakeet, whisperQ8]);

    const { rerender } = render(<SettingsDialog open onOpenChange={() => {}} />);

    const row = (await screen.findByText("Whisper turbo")).closest("li")!;
    fireEvent.click(within(row).getByRole("button", { name: /delete whisper turbo/i }));
    await within(row).findByRole("button", { name: /delete 874 mb\?/i });

    rerender(<SettingsDialog open={false} onOpenChange={() => {}} />);
    rerender(<SettingsDialog open onOpenChange={() => {}} />);

    const reopenedRow = (await screen.findByText("Whisper turbo")).closest("li")!;
    // Back to the trash icon, not the armed "Delete 874 MB?" button.
    const trashButton = await within(reopenedRow).findByRole("button", {
      name: /delete whisper turbo/i,
    });
    expect(within(reopenedRow).queryByRole("button", { name: /delete 874 mb\?/i })).not.toBeInTheDocument();

    // One click on the (now disarmed) trash icon must ask again, not delete.
    fireEvent.click(trashButton);
    expect(ipc.deleteModel).not.toHaveBeenCalled();
    await within(reopenedRow).findByRole("button", { name: /delete 874 mb\?/i });
  });
});

describe("SettingsDialog: AI enhance provider (stage 0.11)", () => {
  async function openAiSection() {
    render(<SettingsDialog open onOpenChange={() => {}} />);
    fireEvent.click(await screen.findByRole("button", { name: /^summary$/i }));
  }

  it("lists probed providers with availability and picks one via the radio", async () => {
    await openAiSection();

    await screen.findByText("2.0.0 (Claude Code)");
    expect(screen.getByText(/not installed \(no `codex` CLI found\)/i)).toBeInTheDocument();

    const claudeRadio = screen.getByRole("radio", { name: "Claude Code" });
    expect(claudeRadio).toBeChecked();
    const codexRadio = screen.getByRole("radio", { name: "Codex" });
    expect(codexRadio).toBeDisabled();
  });

  it("persists a provider switch", async () => {
    vi.mocked(ipc.listEnhanceProviders).mockResolvedValue([
      { id: "claude", name: "Claude Code", available: true, version: "2.0.0" },
      { id: "codex", name: "Codex", available: true, version: "codex-cli 0.149.0" },
    ]);
    await openAiSection();

    const codexRadio = await screen.findByRole("radio", { name: "Codex" });
    fireEvent.click(codexRadio);
    await waitFor(() =>
      expect(ipc.setEnhanceProvider).toHaveBeenCalledExactlyOnceWith("codex"),
    );
    expect(codexRadio).toBeChecked();
  });
});

describe("SettingsDialog: enhance model override (stage 0.11)", () => {
  it("persists a model pick and clears it back to Default", async () => {
    render(<SettingsDialog open onOpenChange={() => {}} />);
    fireEvent.click(await screen.findByRole("button", { name: /^summary$/i }));

    const select = await screen.findByRole("combobox", { name: /claude code model/i });
    fireEvent.change(select, { target: { value: "fable" } });
    await waitFor(() =>
      expect(ipc.setEnhanceModel).toHaveBeenCalledExactlyOnceWith("claude", "fable"),
    );

    fireEvent.change(select, { target: { value: "" } });
    await waitFor(() => expect(ipc.setEnhanceModel).toHaveBeenLastCalledWith("claude", ""));
  });
});
