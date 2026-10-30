import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { SettingsDialog } from "@/components/SettingsDialog";
import * as ipc from "@/lib/ipc";
import type { AppConfig, ModelInfo } from "@/lib/ipc";

vi.mock("@/lib/ipc", () => ({
  aiSetupStatus: vi.fn().mockResolvedValue(null),
  getAiProviderStatus: vi.fn().mockResolvedValue([
    { id: "claude", name: "Claude Code", installed_version: "2.1.236", latest_version: "2.1.314", update_available: true },
    { id: "codex", name: "Codex", installed_version: "0.152.0", latest_version: "0.166.0", update_available: true },
  ]),
  setupAi: vi.fn().mockResolvedValue(undefined),
  cancelAiSetup: vi.fn().mockResolvedValue(undefined),
  getPrompts: vi.fn().mockResolvedValue({ summary_instructions: "", summary_styles: [], quick_questions: [] }),
  savePrompts: vi.fn().mockResolvedValue({ summary_instructions: "", summary_styles: [], quick_questions: [] }),
  restoreDefaultPrompts: vi.fn().mockResolvedValue({ summary_instructions: "", summary_styles: [], quick_questions: [] }),
  getConfig: vi.fn(),
  getLaunchAtLogin: vi.fn(),
  setLaunchAtLogin: vi.fn(),
  listModels: vi.fn().mockResolvedValue([]),
  downloadModel: vi.fn().mockResolvedValue(undefined),
  setRetranscribeModel: vi.fn().mockResolvedValue(undefined),
  deleteModel: vi.fn().mockResolvedValue(undefined),
  setOutputDir: vi.fn().mockResolvedValue(undefined),
  setTheme: vi.fn().mockResolvedValue(undefined),
  setShowInDock: vi.fn().mockResolvedValue(undefined),
  setKeepAudio: vi.fn().mockResolvedValue(undefined),
  setDetectCalls: vi.fn().mockResolvedValue(undefined),
  setEnhanceProvider: vi.fn().mockResolvedValue(undefined),
  setEnhanceModel: vi.fn().mockResolvedValue(undefined),
  setAutoEnhance: vi.fn().mockResolvedValue(undefined),
  setEnhanceRenameTitle: vi.fn().mockResolvedValue(undefined),
  setEnhanceUseCalendar: vi.fn().mockResolvedValue(undefined),
  listEnhanceProviders: vi.fn(),
  setAutoStopAfterCall: vi.fn().mockResolvedValue(undefined),
  storageStats: vi.fn(),
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
  technical_name: "Whisper large-v3-turbo (q8)",
  tagline: "about 12x realtime, slightly weaker on non-English",
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
  technical_name: "Whisper large-v3",
  tagline: "strongest on non-English speech, slower",
  size_mb: 3095,
  downloaded: true,
  active: false,
  role: "retranscribe",
  size_on_disk_bytes: 3_095_033_483,
  retired: false,
};

// Retired entries only ever arrive while a copy is still on disk.
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

const qwenRetired: ModelInfo = {
  id: "qwen3.5-4b-q4-k-m",
  name: "Local summary model",
  technical_name: "Qwen3.5 4B (Q4_K_M)",
  tagline: null,
  size_mb: 2740,
  downloaded: true,
  active: false,
  role: "summary",
  size_on_disk_bytes: 2_740_000_000,
  retired: true,
};

const onlyClaude = [
  { id: "claude", name: "Claude Code", available: true, version: "2.0.0 (Claude Code)", models: [
    { value: "claude-fable-5[1m]", label: "Fable" },
    { value: "haiku", label: "Haiku" },
  ] },
  { id: "codex", name: "Codex", available: false, version: null },
];

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(ipc.getLaunchAtLogin).mockResolvedValue("disabled");
  vi.mocked(ipc.setLaunchAtLogin).mockImplementation(async (enabled) => enabled ? "enabled" : "disabled");
  vi.mocked(ipc.getConfig).mockResolvedValue(baseConfig);
  vi.mocked(ipc.listModels).mockResolvedValue([parakeet, whisperQ8, whisperLargeV3]);
  vi.mocked(ipc.storageStats).mockResolvedValue({
    note_count: 0,
    notes_bytes: 0,
    audio_bytes: 0,
    audio_secs: 0,
  });
  vi.mocked(ipc.listEnhanceProviders).mockResolvedValue(onlyClaude);
});

