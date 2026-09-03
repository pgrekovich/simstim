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
  setShowInDock: vi.fn().mockResolvedValue(undefined),
  setKeepAudio: vi.fn().mockResolvedValue(undefined),
  setAudioFormat: vi.fn().mockResolvedValue(undefined),
  setAutoRetranscribe: vi.fn().mockResolvedValue(undefined),
  setDeleteAudioAfterRetranscribe: vi.fn().mockResolvedValue(undefined),
  setDetectCalls: vi.fn().mockResolvedValue(undefined),
  setEnhanceProvider: vi.fn().mockResolvedValue(undefined),
  setEnhanceModel: vi.fn().mockResolvedValue(undefined),
  setAutoEnhance: vi.fn().mockResolvedValue(undefined),
  setEnhanceRenameTitle: vi.fn().mockResolvedValue(undefined),
  setEnhanceUseCalendar: vi.fn().mockResolvedValue(undefined),
  listEnhanceProviders: vi.fn().mockResolvedValue([
    { id: "local", name: "On this Mac", available: false, version: null },
    { id: "claude", name: "Claude Code", available: true, version: "2.0.0 (Claude Code)" },
    { id: "codex", name: "Codex", available: false, version: null },
  ]),
  setAutoStopAfterCall: vi.fn().mockResolvedValue(undefined),
  storageStats: vi.fn().mockResolvedValue({
    note_count: 0,
    notes_bytes: 0,
    audio_bytes: 0,
    audio_secs: 0,
  }),
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
  onboarding_complete: true,
  theme: "system",
  show_in_dock: true,
  keep_audio: true,
  audio_format: "aac",
  retranscribe_model: "whisper-large-v3-turbo-q8",
  auto_retranscribe: false,
  delete_audio_after_retranscribe: false,
  enhance_provider: "claude",
  enhance_models: {},
  auto_enhance: false,
  enhance_rename_title: false,
  enhance_use_calendar: false,
  chat_provider: "claude",
  chat_model: "",
  detect_calls: true,
  auto_stop_after_call: false,
};

beforeEach(() => {
  vi.mocked(ipc.getConfig).mockClear().mockResolvedValue(baseConfig);
  vi.mocked(ipc.listModels).mockClear().mockResolvedValue([]);
  vi.mocked(ipc.setKeepAudio).mockClear().mockResolvedValue(undefined);
  vi.mocked(ipc.setShowInDock).mockClear().mockResolvedValue(undefined);
  vi.mocked(ipc.setAudioFormat).mockClear().mockResolvedValue(undefined);
  vi.mocked(ipc.setAutoRetranscribe).mockClear().mockResolvedValue(undefined);
  vi.mocked(ipc.setDeleteAudioAfterRetranscribe).mockClear().mockResolvedValue(undefined);
  vi.mocked(ipc.setDetectCalls).mockClear().mockResolvedValue(undefined);
  vi.mocked(ipc.setAutoStopAfterCall).mockClear().mockResolvedValue(undefined);
  vi.mocked(ipc.setActiveModel).mockClear().mockResolvedValue(undefined);
  vi.mocked(ipc.setRetranscribeModel).mockClear().mockResolvedValue(undefined);
  vi.mocked(ipc.deleteModel).mockClear().mockResolvedValue(undefined);
  vi.mocked(ipc.storageStats).mockClear().mockResolvedValue({
    note_count: 0,
    notes_bytes: 0,
    audio_bytes: 0,
    audio_secs: 0,
  });
  vi.mocked(ipc.listEnhanceProviders).mockClear().mockResolvedValue([
    { id: "local", name: "On this Mac", available: false, version: null },
    { id: "claude", name: "Claude Code", available: true, version: "2.0.0 (Claude Code)" },
    { id: "codex", name: "Codex", available: false, version: null },
  ]);
});

/**
 * Every section stays mounted (visibility is CSS-only), and since stage 4.1 a
 * downloaded model renders BOTH in its function section and on Storage - so
 * model-row queries must be scoped to one section or they match twice.
 */
function section(id: string): HTMLElement {
  const el = document.getElementById(id);
  if (!el) throw new Error(`section ${id} not rendered`);
  return el;
}

