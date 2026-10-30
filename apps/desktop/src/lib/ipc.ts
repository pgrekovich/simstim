/**
 * Typed Tauri IPC wrappers. Keep TypeScript types aligned with Rust
 * responses and route UI calls through this module.
 *
 */
import { invoke } from "@tauri-apps/api/core";

/** Mirrors `config::Config` (apps/desktop/src-tauri/src/config.rs). */
export interface AppConfig {
  output_dir: string;
  active_model: string;
  /** Whether the guided permission and recording-model setup has completed. */
  onboarding_complete: boolean;
  /** `"system" | "light" | "dark"`. */
  theme: string;
  /** Whether simstim appears in the macOS Dock. Defaults to `true`. */
  show_in_dock: boolean;
  /**
 * Retain stereo audio next to the note: WAV during capture, then the
 * configured format. Enabled by default.
 *
 */
  keep_audio: boolean;
  /**
 * Audio encoding format: aac by default, or lossless flac. Advanced
 * file-only setting.
 *
 */
  audio_format: string;
  /**
 * Registry ID of the selected offline Whisper model. Defaults to
 * whisper-large-v3-turbo-q8.
 *
 */
  retranscribe_model: string;
  /**
 * Whether `stop_recording` automatically queues a re-transcribe job for the
 * session that just stopped. Only actually fires when `keep_audio` is also
 * on and the session has audio. Defaults to `true`. Advanced override in
 * config.toml.
 *
 */
  auto_retranscribe: boolean;
  /**
 * Opt-in audio deletion after successful transcription. Disabled by default
 * and configured in config.toml. Without the audio, another pass is
 * impossible.
 *
 */
  delete_audio_after_retranscribe: boolean;
  /**
   * Which AI CLI `enhance_session` runs: `"claude" | "codex"` (the user's
   * own subscription, text only). The on-device provider was retired.
   */
  enhance_provider: string;
  /**
   * Per-provider model override, keyed by provider id (e.g.
   * `{"claude": "fable"}`). Missing/empty = the CLI's own default.
   */
  enhance_models: Record<string, string>;
  /**
 * Whether a finished recording auto-enhances: fires after the call stops
 * (after the auto re-transcribe when that's on) and after any successful
 * re-transcribe. Defaults to `true`; the onboarding screen shows it as one
 * visible switch.
 *
 */
  auto_enhance: boolean;
  /**
   * Whether an enhance may replace an AUTOMATIC title ("Untitled", or the
   * detected "Zoom call 14:03" shape) with one proposed by the summary
   * model. A title the user typed themselves is never touched, regardless
   * of this setting. Defaults to `true`.
   */
  enhance_rename_title: boolean;
  /**
   * Whether an enhance may look up the calendar event that overlapped the
   * call through the user's own connectors. The lookup itself always runs
   * through the Claude Code CLI (codex's non-interactive sandbox cannot run
   * MCP tools); the found event feeds either provider as text. Defaults to
   * `true`.
   */
  enhance_use_calendar: boolean;
  /** Whether known mic-using call apps may surface a recording suggestion. */
  detect_calls: boolean;
  /** Whether an associated recording stops after app I/O release and remote-channel silence. */
  auto_stop_after_call: boolean;
}

/**
 * Payload of the `banner-start` event the banner window emits toward the
 * main window when its Start recording button is clicked.
 */
export interface CallDetectedPayload {
  bundle_id: string;
  app_name: string;
}

/**
 * What the floating banner window is showing. Pushed by the backend as the
 * `banner-state` event and returned by `banner_current_state` (the mount-time
 * seed, so a reloaded banner webview cannot miss a push). Mirrors
 * `banner::BannerPayload` (apps/desktop/src-tauri/src/banner.rs).
 */
export type BannerStatePayload =
  | { kind: "hidden" }
  | { kind: "detected"; bundle_id: string; app_name: string }
  | { kind: "ended" };

export function bannerCurrentState(): Promise<BannerStatePayload> {
  return invoke("banner_current_state");
}

/** Hides the banner (the X button and the auto-dismiss timeout). */
export function bannerDismiss(): Promise<void> {
  return invoke("banner_dismiss");
}

/** The banner's "Open simstim": shows and focuses the main window. */
export function showMainWindow(): Promise<void> {
  return invoke("show_main_window");
}

/**
 * Mirrors `HearingStatus` (lib.rs): milliseconds since speech was last
 * heard on each channel, since the recording started if never.
 */