/** Model rows render in their section AND on Storage: scope the query. */
function section(id: string): HTMLElement {
  const el = document.getElementById(id);
  if (!el) throw new Error(`section ${id} not rendered`);
  return el;
}

describe("SettingsDialog: one page, no engine names", () => {
  it("has no left rail and no page switcher: every section is on one scrolling page", async () => {
    render(<SettingsDialog open onOpenChange={() => {}} />);
    await screen.findByRole("checkbox", { name: /show in dock/i });

    for (const id of [
      "settings-general",
      "settings-recording",
      "settings-transcription",
      "settings-summaries",
      "settings-storage",
    ]) {
      expect(section(id)).toBeVisible();
    }
    for (const name of [/^general$/i, /^recording$/i, /^transcription$/i, /^summaries$/i, /^storage$/i]) {
      expect(screen.queryByRole("button", { name })).not.toBeInTheDocument();
    }
    expect(screen.getByRole("button", { name: /notes/i })).toBeVisible();
  });

  it("never shows an engine name or a quantization suffix", async () => {
    vi.mocked(ipc.listModels).mockResolvedValue([parakeet, whisperQ8, whisperLargeV3, whisperLargeQ5Retired]);
    render(<SettingsDialog open onOpenChange={() => {}} />);
    await screen.findByRole("radio", { name: "Balanced" });

    expect(document.body.textContent).not.toMatch(/parakeet|q8|q5|int8|tdt|whisper large-v3-turbo/i);
  });

  it("offers none of the pipeline's plumbing as a setting: no format, no auto-pass or delete-audio toggles", async () => {
    render(<SettingsDialog open onOpenChange={() => {}} />);
    await screen.findByRole("checkbox", { name: /keep audio recordings/i });

    expect(screen.queryByRole("button", { name: /^aac$/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^flac$/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("checkbox", { name: /run automatically/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("checkbox", { name: /delete audio/i })).not.toBeInTheDocument();
  });
});

describe("SettingsDialog: general", () => {
  it("persists menu-bar-only mode", async () => {
    render(<SettingsDialog open onOpenChange={() => {}} />);
    const toggle = await screen.findByRole("checkbox", { name: /show in dock/i });
    expect(toggle).toHaveAttribute("aria-checked", "true");
    fireEvent.click(toggle);
    await waitFor(() => expect(ipc.setShowInDock).toHaveBeenCalledWith(false));
    expect(toggle).toHaveAttribute("aria-checked", "false");
  });

  it("switches the theme and rolls back when the save rejects", async () => {
    // An explicit "light" start: rolling back to "system" would consult
    // window.matchMedia, which jsdom does not implement.
    vi.mocked(ipc.getConfig).mockResolvedValue({ ...baseConfig, theme: "light" });
    vi.mocked(ipc.setTheme).mockRejectedValueOnce(new Error("disk full"));
    render(<SettingsDialog open onOpenChange={() => {}} />);
    const dark = await screen.findByRole("button", { name: /^dark$/i });
    fireEvent.click(dark);
    await waitFor(() => expect(ipc.setTheme).toHaveBeenCalledWith("dark"));
    await screen.findByText(/disk full/);
    expect(document.documentElement.classList.contains("dark")).toBe(false);
  });
});

describe("SettingsDialog: calls", () => {
  it("persists detection and enables the dependent auto-stop row", async () => {
    let persisted = { ...baseConfig };
    vi.mocked(ipc.getConfig).mockImplementation(async () => ({ ...persisted }));
    vi.mocked(ipc.setDetectCalls).mockImplementation(async (enabled: boolean) => {
      persisted = { ...persisted, detect_calls: enabled };
    });
    render(<SettingsDialog open onOpenChange={() => {}} />);

    const autoStop = await screen.findByRole("checkbox", { name: /stop when the call ends/i });
    expect(autoStop).toBeEnabled();
    fireEvent.click(screen.getByRole("checkbox", { name: /suggest recording/i }));
    await waitFor(() => expect(ipc.setDetectCalls).toHaveBeenCalledWith(false));
    await waitFor(() => expect(autoStop).toBeDisabled());
  });

  it("reverts keep-audio and shows the error inline when the save rejects", async () => {
    vi.mocked(ipc.setKeepAudio).mockRejectedValueOnce(new Error("read-only config"));
    render(<SettingsDialog open onOpenChange={() => {}} />);
    const toggle = await screen.findByRole("checkbox", { name: /keep audio recordings/i });
    fireEvent.click(toggle);
    await screen.findByText(/read-only config/);
    expect(toggle).toHaveAttribute("aria-checked", "true");
  });
});

describe("SettingsDialog: accurate transcript", () => {
  it("selects a downloaded model via its radio and persists it", async () => {
    let persisted = { ...baseConfig };
    vi.mocked(ipc.getConfig).mockImplementation(async () => ({ ...persisted }));
    vi.mocked(ipc.setRetranscribeModel).mockImplementation(async (id: string) => {
      persisted = { ...persisted, retranscribe_model: id };
    });
    render(<SettingsDialog open onOpenChange={() => {}} />);

    const current = await screen.findByRole("radio", { name: "Balanced" });
    expect(current).toBeChecked();
    const other = screen.getByRole("radio", { name: "Best quality" });
    fireEvent.click(other);
    await waitFor(() => expect(ipc.setRetranscribeModel).toHaveBeenCalledWith(whisperLargeV3.id));
    await waitFor(() => expect(other).toBeChecked());
  });

  it("offers Download instead of a radio for a model not on disk, and explains a missing selection", async () => {
    vi.mocked(ipc.listModels).mockResolvedValue([
      parakeet,
      { ...whisperQ8, downloaded: false, size_on_disk_bytes: null },
      whisperLargeV3,
    ]);
    render(<SettingsDialog open onOpenChange={() => {}} />);

    const transcription = section("settings-transcription");
    const row = (await within(transcription).findByText("Balanced")).closest("li")!;
    expect(within(row).queryByRole("radio")).not.toBeInTheDocument();
    fireEvent.click(within(row).getByRole("button", { name: /download/i }));
    await waitFor(() => expect(ipc.downloadModel).toHaveBeenCalledWith(whisperQ8.id));
    expect(within(transcription).getByText(/not downloaded/i)).toBeInTheDocument();
  });

  it("surfaces the live model only while it is missing, in plain words", async () => {
    vi.mocked(ipc.listModels).mockResolvedValue([
      { ...parakeet, downloaded: false, size_on_disk_bytes: null },
      whisperQ8,
      whisperLargeV3,
    ]);
    render(<SettingsDialog open onOpenChange={() => {}} />);

    const transcription = section("settings-transcription");
    const row = (await within(transcription).findByText("Live speech recognition")).closest("li")!;
    fireEvent.click(within(row).getByRole("button", { name: /download/i }));
    await waitFor(() => expect(ipc.downloadModel).toHaveBeenCalledWith(parakeet.id));
  });

  it("does not mention the live model at all once it is on disk", async () => {
    render(<SettingsDialog open onOpenChange={() => {}} />);
    await screen.findByRole("radio", { name: "Balanced" });
    expect(within(section("settings-transcription")).queryByText(/live speech recognition/i)).not.toBeInTheDocument();
  });
});

describe("SettingsDialog: summaries", () => {
  it("names the one installed assistant with its model menu, no radio to choose from", async () => {
    render(<SettingsDialog open onOpenChange={() => {}} />);

    await screen.findByText("Written by Claude Code");
    expect(screen.getByText("2.0.0")).toBeInTheDocument();
    expect(screen.queryByRole("radio", { name: /claude|codex/i })).not.toBeInTheDocument();

    const select = screen.getByRole("combobox", { name: /claude code model/i });
    fireEvent.change(select, { target: { value: "claude-fable-5[1m]" } });
    await waitFor(() => expect(ipc.setEnhanceModel).toHaveBeenCalledExactlyOnceWith("claude", "claude-fable-5[1m]"));
  });

  it("offers a choice only when two assistants are installed", async () => {
    vi.mocked(ipc.listEnhanceProviders).mockResolvedValue([
      { id: "claude", name: "Claude Code", available: true, version: "2.0.0" },
      { id: "codex", name: "Codex", available: true, version: "codex-cli 0.149.0" },
    ]);
    render(<SettingsDialog open onOpenChange={() => {}} />);

    const codex = await screen.findByRole("radio", { name: "Codex" });
    expect(screen.getByRole("radio", { name: "Claude Code" })).toBeChecked();
    fireEvent.click(codex);
    await waitFor(() => expect(ipc.setEnhanceProvider).toHaveBeenCalledExactlyOnceWith("codex"));
    expect(codex).toBeChecked();
  });

  it("offers current Codex models and persists the chosen model", async () => {
    vi.mocked(ipc.listEnhanceProviders).mockResolvedValue([
      { id: "codex", name: "Codex", available: true, version: "codex-cli 0.166.0", models: [
        { value: "gpt-6.1-sol", label: "GPT-6.1 Sol" },
        { value: "gpt-6.1-terra", label: "GPT-6.1 Terra" },
        { value: "gpt-6-luna", label: "GPT-6 Luna" },
      ] },
    ]);
    render(<SettingsDialog open onOpenChange={() => {}} />);

    const select = await screen.findByRole("combobox", { name: "Codex model" });
    expect(within(select).getByRole("option", { name: "GPT-6.1 Sol" })).toHaveValue("gpt-6.1-sol");
    expect(within(select).getByRole("option", { name: "GPT-6.1 Terra" })).toHaveValue("gpt-6.1-terra");
    expect(within(select).getByRole("option", { name: "GPT-6 Luna" })).toHaveValue("gpt-6-luna");
    expect(within(select).queryByRole("option", { name: /spark/i })).not.toBeInTheDocument();
    fireEvent.change(select, { target: { value: "gpt-6.1-sol" } });
    await waitFor(() => expect(ipc.setEnhanceModel).toHaveBeenCalledExactlyOnceWith("codex", "gpt-6.1-sol"));
  });

  it("keeps a saved model visible until the user chooses a replacement", async () => {
    vi.mocked(ipc.getConfig).mockResolvedValue({
      ...baseConfig,
      enhance_models: { claude: "claude-haiku-4-5" },
    });
    render(<SettingsDialog open onOpenChange={() => {}} />);

    const select = await screen.findByRole("combobox", { name: "Claude Code model" });
    expect(select).toHaveValue("claude-haiku-4-5");
    expect(within(select).getByRole("option", { name: "claude-haiku-4-5 (saved; not listed)" })).toBeInTheDocument();
    expect(ipc.setEnhanceModel).not.toHaveBeenCalled();
    fireEvent.change(select, { target: { value: "haiku" } });
    await waitFor(() => expect(ipc.setEnhanceModel).toHaveBeenCalledExactlyOnceWith("claude", "haiku"));
    expect(select).toHaveValue("haiku");
  });

  it("uses the helper catalog instead of offering models unknown to an older Codex", async () => {
    vi.mocked(ipc.listEnhanceProviders).mockResolvedValue([
      { id: "codex", name: "Codex", available: true, version: "codex-cli 0.152.0", models: [
        { value: "account-model", label: "Available to this account" },
      ] },
    ]);
    render(<SettingsDialog open onOpenChange={() => {}} />);
    const select = await screen.findByRole("combobox", { name: "Codex model" });
    expect(within(select).queryByRole("option", { name: "GPT-6.1 Terra" })).not.toBeInTheDocument();
    fireEvent.change(select, { target: { value: "account-model" } });
    await waitFor(() => expect(ipc.setEnhanceModel).toHaveBeenCalledWith("codex", "account-model"));
  });

  it("keeps a saved Codex selection visible and offers Default when discovery fails", async () => {
    vi.mocked(ipc.getConfig).mockResolvedValue({ ...baseConfig, enhance_provider: "codex", enhance_models: { codex: "gpt-6.1-terra" } });
    vi.mocked(ipc.listEnhanceProviders).mockResolvedValue([
      { id: "codex", name: "Codex", available: true, version: "codex-cli 0.152.0", models: null },
    ]);
    render(<SettingsDialog open onOpenChange={() => {}} />);
    const select = await screen.findByRole("combobox", { name: "Codex model" });
    expect(select).toHaveValue("gpt-6.1-terra");
    expect(screen.getByText(/could not load codex models/i)).toBeInTheDocument();
    expect(ipc.setEnhanceModel).not.toHaveBeenCalled();
    fireEvent.change(select, { target: { value: "" } });
    await waitFor(() => expect(ipc.setEnhanceModel).toHaveBeenCalledWith("codex", ""));
  });

  it.each([
    { id: "codex", name: "Codex", model: "gpt-6.1-terra", label: "GPT-6.1 Terra" },
    { id: "claude", name: "Claude Code", model: "claude-fable-5[1m]", label: "Fable" },
  ])("refreshes $name models after an update without changing the provider", async ({ id, name, model, label }) => {
    vi.mocked(ipc.getConfig).mockResolvedValue({ ...baseConfig, enhance_provider: id });
    vi.mocked(ipc.listEnhanceProviders)
      .mockResolvedValueOnce([{ id, name, available: true, version: "old-version", models: [] }])
      .mockResolvedValue([{ id, name, available: true, version: "new-version", models: [{ value: model, label }] }]);
    render(<SettingsDialog open onOpenChange={() => {}} />);
    fireEvent.click(await screen.findByRole("button", { name: `Update ${name}` }));
    await waitFor(() => expect(ipc.setupAi).toHaveBeenCalledExactlyOnceWith(id, true));
    expect(await screen.findByRole("option", { name: label })).toHaveValue(model);
    expect(ipc.setEnhanceProvider).not.toHaveBeenCalled();
    expect(ipc.setEnhanceModel).not.toHaveBeenCalled();
  });

  it("offers Default when the Claude catalog cannot be loaded instead of guessing model aliases", async () => {
    vi.mocked(ipc.listEnhanceProviders).mockResolvedValue([
      { id: "claude", name: "Claude Code", available: true, version: "2.1.236", models: null },
    ]);
    render(<SettingsDialog open onOpenChange={() => {}} />);
    const select = await screen.findByRole("combobox", { name: "Claude Code model" });
    expect(within(select).getAllByRole("option")).toHaveLength(1);
    expect(select).toHaveValue("");
    expect(screen.getByText(/could not load claude code models/i)).toBeInTheDocument();
  });

  it("says plainly when nothing is installed and dims the behavior toggles", async () => {
    vi.mocked(ipc.listEnhanceProviders).mockResolvedValue([
      { id: "claude", name: "Claude Code", available: false, version: null },
      { id: "codex", name: "Codex", available: false, version: null },
    ]);
    render(<SettingsDialog open onOpenChange={() => {}} />);

    await screen.findByText("Connect an AI assistant");
    expect(screen.getByRole("checkbox", { name: /summarize after each call/i })).toBeDisabled();
    expect(screen.getByRole("checkbox", { name: /let summaries title notes/i })).toBeDisabled();
    expect(screen.getByRole("checkbox", { name: /use calendar for context/i })).toBeDisabled();
  });

  it("persists the three behavior toggles", async () => {
    render(<SettingsDialog open onOpenChange={() => {}} />);

    const auto = await screen.findByRole("checkbox", { name: /summarize after each call/i });
    expect(auto).toHaveAttribute("aria-checked", "true");
    fireEvent.click(auto);
    await waitFor(() => expect(ipc.setAutoEnhance).toHaveBeenCalledWith(false));

    fireEvent.click(screen.getByRole("checkbox", { name: /let summaries title notes/i }));
    await waitFor(() => expect(ipc.setEnhanceRenameTitle).toHaveBeenCalledWith(false));
    // The copy must state the rule-6 boundary plainly.
    expect(screen.getByText(/yours are never touched/i)).toBeInTheDocument();

    fireEvent.click(screen.getByRole("checkbox", { name: /use calendar for context/i }));
    await waitFor(() => expect(ipc.setEnhanceUseCalendar).toHaveBeenCalledWith(false));
  });
});

describe("SettingsDialog: storage", () => {
  it("weighs notes against audio and points at the notes folder", async () => {
    vi.mocked(ipc.storageStats).mockResolvedValue({
      note_count: 142,
      notes_bytes: 340_000,
      audio_bytes: 1_857_000_000,
      audio_secs: 345_600,
    });
    render(<SettingsDialog open onOpenChange={() => {}} />);

    const storage = section("settings-storage");
    await within(storage).findByText("142 calls, one Markdown file each");
    expect(within(storage).getByText("340 KB")).toBeInTheDocument();
    expect(within(storage).getByText("96 hours")).toBeInTheDocument();
    expect(within(storage).getAllByText("1.86 GB")).toHaveLength(2);
    expect(within(storage).getByText(/yours to move, sync or delete/i)).toBeInTheDocument();
    expect(within(storage).getByText("/tmp/calls")).toBeInTheDocument();
  });

  it("lists downloaded models by their role, states real sizes, and never offers to delete the live one", async () => {
    vi.mocked(ipc.listModels).mockResolvedValue([parakeet, whisperQ8, whisperLargeQ5Retired, qwenRetired]);
    render(<SettingsDialog open onOpenChange={() => {}} />);

    const storage = section("settings-storage");
    const live = (await within(storage).findByText("Speech recognition (live)")).closest("li")!;
    expect(within(live).queryByRole("button", { name: /delete/i })).not.toBeInTheDocument();
    expect(within(live).getByText("487 MB")).toBeInTheDocument();

    const balanced = within(storage).getByText("Accurate transcript · Balanced").closest("li")!;
    expect(within(balanced).getByRole("button", { name: /delete balanced/i })).toBeInTheDocument();

    // Retired rows: the old whisper and the retired on-device summarizer
    // stay deletable so the disk can be reclaimed.
    const retired = within(storage).getByText(/whisper large-v3 \(older version\)/i).closest("li")!;
    expect(within(retired).getByText(/no longer offered/i)).toBeInTheDocument();
    expect(within(retired).getByText("1.1 GB")).toBeInTheDocument();
    const qwen = within(storage).getByText(/local summary model/i).closest("li")!;
    expect(within(qwen).getByRole("button", { name: /delete/i })).toBeInTheDocument();
    expect(within(qwen).getByText("2.74 GB")).toBeInTheDocument();
    // Never on the accurate-transcript list: it is no longer offered.
    expect(within(section("settings-transcription")).queryByText(/older version/i)).not.toBeInTheDocument();
  });

  it("requires confirmation before deleting, states the real freed size, and disarms on close", async () => {
    const { rerender } = render(<SettingsDialog open onOpenChange={() => {}} />);

    const row = (await within(section("settings-storage")).findByText("Accurate transcript · Balanced")).closest("li")!;
    fireEvent.click(within(row).getByRole("button", { name: /delete balanced/i }));
    expect(ipc.deleteModel).not.toHaveBeenCalled();
    await within(row).findByRole("button", { name: /delete 874 mb\?/i });

    // Closing without choosing disarms the confirm step.
    rerender(<SettingsDialog open={false} onOpenChange={() => {}} />);
    rerender(<SettingsDialog open onOpenChange={() => {}} />);
    const reopened = (await within(section("settings-storage")).findByText("Accurate transcript · Balanced")).closest("li")!;
    expect(within(reopened).queryByRole("button", { name: /delete 874 mb\?/i })).not.toBeInTheDocument();

    fireEvent.click(within(reopened).getByRole("button", { name: /delete balanced/i }));
    fireEvent.click(await within(reopened).findByRole("button", { name: /delete 874 mb\?/i }));
    await waitFor(() => expect(ipc.deleteModel).toHaveBeenCalledExactlyOnceWith(whisperQ8.id));
  });
});


describe("SettingsDialog: launch at login", () => {
  it("reads system state and enables and disables login launch through IPC", async () => {
    render(<SettingsDialog open onOpenChange={() => {}} />);
    const toggle = await screen.findByRole("checkbox", { name: "Launch at login" });
    await waitFor(() => expect(toggle).toBeEnabled());
    expect(toggle).not.toBeChecked();
    expect(ipc.setLaunchAtLogin).not.toHaveBeenCalled();
    fireEvent.click(toggle);
    await waitFor(() => expect(toggle).toBeChecked());
    expect(ipc.setLaunchAtLogin).toHaveBeenLastCalledWith(true);
    fireEvent.click(toggle);
    await waitFor(() => expect(toggle).not.toBeChecked());
    expect(ipc.setLaunchAtLogin).toHaveBeenLastCalledWith(false);
  });

  it("keeps the previous state and displays registration errors", async () => {
    vi.mocked(ipc.setLaunchAtLogin).mockRejectedValueOnce("Registration failed");
    render(<SettingsDialog open onOpenChange={() => {}} />);
    const toggle = await screen.findByRole("checkbox", { name: "Launch at login" });
    await waitFor(() => expect(toggle).toBeEnabled());
    fireEvent.click(toggle);
    await screen.findByText("Registration failed");
    expect(toggle).not.toBeChecked();
    expect(toggle).toBeEnabled();
  });

  it("shows pending system approval and lets the user unregister it", async () => {
    vi.mocked(ipc.setLaunchAtLogin).mockResolvedValueOnce("requires_approval");
    render(<SettingsDialog open onOpenChange={() => {}} />);
    const toggle = await screen.findByRole("checkbox", { name: "Launch at login" });
    await waitFor(() => expect(toggle).toBeEnabled());
    fireEvent.click(toggle);
    await screen.findByText("Allow Simstim in System Settings > General > Login Items.");
    expect(toggle).toBeChecked();
    fireEvent.click(toggle);
    await waitFor(() => expect(toggle).not.toBeChecked());
    expect(ipc.setLaunchAtLogin).toHaveBeenLastCalledWith(false);
  });

  it("refreshes changes from macOS when the window regains focus", async () => {
    vi.mocked(ipc.getLaunchAtLogin).mockResolvedValue("enabled");
    render(<SettingsDialog open onOpenChange={() => {}} />);
    const toggle = await screen.findByRole("checkbox", { name: "Launch at login" });
    await waitFor(() => expect(toggle).toBeChecked());
    vi.mocked(ipc.getLaunchAtLogin).mockResolvedValue("disabled");
    fireEvent.focus(window);
    await waitFor(() => expect(toggle).not.toBeChecked());
  });

  it("disables the toggle while saving", async () => {
    let finish!: (status: ipc.LoginStatus) => void;
    vi.mocked(ipc.setLaunchAtLogin).mockReturnValueOnce(new Promise((resolve) => { finish = resolve; }));
    render(<SettingsDialog open onOpenChange={() => {}} />);
    const toggle = await screen.findByRole("checkbox", { name: "Launch at login" });
    await waitFor(() => expect(toggle).toBeEnabled());
    fireEvent.click(toggle);
    expect(toggle).toBeDisabled();
    fireEvent.click(toggle);
    expect(ipc.setLaunchAtLogin).toHaveBeenCalledTimes(1);
    finish("enabled");
    await waitFor(() => expect(toggle).toBeEnabled());
    expect(toggle).toBeChecked();
  });
});