describe("SettingsDialog: Dock visibility (stage 3)", () => {
  it("persists menu-bar-only mode from General settings", async () => {
    render(<SettingsDialog open onOpenChange={() => {}} />);
    const toggle = await screen.findByRole("checkbox", { name: /show in dock/i });

    expect(toggle).toHaveAttribute("aria-checked", "true");
    fireEvent.click(toggle);

    await waitFor(() => expect(ipc.setShowInDock).toHaveBeenCalledWith(false));
    expect(toggle).toHaveAttribute("aria-checked", "false");
  });
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
      name: /suggest recording when a call starts/i,
    });
    const autoStop = screen.getByRole("checkbox", {
      name: /stop when the call ends/i,
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
      name: /suggest recording when a call starts/i,
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

    const checkbox = await screen.findByRole("checkbox", { name: /keep audio files/i });
    expect(checkbox).toHaveAttribute("aria-checked", "true");

    fireEvent.click(checkbox);

    await waitFor(() => expect(ipc.setKeepAudio).toHaveBeenCalledWith(false));
    await waitFor(() => expect(checkbox).toHaveAttribute("aria-checked", "false"));
  });

  it("reverts the checkbox and shows an inline error when setKeepAudio rejects", async () => {
    vi.mocked(ipc.setKeepAudio).mockRejectedValueOnce(new Error("disk write failed"));

    render(<SettingsDialog open onOpenChange={() => {}} />);

    const checkbox = await screen.findByRole("checkbox", { name: /keep audio files/i });
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
      name: /run automatically after each call/i,
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
      name: /run automatically after each call/i,
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
      name: /delete audio after re-transcribing/i,
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
      name: /delete audio after re-transcribing/i,
    });
    fireEvent.click(checkbox);

    await waitFor(() => expect(ipc.setDeleteAudioAfterRetranscribe).toHaveBeenCalledWith(true));
    await waitFor(() => expect(checkbox).toHaveAttribute("aria-checked", "false"));
    await screen.findByText(/disk write failed/i);
  });

  it("renders the delete toggle's irreversibility copy plainly, for every re-transcribe not just automatic", async () => {
    render(<SettingsDialog open onOpenChange={() => {}} />);

    // Stage 4.1 copy: one line, but both halves of the contract survive --
    // it fires on manual runs too, and it is irreversible.
    await screen.findByText(/applies to manual runs too/i);
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
      name: /run automatically after each call/i,
    });
    const deleteCheckbox = await screen.findByRole("checkbox", {
      name: /delete audio after re-transcribing/i,
    });
    expect(autoCheckbox).toBeDisabled();
    expect(deleteCheckbox).not.toBeDisabled();

    // Exactly one keep-audio hint -- on the auto row only.
    const notes = screen.getAllByText(/turn on "keep audio files" first/i);
    expect(notes).toHaveLength(1);
  });

  it("enables both toggles when keep_audio is on", async () => {
    render(<SettingsDialog open onOpenChange={() => {}} />);

    const autoCheckbox = await screen.findByRole("checkbox", {
      name: /run automatically after each call/i,
    });
    const deleteCheckbox = await screen.findByRole("checkbox", {
      name: /delete audio after re-transcribing/i,
    });
    expect(autoCheckbox).not.toBeDisabled();
    expect(deleteCheckbox).not.toBeDisabled();
    expect(screen.queryByText(/turn on "keep audio files" first/i)).not.toBeInTheDocument();
  });

  it("keeps the delete toggle enabled even when keep_audio is off, so an old session's audio can still be deleted after a manual re-transcribe", async () => {
    vi.mocked(ipc.getConfig).mockResolvedValue({ ...baseConfig, keep_audio: false });

    render(<SettingsDialog open onOpenChange={() => {}} />);

    const deleteCheckbox = await screen.findByRole("checkbox", {
      name: /delete audio after re-transcribing/i,
    });
    expect(deleteCheckbox).not.toBeDisabled();
    expect(deleteCheckbox).not.toHaveAttribute("aria-checked", "true");
  });
});