export interface HearingStatus {
  me_silent_ms: number;
  them_silent_ms: number;
}

/**
 * The live "hearing you / hearing them" readout, polled by the toolbar while
 * a recording runs; `null` when nothing is recording.
 */
export function recordingActivity(): Promise<HearingStatus | null> {
  return invoke("recording_activity");
}

export interface CallEndedPayload {
  bundle_id: string;
  recording_id: string;
}

/**
 * Mirrors `sessions::SessionMeta` (apps/desktop/src-tauri/src/sessions.rs):
 * the sidebar's session list, one entry per output-folder `.md` file that
 * parses as a transcript. `list_sessions` scans the folder fresh on every
 * call -- the files are the only store, there is no separate index.
 */
export interface SessionMeta {
  path: string;
  title: string;
  /** RFC3339. */
  date: string;
  /** Rust `Option<u64>` serializes as an explicit `null`, not a missing key. */
  duration_secs: number | null;
}

/**
 * Mirrors `sessions::SessionDataOut` (apps/desktop/src-tauri/src/sessions.rs):
 * a full session read for the session view.
 */
export interface SessionData {
  title: string;
  /** RFC3339. */
  date: string;
  notes: string;
  /** Raw `## Transcript` section text: one `[hh:mm:ss] Who: text` line per entry. */
  transcript: string;
  /**
 * AAC, FLAC or recoverable WAV path resolved by the backend, or null when
 * audio is absent. Passed through convertFileSrc for playback.
 *
 */
  audio_path: string | null;
  /**
 * The `## Summary` section body, `""` without one.
 *
 */
  summary: string;
  /** What generated `summary` ("claude" / "codex"), `null` without one. */
  summary_model: string | null;
  /**
   * The summary style (a `SummaryStyle.id` from prompts.toml) that wrote
   * `summary`; `null` for the built-in prompt or without a summary.
   */
  summary_style: string | null;
  /**
   * Who was on the call: the `people:` frontmatter list, written from the
   * calendar event the enhance lookup matched. `[]` without one.
   */
  people: string[];
}

/** Mirrors `enhance::EnhanceProviderInfo`: one row of the Settings AI section. */
export interface EnhanceProviderInfo {
  /** Installed is distinct from authenticated/available. */
  installed?: boolean;
  id: string;
  name: string;
  available: boolean;
  version: string | null;
  /** Official helper model catalog, null when discovery is unavailable. */
  models?: { value: string; label: string }[] | null;
}

/** Mirrors `models::ModelInfo` (apps/desktop/src-tauri/src/models.rs). */
export interface ModelInfo {
  id: string;
  name: string;
  /** Exact upstream name, rendered as quiet expert-facing metadata. */
  technical_name: string | null;
  /**
 * Short descriptive subtitle shown under `name` , `null` when there's
 * nothing worth saying beyond the name itself. Mirrors `ModelSpec::tagline`.
 *
 */
  tagline: string | null;
  size_mb: number;
  downloaded: boolean;
  active: boolean;
  /**
 * Model role used to group settings rows: live, retranscribe or legacy
 * summary.
 *
 */
  role: string;
  /**
 * Real bytes on disk for this model's downloaded files, `null` when not
 * downloaded: actual usage rather than the advertised `size_mb`, which can
 * differ from what the real downloaded files occupy.
 *
 */
  size_on_disk_bytes: number | null;
  /**
 * Retired model entries remain visible only to remove downloaded files. They
 * cannot be selected or downloaded again.
 */
  retired: boolean;
}

/** Mirrors `SessionErrorPayload` (apps/desktop/src-tauri/src/lib.rs), the `session-error` event payload. */
export interface SessionErrorPayload {
  message: string;
}

/**
 * Mirrors `TranscriptInterimPayload` (apps/desktop/src-tauri/src/lib.rs), the
 * `transcript-interim` event payload: a live (not-yet-finalized) draft for
 * one channel. An empty `text` means "clear this channel's draft" (the
 * segment finalized or speech stopped). Never written to disk.
 */
export interface TranscriptInterimPayload {
  channel: "me" | "them";
  text: string;
}

export type DownloadPhase = "download" | "done" | "error";

export type RetranscribePhase = "loading-model" | "transcribing" | "writing" | "done" | "error";