describe("SettingsDialog: whisper model row (stage 0.7 Task 5)", () => {
  const models: ModelInfo[] = [
    {
      id: "parakeet-tdt-0.6b-v3-int8",
      name: "Live transcription",
      technical_name: "Parakeet TDT 0.6B v3 (int8)",
      tagline: "required for recording",
      size_mb: 671,
      downloaded: true,
      active: true,
      role: "live",
      size_on_disk_bytes: 487_000_000,
      retired: false,
    },
    {
      id: "whisper-large-v3-turbo-q8",
      name: "Balanced",
      technical_name: null,
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

    const transcription = section("settings-transcription");
    await within(transcription).findByText("Balanced");

    // The live-engine row still gets its normal Active affordance.
    const parakeetRow = within(transcription).getByText("Live transcription").closest("li")!;
    expect(within(parakeetRow).getByText(/active/i)).toBeInTheDocument();

    // Whisper is downloaded but must never show Use or Active: it can't
    // become the live engine. Being downloaded surfaces as a selection
    // radio here; deletion lives on Storage (stage 4.1), not on this row.
    const whisperRow = within(transcription).getByText("Balanced").closest("li")!;
    expect(within(whisperRow).queryByRole("button", { name: /use/i })).not.toBeInTheDocument();
    expect(within(whisperRow).queryByText(/^active$/i)).not.toBeInTheDocument();
    expect(within(whisperRow).getByRole("radio")).toBeInTheDocument();
    expect(within(whisperRow).queryByRole("button", { name: /delete/i })).not.toBeInTheDocument();

    const storageRow = within(section("settings-storage")).getByText("Accurate pass · Balanced").closest("li")!;
    expect(within(storageRow).getByRole("button", { name: /delete/i })).toBeInTheDocument();
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
    name: "Live transcription",
    technical_name: "Parakeet TDT 0.6B v3 (int8)",
    tagline: "required for recording",
    size_mb: 671,
    downloaded: true,
    active: true,
    role: "live",
    size_on_disk_bytes: 487_000_000,
    retired: false,
  };

  const whisperQ8: ModelInfo = {
    id: "whisper-large-v3-turbo-q8",
    name: "Balanced",
    technical_name: null,
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
    name: "Best quality",
    technical_name: null,
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
    technical_name: null,
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

    const currentRadio = await screen.findByRole("radio", { name: "Balanced" });
    expect(currentRadio).toBeChecked();

    const otherRadio = screen.getByRole("radio", { name: "Best quality" });
    expect(otherRadio).not.toBeChecked();

    fireEvent.click(otherRadio);

    await waitFor(() => expect(ipc.setRetranscribeModel).toHaveBeenCalledWith(whisperLargeV3.id));
    await waitFor(() => expect(otherRadio).toBeChecked());
    expect(currentRadio).not.toBeChecked();
  });

  it("renders a retired model (still on disk) on Storage only, with size and delete, no radio, no download", async () => {
    vi.mocked(ipc.listModels).mockResolvedValue([parakeet, whisperQ8, whisperLargeQ5Retired]);

    render(<SettingsDialog open onOpenChange={() => {}} />);

    // Stage 4.1: Transcription shows only the offered lineup; a retired
    // entry surfaces solely on Storage, where it can still be deleted.
    const transcription = section("settings-transcription");
    await within(transcription).findByText("Balanced");
    expect(
      within(transcription).queryByText(/whisper large-v3 \(older version\)/i),
    ).not.toBeInTheDocument();

    const row = within(section("settings-storage"))
      .getByText(/whisper large-v3 \(older version\)/i)
      .closest("li")!;
    expect(within(row).queryByRole("radio")).not.toBeInTheDocument();
    expect(within(row).queryByRole("button", { name: /download/i })).not.toBeInTheDocument();
    expect(within(row).getByRole("button", { name: /delete/i })).toBeInTheDocument();
    expect(within(row).getByText(/no longer offered/i)).toBeInTheDocument();
    // Storage states the REAL on-disk size (1_100_000_000 bytes), not the
    // advertised size_mb.
    expect(within(row).getByText("1.1 GB")).toBeInTheDocument();

    // The still-offered rows are unaffected by a retired sibling being present.
    expect(screen.getByRole("radio", { name: "Balanced" })).toBeInTheDocument();
  });

  it("requires confirmation before deleting, and states the real freed size", async () => {
    vi.mocked(ipc.listModels).mockResolvedValue([parakeet, whisperQ8]);

    render(<SettingsDialog open onOpenChange={() => {}} />);

    const row = (await within(section("settings-storage")).findByText("Accurate pass · Balanced")).closest("li")!;
    const deleteButton = within(row).getByRole("button", { name: /delete balanced/i });

    fireEvent.click(deleteButton);
    expect(ipc.deleteModel).not.toHaveBeenCalled();

    // 874_188_075 bytes (the real on-disk size, not the advertised size_mb)
    // rounds to 874 MB.
    const confirmButton = await within(row).findByRole("button", { name: /delete 874 mb\?/i });
    fireEvent.click(confirmButton);

    await waitFor(() => expect(ipc.deleteModel).toHaveBeenCalledWith(whisperQ8.id));
  });

  it("renders no delete control on the live model row, even on Storage", async () => {
    vi.mocked(ipc.listModels).mockResolvedValue([parakeet, whisperQ8]);

    render(<SettingsDialog open onOpenChange={() => {}} />);

    const parakeetRow = (
      await within(section("settings-storage")).findByText("Live transcription")
    ).closest("li")!;
    expect(within(parakeetRow).queryByRole("button", { name: /delete/i })).not.toBeInTheDocument();
  });

  it("offers Download and no selection control for a not-downloaded re-transcribe model", async () => {
    vi.mocked(ipc.listModels).mockResolvedValue([parakeet, whisperLargeV3NotDownloaded]);

    render(<SettingsDialog open onOpenChange={() => {}} />);

    const row = (await screen.findByText("Best quality")).closest("li")!;
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

    const row = (await within(section("settings-storage")).findByText("Accurate pass · Balanced")).closest("li")!;
    fireEvent.click(within(row).getByRole("button", { name: /delete balanced/i }));
    fireEvent.click(await within(row).findByRole("button", { name: /delete 874 mb\?/i }));

    await waitFor(() => expect(ipc.deleteModel).toHaveBeenCalledWith(whisperQ8.id));
    await waitFor(() => {
      // Gone from Storage (only downloaded models weigh anything there),
      // back to a Download offer on Transcription.
      expect(within(section("settings-storage")).queryByText("Accurate pass · Balanced")).not.toBeInTheDocument();
      const refreshedRow = within(section("settings-transcription"))
        .getByText("Balanced")
        .closest("li")!;
      expect(within(refreshedRow).getByRole("button", { name: /download/i })).toBeInTheDocument();
      expect(within(refreshedRow).queryByRole("radio")).not.toBeInTheDocument();
    });
  });

  it("formats GB sizes decimally, matching the README (advertised on Transcription, on-disk on Storage)", async () => {
    vi.mocked(ipc.listModels).mockResolvedValue([parakeet, whisperLargeQ5Retired, whisperLargeV3]);

    render(<SettingsDialog open onOpenChange={() => {}} />);

    // Transcription quotes the advertised size_mb (3095 MB -> "3.1 GB",
    // inside the tagline line); Storage quotes real on-disk bytes
    // (1_100_000_000 -> "1.1 GB" on the retired row). Both decimal, never 1024.
    await within(section("settings-transcription")).findByText(/3\.1 GB/);
    await within(section("settings-storage")).findByText("1.1 GB");
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

    await within(section("settings-transcription")).findByText("Balanced");
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

  it("shows the total on-disk model size in the Storage heading, downloaded rows only", async () => {
    vi.mocked(ipc.listModels).mockResolvedValue([parakeet, whisperQ8, whisperLargeV3NotDownloaded]);

    render(<SettingsDialog open onOpenChange={() => {}} />);

    // 487_000_000 + 874_188_075 real bytes -> "1.36 GB"; the not-downloaded
    // large-v3 contributes nothing and renders no Storage row at all.
    const storage = section("settings-storage");
    const heading = await within(storage).findByRole("heading", { name: "Models" });
    expect(within(heading.parentElement!).getByText("1.36 GB")).toBeInTheDocument();
    expect(within(storage).queryByText("Best quality")).not.toBeInTheDocument();
  });

  it("disarms an armed delete confirmation when the dialog closes and reopens", async () => {
    // Security-review finding: the dialog stays mounted for the app's whole
    // lifetime (only `open` toggles visibility) and `deleteConfirmId` used to
    // survive that, so arming delete on a model, closing without choosing
    // (the natural way to back out), and reopening later left the row one
    // click from `deleteModel` with no fresh gate.
    vi.mocked(ipc.listModels).mockResolvedValue([parakeet, whisperQ8]);

    const { rerender } = render(<SettingsDialog open onOpenChange={() => {}} />);

    const row = (await within(section("settings-storage")).findByText("Accurate pass · Balanced")).closest("li")!;
    fireEvent.click(within(row).getByRole("button", { name: /delete balanced/i }));
    await within(row).findByRole("button", { name: /delete 874 mb\?/i });

    rerender(<SettingsDialog open={false} onOpenChange={() => {}} />);
    rerender(<SettingsDialog open onOpenChange={() => {}} />);

    const reopenedRow = (
      await within(section("settings-storage")).findByText("Accurate pass · Balanced")
    ).closest("li")!;
    // Back to the trash icon, not the armed "Delete 874 MB?" button.
    const trashButton = await within(reopenedRow).findByRole("button", {
      name: /delete balanced/i,
    });
    expect(within(reopenedRow).queryByRole("button", { name: /delete 874 mb\?/i })).not.toBeInTheDocument();

    // One click on the (now disarmed) trash icon must ask again, not delete.
    fireEvent.click(trashButton);
    expect(ipc.deleteModel).not.toHaveBeenCalled();
    await within(reopenedRow).findByRole("button", { name: /delete 874 mb\?/i });
  });
});

describe("SettingsDialog: AI enhance provider (stage 0.11)", () => {
  // The summary provider rows live on the Summaries page (stage 4.1): the
  // local row is the local model row (download/progress inline) with its
  // selection radio, so picking and downloading never span pages. Deletion
  // lives on Storage with every other downloaded model.
  const qwen: ModelInfo = {
    id: "qwen3.5-4b-q4-k-m",
    name: "Local summary model",
    technical_name: "Qwen3.5 4B (Q4_K_M)",
    tagline: "nothing leaves this Mac, simpler summaries",
    size_mb: 2740,
    downloaded: false,
    active: false,
    role: "summary",
    size_on_disk_bytes: null,
    retired: false,
  };
  const qwenDownloaded: ModelInfo = {
    ...qwen,
    downloaded: true,
    size_on_disk_bytes: 2_740_000_000,
  };

  async function openSummariesSection() {
    render(<SettingsDialog open onOpenChange={() => {}} />);
    fireEvent.click(await screen.findByRole("button", { name: /^summaries$/i }));
  }

  it("lists probed providers with availability and picks one via the radio", async () => {
    vi.mocked(ipc.listModels).mockResolvedValue([qwen]);
    await openSummariesSection();

    await screen.findByText("2.0.0");
    expect(screen.getByText(/not installed \(no `codex` CLI found\)/i)).toBeInTheDocument();

    const claudeRadio = screen.getByRole("radio", { name: "Claude Code" });
    expect(claudeRadio).toBeChecked();
    const codexRadio = screen.getByRole("radio", { name: "Codex" });
    expect(codexRadio).toBeDisabled();
  });

  it("renders the local row as one merged download-and-select row", async () => {
    vi.mocked(ipc.listModels).mockResolvedValue([qwen]);
    await openSummariesSection();

    // Not downloaded: the row offers Download right here (no pointer to a
    // different page) and its selection radio is disabled until it is ready.
    const row = (await screen.findByText("On this Mac")).closest("li")!;
    expect(within(row).getByRole("button", { name: /download/i })).toBeInTheDocument();
    expect(within(row).getByRole("radio", { name: "On this Mac" })).toBeDisabled();
    const technicalName = within(row).getByText("Qwen3.5 4B (Q4_K_M)");
    expect(technicalName).toHaveClass("text-[10px]", "text-muted-foreground/55");
    expect(within(row).getByText(/2\.74 GB/)).toBeInTheDocument();
  });

  it("offers an enabled radio once the local model is downloaded, with deletion on Storage", async () => {
    vi.mocked(ipc.listModels).mockResolvedValue([qwenDownloaded]);
    vi.mocked(ipc.listEnhanceProviders).mockResolvedValue([
      { id: "local", name: "On this Mac", available: true, version: "on-device · Metal" },
      { id: "claude", name: "Claude Code", available: true, version: "2.0.0" },
      { id: "codex", name: "Codex", available: false, version: null },
    ]);
    await openSummariesSection();

    const row = (
      await within(section("settings-summaries")).findByText("On this Mac")
    ).closest("li")!;
    expect(within(row).queryByRole("button", { name: /download/i })).not.toBeInTheDocument();
    expect(within(row).queryByRole("button", { name: /delete/i })).not.toBeInTheDocument();
    expect(within(row).getByRole("radio", { name: "On this Mac" })).not.toBeDisabled();

    const storageRow = within(section("settings-storage"))
      .getByText("Summaries · On this Mac")
      .closest("li")!;
    expect(
      within(storageRow).getByRole("button", { name: /delete local summary model/i }),
    ).toBeInTheDocument();
  });

  it("persists a provider switch", async () => {
    vi.mocked(ipc.listModels).mockResolvedValue([qwenDownloaded]);
    vi.mocked(ipc.listEnhanceProviders).mockResolvedValue([
      { id: "local", name: "On this Mac", available: true, version: "on-device · Metal" },
      { id: "claude", name: "Claude Code", available: true, version: "2.0.0" },
      { id: "codex", name: "Codex", available: true, version: "codex-cli 0.149.0" },
    ]);
    await openSummariesSection();

    const codexRadio = await screen.findByRole("radio", { name: "Codex" });
    fireEvent.click(codexRadio);
    await waitFor(() =>
      expect(ipc.setEnhanceProvider).toHaveBeenCalledExactlyOnceWith("codex"),
    );
    expect(codexRadio).toBeChecked();
  });

  it("offers the downloaded local provider without a model override menu", async () => {
    vi.mocked(ipc.getConfig).mockResolvedValue({ ...baseConfig, enhance_provider: "local" });
    vi.mocked(ipc.listModels).mockResolvedValue([qwenDownloaded]);
    vi.mocked(ipc.listEnhanceProviders).mockResolvedValue([
      { id: "local", name: "On this Mac", available: true, version: "on-device · Metal" },
      { id: "claude", name: "Claude Code", available: true, version: "2.0.0" },
    ]);
    await openSummariesSection();

    expect(await screen.findByRole("radio", { name: "On this Mac" })).toBeChecked();
    expect(screen.queryByRole("combobox", { name: /local.*model/i })).not.toBeInTheDocument();
  });
});

describe("SettingsDialog: enhance model override (stage 0.11)", () => {
  it("persists a model pick and clears it back to Default", async () => {
    render(<SettingsDialog open onOpenChange={() => {}} />);
    fireEvent.click(await screen.findByRole("button", { name: /^summaries$/i }));

    const select = await screen.findByRole("combobox", { name: /claude code model/i });
    fireEvent.change(select, { target: { value: "fable" } });
    await waitFor(() =>
      expect(ipc.setEnhanceModel).toHaveBeenCalledExactlyOnceWith("claude", "fable"),
    );

    fireEvent.change(select, { target: { value: "" } });
    await waitFor(() => expect(ipc.setEnhanceModel).toHaveBeenLastCalledWith("claude", ""));
  });
});

describe("SettingsDialog: model titles and calendar context", () => {
  it("persists the title-renaming opt-in", async () => {
    render(<SettingsDialog open onOpenChange={() => {}} />);

    const toggle = await screen.findByRole("checkbox", { name: /allow renaming notes/i });
    expect(toggle).toHaveAttribute("aria-checked", "false");
    fireEvent.click(toggle);

    await waitFor(() => expect(ipc.setEnhanceRenameTitle).toHaveBeenCalledWith(true));
    expect(toggle).toHaveAttribute("aria-checked", "true");
    // The copy must state the rule-6 boundary plainly.
    await screen.findByText(/yours are never touched/i);
  });

  it("offers calendar context for both cloud providers", async () => {
    render(<SettingsDialog open onOpenChange={() => {}} />);

    // baseConfig selects claude: the toggle is live and persists.
    const toggle = await screen.findByRole("checkbox", { name: /use calendar for context/i });
    expect(toggle).not.toBeDisabled();
    fireEvent.click(toggle);
    await waitFor(() => expect(ipc.setEnhanceUseCalendar).toHaveBeenCalledWith(true));
  });

  it("keeps the calendar toggle live for codex (the lookup runs via the claude CLI)", async () => {
    vi.mocked(ipc.getConfig).mockResolvedValue({ ...baseConfig, enhance_provider: "codex" });
    render(<SettingsDialog open onOpenChange={() => {}} />);

    const toggle = await screen.findByRole("checkbox", { name: /use calendar for context/i });
    expect(toggle).not.toBeDisabled();
  });

  it("disables the calendar toggle for the local provider: local means no cloud call", async () => {
    vi.mocked(ipc.getConfig).mockResolvedValue({ ...baseConfig, enhance_provider: "local" });
    render(<SettingsDialog open onOpenChange={() => {}} />);

    const toggle = await screen.findByRole("checkbox", { name: /use calendar for context/i });
    expect(toggle).toBeDisabled();
  });
});

describe("SettingsDialog: five-page navigation (stage 4.1)", () => {
  it("offers exactly the five task-shaped sections in the nav", async () => {
    render(<SettingsDialog open onOpenChange={() => {}} />);

    await screen.findByRole("button", { name: /^general$/i });
    for (const name of [/^recording$/i, /^transcription$/i, /^summaries$/i, /^storage$/i]) {
      expect(screen.getByRole("button", { name })).toBeInTheDocument();
    }
    // The old download-center page is gone.
    expect(screen.queryByRole("button", { name: /^models$/i })).not.toBeInTheDocument();
  });

  it("keeps every toggle next to what it configures, and Storage free of toggles", async () => {
    render(<SettingsDialog open onOpenChange={() => {}} />);
    await screen.findByRole("checkbox", { name: /run automatically after each call/i });

    // General: only the app itself.
    const general = section("settings-general");
    expect(within(general).getByRole("checkbox", { name: /show in dock/i })).toBeInTheDocument();
    expect(within(general).queryAllByRole("checkbox")).toHaveLength(1);

    // Recording: detection with its dependent auto-stop, and audio retention.
    const recording = section("settings-recording");
    for (const name of [
      /suggest recording when a call starts/i,
      /stop when the call ends/i,
      /keep audio files/i,
    ]) {
      expect(within(recording).getByRole("checkbox", { name })).toBeInTheDocument();
    }

    // Transcription: the accurate pass's two automation toggles sit under
    // the whisper choice they configure.
    const transcription = section("settings-transcription");
    for (const name of [
      /run automatically after each call/i,
      /delete audio after re-transcribing/i,
    ]) {
      expect(within(transcription).getByRole("checkbox", { name })).toBeInTheDocument();
    }

    // Summaries: provider choice plus the three summary-behavior toggles.
    const summaries = section("settings-summaries");
    for (const name of [
      /summarize after each call/i,
      /allow renaming notes/i,
      /use calendar for context/i,
    ]) {
      expect(within(summaries).getByRole("checkbox", { name })).toBeInTheDocument();
    }

    // Storage only weighs and deletes; it configures nothing.
    expect(within(section("settings-storage")).queryAllByRole("checkbox")).toHaveLength(0);
  });
});

describe("SettingsDialog: storage page (stage 4.1)", () => {
  it("weighs notes against audio and points at the notes folder", async () => {
    vi.mocked(ipc.storageStats).mockResolvedValue({
      note_count: 142,
      notes_bytes: 340_000,
      audio_bytes: 1_857_000_000,
      audio_secs: 345_600,
    });

    render(<SettingsDialog open onOpenChange={() => {}} />);

    const storage = section("settings-storage");
    // The whole point of the group: years of notes are kilobytes, the
    // audio next to them is gigabytes.
    await within(storage).findByText("142 calls, one Markdown file each");
    expect(within(storage).getByText("340 KB")).toBeInTheDocument();
    expect(within(storage).getByText("96 hours")).toBeInTheDocument();
    // Row value and the group total both round to the same figure here.
    expect(within(storage).getAllByText("1.86 GB")).toHaveLength(2);
    // The folder pointer keeps the files-are-yours promise in view.
    expect(within(storage).getByText(/yours to move, sync or delete/i)).toBeInTheDocument();
    expect(within(storage).getByText("/tmp/calls")).toBeInTheDocument();
  });

  it("says plainly when no audio is kept", async () => {
    vi.mocked(ipc.storageStats).mockResolvedValue({
      note_count: 3,
      notes_bytes: 9_000,
      audio_bytes: 0,
      audio_secs: 0,
    });

    render(<SettingsDialog open onOpenChange={() => {}} />);

    const storage = section("settings-storage");
    await within(storage).findByText("3 calls, one Markdown file each");
    expect(within(storage).getByText("No audio kept")).toBeInTheDocument();
  });
});