/**
 * Mirrors `retranscribe::RetranscribeProgressPayload`
 * (apps/desktop/src-tauri/src/retranscribe.rs), the `retranscribe-progress`
 * event payload. Unlike `DownloadProgress.message`, `channel` and `message`
 * are always present (as explicit `null`, not a missing key) since the Rust
 * side never uses `skip_serializing_if` for this payload.
 */
export interface RetranscribeProgress {
  path: string;
  phase: RetranscribePhase;
  channel: "me" | "them" | null;
  message: string | null;
}

/**
 * Mirrors `models::ProgressPayload`, the `model-download-progress` event
 * payload. `message` is only present when `phase === "error"`.
 */
export interface DownloadProgress {
  id: string;
  phase: DownloadPhase;
  received: number;
  total: number;
  message?: string;
}

export interface PermissionCheck {
  granted: boolean;
  error: string | null;
}

export interface PermissionCheckResult {
  microphone: PermissionCheck;
  system_audio: PermissionCheck;
}

export function getConfig(): Promise<AppConfig> {
  return invoke("get_config");
}

/** Requests and verifies both macOS audio permissions without saving audio. */
export function checkAudioPermissions(): Promise<PermissionCheckResult> {
  return invoke("check_audio_permissions");
}

/** Persists first-run completion after backend verification of permissions and model. */
export function completeOnboarding(): Promise<void> {
  return invoke("complete_onboarding");
}

export function setOutputDir(path: string): Promise<void> {
  return invoke("set_output_dir", { path });
}

export function setActiveModel(id: string): Promise<void> {
  return invoke("set_active_model", { id });
}

/**
 * Sets and persists which registered whisper model `retranscribe_session`
 * uses. Mirrors `set_retranscribe_model`
 * (apps/desktop/src-tauri/src/lib.rs). Rejects an unknown id or one that
 * isn't a re-transcribe-role model (e.g. the live Parakeet id).
 *
 */
export function setRetranscribeModel(id: string): Promise<void> {
  return invoke("set_retranscribe_model", { id });
}

export function listModels(): Promise<ModelInfo[]> {
  return invoke("list_models");
}

export function downloadModel(id: string): Promise<void> {
  return invoke("download_model", { id });
}

/**
 * Delete downloaded model files. Reject deletion of the live model or files
 * needed by a running download or queued/running transcription job.
 *
 */
export function deleteModel(id: string): Promise<void> {
  return invoke("delete_model", { id });
}

/**
 * Starts recording. `path: null` starts a brand-new session (the previous
 * behavior). `path: <a session's path>` instead continues that existing
 * session: the worker loads its title/notes/transcript/date/duration and
 * appends to it rather than overwriting, with new timestamps continuing from
 * where the file left off. Mirrors `start_recording`
 * (apps/desktop/src-tauri/src/lib.rs). Resolves with the path recorded into
 * either way.
 */
export function startRecording(
  path: string | null,
  title?: string,
  detectedBundleId?: string,
): Promise<string> {
  return invoke("start_recording", { path, title, detectedBundleId });
}

export function stopRecording(): Promise<string> {
  return invoke("stop_recording");
}

export function setTheme(theme: string): Promise<void> {
  return invoke("set_theme", { theme });
}

export function setShowInDock(showInDock: boolean): Promise<void> {
  return invoke("set_show_in_dock", { showInDock });
}

/**
 * Sets and persists whether recordings keep an audio file alongside the
 * transcript. Mirrors `config::set_keep_audio`
 * (apps/desktop/src-tauri/src/config.rs). Called from the Settings dialog's
 * "Keep call audio" checkbox.
 *
 */
export function setKeepAudio(keepAudio: boolean): Promise<void> {
  return invoke("set_keep_audio", { keepAudio });
}

/**
 * Sets and persists which format new recordings are encoded to. Mirrors
 * `config::set_audio_format` (apps/desktop/src-tauri/src/config.rs). Only
 * affects future encodes: an already-recorded session's audio file keeps
 * whatever format it was written in.
 *
 */
export function setAudioFormat(format: string): Promise<void> {
  return invoke("set_audio_format", { format });
}

/**
 * Persist automatic re-transcription. It requires retained audio and a
 * downloaded model; advanced override in config.toml.
 *
 */
export function setAutoRetranscribe(autoRetranscribe: boolean): Promise<void> {
  return invoke("set_auto_retranscribe", { autoRetranscribe });
}

/**
 * Persist opt-in deletion of audio after successful re-transcription. This
 * is irreversible; advanced setting in config.toml.
 *
 */
export function setDeleteAudioAfterRetranscribe(deleteAudioAfterRetranscribe: boolean): Promise<void> {
  return invoke("set_delete_audio_after_retranscribe", { deleteAudioAfterRetranscribe });
}

export function setDetectCalls(enabled: boolean): Promise<void> {
  return invoke("set_detect_calls", { enabled });
}

export function setAutoStopAfterCall(autoStopAfterCall: boolean): Promise<void> {
  return invoke("set_auto_stop_after_call", { autoStopAfterCall });
}

export function listSessions(): Promise<SessionMeta[]> {
  return invoke("list_sessions");
}

/**
 * Mirrors `sessions::StorageStats`: what the notes folder weighs, split into
 * notes vs audio, for the Settings Storage page. `audio_secs` sums only
 * sessions that actually have an audio file.
 *
 */
export interface StorageStats {
  note_count: number;
  notes_bytes: number;
  audio_bytes: number;
  audio_secs: number;
}

export function storageStats(): Promise<StorageStats> {
  return invoke("storage_stats");
}

/**
 * Enhances a note with the configured AI CLI: the user's rough notes lead,
 * the transcript backs them, and the result replaces the note's `## Summary`
 * section (never the notes). Mirrors `enhance::enhance_session`. Rejects the
 * recording/finalizing session, an empty transcript, a missing provider CLI,
 * and a second enhance while one is running. Resolves with the generated
 * summary text once it's on disk.
 *
 */
/**
 * `style` is a `SummaryStyle.id`, `"standard"` for the built-in prompt, or
 * omitted to repeat whatever style the note remembers (what "Again" does).
 */
export function enhanceSession(path: string, style?: string): Promise<string> {
  return invoke("enhance_session", { path, style: style ?? null });
}

/** Which local and cloud summary providers are ready. Mirrors `enhance::list_enhance_providers`. */
export function listEnhanceProviders(): Promise<EnhanceProviderInfo[]> {
  return invoke("list_enhance_providers");
}

/** Sets and persists which summary provider `enhanceSession` runs. */
export function setEnhanceProvider(provider: string): Promise<void> {
  return invoke("set_enhance_provider", { provider });
}

export type EnhancePhase = "running" | "done" | "error";

/**
 * Mirrors `enhance::EnhanceProgressPayload`, the `enhance-progress` event
 * payload: one enhance job's lifecycle, emitted for BOTH manual and
 * automatic runs. `running` fires once the job actually claims the
 * one-at-a-time slot, then again per local map/reduce step with a progress
 * `message` ("Summarizing part 2 of 7..."); exactly one terminal
 * `done`/`error` follows, including for failures that never reach the slot
 * (validation, a missing provider).
 */
export interface EnhanceProgress {
  path: string;
  phase: EnhancePhase;
  /** The provider id that ran (`"local"`, `"claude"`, `"codex"`). */
  provider: string;
  message: string | null;
}

/** Sets and persists whether a finished recording auto-enhances. Mirrors `config::set_auto_enhance`. */
export function setAutoEnhance(autoEnhance: boolean): Promise<void> {
  return invoke("set_auto_enhance", { autoEnhance });
}

/**
 * Sets and persists whether an enhance may retitle automatically-named
 * notes from the summary. Mirrors `config::set_enhance_rename_title`.
 */
export function setEnhanceRenameTitle(enhanceRenameTitle: boolean): Promise<void> {
  return invoke("set_enhance_rename_title", { enhanceRenameTitle });
}

/**
 * Sets and persists whether the Claude Code provider may consult the
 * user's calendar connectors for meeting context. Mirrors
 * `config::set_enhance_use_calendar`.
 */
export function setEnhanceUseCalendar(enhanceUseCalendar: boolean): Promise<void> {
  return invoke("set_enhance_use_calendar", { enhanceUseCalendar });
}


/**
 * Sets one provider's model override; an empty string clears it back to the
 * CLI's default. Mirrors `config::set_enhance_model`.
 *
 */
export function setEnhanceModel(provider: string, model: string): Promise<void> {
  return invoke("set_enhance_model", { provider, model });
}

/**
 * Mirrors `chat::ChatSource` (apps/desktop/src-tauri/src/chat.rs): one
 * evidence chip on an assistant message. `file` is the cited note's file
 * name relative to the notes folder (`null` when the model cited the
 * current call itself); `timestamp` is the supporting transcript second
 * (`null` when the evidence wasn't a transcript line).
 */
export interface ChatSource {
  file: string | null;
  timestamp: string | null;
}

/** Mirrors `chat::ChatMessage`: one message of a conversation. */
export interface ChatMessage {
  role: "user" | "assistant";
  text: string;
  sources: ChatSource[];
  followups: string[];
  /** Provider/model label ("claude", "claude/opus"); `null` on user messages. */
  label: string | null;
  /**
   * Display-only "asked at" string the UI passed with the question (the
   * recording clock for a mid-call ask); `null` otherwise.
   */
  asked_at: string | null;
  /**
   * A quick question's name (prompts.toml), shown instead of the prompt
   * text the message actually sent; `null` on typed questions and answers.
   */
  question_name: string | null;
}

/** Mirrors `chat::ChatHistory`: a scope's messages plus its in-flight flag. */
export interface ChatHistory {
  messages: ChatMessage[];
  running: boolean;
}

/**
 * Mirrors `chat::ChatProgressPayload`, the `chat-progress` event payload:
 * one ask's lifecycle for its scope (`"all"`, or the note's path). `running`
 * fires once the ask claims the scope; exactly one terminal `done`/`error`
 * follows. A cancelled ask terminates as `error` with the message
 * "cancelled" -- the UI's cue for a quiet dismissal, not an error banner.
 */
export interface ChatProgress {
  scope: string;
  /**
   * `running` once the ask claims the scope; any number of `step` events
   * follow (one per tool call, `message` says what is being read, e.g.
   * "Reading 2026-09-03 All-hands.md"); exactly one `done`/`error` ends it.
   */
  phase: "running" | "step" | "done" | "error";
  message: string | null;
}

/**
 * Asks one question in a conversation. `path: null` asks the whole notes
 * folder; `path: <a session's path>` asks that note, including the one being
 * recorded right now (the backend snapshots its on-disk transcript per ask).
 * Mirrors `chat::chat_ask`. The promise resolves once the answer is stored
 * (or rejects with the failure); progress and the terminal phase also arrive
 * as `chat-progress` events, and the updated conversation is fetched back
 * with `chatHistory`.
 *
 */
export function chatAsk(
  path: string | null,
  question: string,
  askedAt?: string,
  questionName?: string,
  connectors?: boolean,
): Promise<void> {
  return invoke("chat_ask", {
    path,
    question,
    askedAt,
    questionName: questionName ?? null,
    connectors: connectors ?? false,
  });
}

/** Mirrors `prompts::SummaryStyle`: one way of writing a note's Summary section. */
export interface SummaryStyle {
  id: string;
  name: string;
  prompt: string;
}

/** Where a quick question is offered: the open call, the notes folder, or both. */
export type QuickScope = "call" | "notes" | "both";

/** Mirrors `prompts::QuickQuestion`: one chat shortcut. */
export interface QuickQuestion {
  id: string;
  name: string;
  prompt: string;
  scope: QuickScope;
  /**
   * Whether the ask runs with the user's connectors (the CLI's MCP servers:
   * Jira, calendar, ...) available, read-only by instruction. Off, the ask
   * has only the file tools and never connects a server, which keeps it fast.
   */
  connectors: boolean;
}

/** Mirrors `prompts::Prompts`: the whole of the user's `prompts.toml`. */
export interface Prompts {
  /** Standing instructions appended to every summary prompt. */
  summary_instructions: string;
  summary_styles: SummaryStyle[];
  quick_questions: QuickQuestion[];
}

/** The user's prompts, seeded with defaults on first read. Rejects with the parse error of a broken hand edit. */
export function getPrompts(): Promise<Prompts> {
  return invoke("get_prompts");
}

/** Replaces the whole prompts document; resolves to what was saved. */
export function savePrompts(prompts: Prompts): Promise<Prompts> {
  return invoke("save_prompts", { prompts });
}

/** Back to the seeded defaults, discarding every edit. */
export function restoreDefaultPrompts(): Promise<Prompts> {
  return invoke("restore_default_prompts");
}

/** The scope's conversation so far, for seeding a freshly mounted view. */
export function chatHistory(path: string | null): Promise<ChatHistory> {
  return invoke("chat_history", { path });
}

/** Forgets the scope's conversation. Rejected while an ask is in flight. */
export function chatClear(path: string | null): Promise<void> {
  return invoke("chat_clear", { path });
}

/** Cancels the scope's in-flight ask, if any (kills the CLI subprocess). */
export function chatCancel(path: string | null): Promise<void> {
  return invoke("chat_cancel", { path });
}

/** Mirrors `sessions::SearchSnippet`: one matching line inside a SearchHit. */
export interface SearchSnippet {
  /** Where the line came from, so the results pane can label it. */
  section: "title" | "notes" | "transcript";
  /** The whole matching line, verbatim; the UI windows and highlights it. */
  text: string;
}

/**
 * Mirrors `sessions::SearchHit` (SessionMeta is serde-flattened into it):
 * one matching session with its first few matching lines.
 */
export interface SearchHit extends SessionMeta {
  /** First few matching lines (backend caps them); shown highlighted. */
  snippets: SearchSnippet[];
  /** Total occurrences across the whole file, beyond the shown snippets. */
  total_matches: number;
}

/**
 * Full-text search over the output folder: case-insensitive substring match
 * against each session's title, notes and transcript. Mirrors
 * `sessions::search_sessions` (apps/desktop/src-tauri/src/sessions.rs). A
 * blank query returns the same list as `listSessions`, with empty snippets.
 *
 */
export function searchSessions(query: string): Promise<SearchHit[]> {
  return invoke("search_sessions", { query });
}

export function readSession(path: string): Promise<SessionData> {
  return invoke("read_session", { path });
}

/**
 * Creates a new, empty session note (title "Untitled", no notes, no
 * transcript) the user can type into without recording. Mirrors
 * `sessions::create_session` (apps/desktop/src-tauri/src/sessions.rs).
 * Resolves with the new file's path.
 */
export function createSession(): Promise<string> {
  return invoke("create_session");
}

/**
 * Move the note and its audio to macOS Trash. Reject deletion while this
 * session is recording, finalizing or being processed.
 */
export function deleteSession(path: string): Promise<void> {
  return invoke("delete_session", { path });
}

export function saveSession(path: string, title: string, notes: string): Promise<void> {
  return invoke("save_session", { path, title, notes });
}

/**
 * Routes a title/notes edit for the currently-active recording session into
 * its worker (single-writer rule: the worker, not the UI, owns that file
 * while recording). `undefined` fields mean "leave unchanged", matching the
 * Rust side's `Option<String>` parameters.
 */
export function updateActiveSession(title?: string, notes?: string): Promise<void> {
  return invoke("update_active_session", { title, notes });
}

/**
 * Queue an offline pass with the configured Whisper model. Preserve metadata
 * and notes, retain transcript text outside partial audio coverage, and
 * optionally delete audio only after success. Resolve when this job
 * finishes; progress events report intermediate state.
 *
 */
export function retranscribeSession(path: string): Promise<void> {
  return invoke("retranscribe_session", { path });
}

/** Vendor-owned setup; no credentials are passed over these commands. */
export interface AiSetupStatus { provider: string; phase: "installing" | "signing_in"; updating?: boolean; }
export function aiSetupStatus(): Promise<AiSetupStatus | null> { return invoke("ai_setup_status"); }
export function setupAi(provider: string, update = false): Promise<void> { return invoke("setup_ai", { provider, update }); }
export function cancelAiSetup(): Promise<void> { return invoke("cancel_ai_setup"); }

export interface ProviderUpdate {
  id: string;
  name: string;
  installed_version: string;
  latest_version: string;
}

/** Checks official release metadata for installed helpers, at most once per day in memory. */
export function checkAiUpdates(): Promise<ProviderUpdate[]> { return invoke("check_ai_updates"); }

export interface ProviderStatus {
  id: string;
  name: string;
  installed_version: string | null;
  latest_version: string | null;
  update_available: boolean;
}

/** Reads helper versions; explicit checks bypass the automatic daily cache. */
export function getAiProviderStatus(force = false): Promise<ProviderStatus[]> {
  return invoke("ai_provider_status", { force });
}

export type LoginStatus = "disabled" | "enabled" | "requires_approval" | "unavailable";

export function getLaunchAtLogin(): Promise<LoginStatus> {
  return invoke("get_launch_at_login");
}

export function setLaunchAtLogin(enabled: boolean): Promise<LoginStatus> {
  return invoke("set_launch_at_login", { enabled });
}
