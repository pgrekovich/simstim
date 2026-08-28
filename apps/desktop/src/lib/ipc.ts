/**
 * Single source of truth for the Tauri IPC surface: TS types mirroring the
 * Rust shapes exactly (see docs/plans/2026-08-14-stage-0.5.md "Global
 * Constraints") plus typed invoke wrappers. Both the main screen (Task 4)
 * and the settings dialog (Task 5) import from here instead of calling
 * `invoke` directly, so the contract only needs to be kept in sync in one
 * place.
 */
import { invoke } from "@tauri-apps/api/core";

/** Mirrors `config::Config` (apps/desktop/src-tauri/src/config.rs). */
export interface AppConfig {
  output_dir: string;
  active_model: string;
  /** `"system" | "light" | "dark"`. */
  theme: string;
  /** Whether simstim appears in the macOS Dock. Defaults to `true`. */
  show_in_dock: boolean;
  /**
   * Whether recordings keep a stereo audio file (wav during capture,
   * encoded to `audio_format` after stop) next to the transcript. Defaults
   * to `true`. Toggled from the Settings dialog's "Keep call audio"
   * checkbox (Task 5).
   */
  keep_audio: boolean;
  /**
   * `"aac" | "flac"` (stage-0.7 format change): which format new recordings
   * are encoded to. Defaults to `"aac"` (owner decision after a measured
   * codec benchmark: ~17 MB/hour at 3.8% WER vs lossless, against FLAC's
   * ~137 MB/hour at 0% WER). Toggled from the Settings dialog's audio
   * format control, next to "Keep call audio".
   */
  audio_format: string;
  /**
   * Registry id (stage 0.8) of whichever whisper model
   * `retranscribe_session` uses for its offline re-transcribe pass. Defaults
   * to `"whisper-large-v3-turbo-q8"` (the model the owner already
   * downloaded before model selection existed). Set from the Settings
   * dialog's model manager via `setRetranscribeModel`.
   */
  retranscribe_model: string;
  /**
   * Whether `stop_recording` automatically queues a re-transcribe job for
   * the session that just stopped (stage 0.9). Only actually fires when
   * `keep_audio` is also on and the session has audio. Defaults to `false`.
   * Toggled from the Settings dialog's Models section (Re-transcribe group)
   * via `setAutoRetranscribe`.
   */
  auto_retranscribe: boolean;
  /**
   * Whether a SUCCESSFUL re-transcribe -- manual or automatic, this doesn't
   * depend on how it was triggered -- deletes the session's audio file
   * afterward (stage 0.9). Irreversible: once the audio is gone, the call
   * can never be re-transcribed again. Defaults to `false`. Toggled from
   * the Settings dialog's Models section (Re-transcribe group) via
   * `setDeleteAudioAfterRetranscribe`.
   */
  delete_audio_after_retranscribe: boolean;
  /**
   * Which summary provider `enhance_session` runs: built-in `"local"`, or
   * the opt-in cloud CLIs `"claude" | "codex"`.
   */
  enhance_provider: string;
  /**
   * Per-provider model override, keyed by provider id (e.g.
   * `{"claude": "fable"}`). Missing/empty = the CLI's own default.
   */
  enhance_models: Record<string, string>;
  /**
   * Whether a finished recording auto-enhances (stage 0.11): fires after
   * the call stops (after the auto re-transcribe when that's on) and after
   * any successful re-transcribe. Defaults to `false`.
   */
  auto_enhance: boolean;
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
   * The session's audio file (its `.flac`, or a `.wav` fallback if the
   * flac-encode step failed), resolved by the same helper the re-transcribe
   * command uses (`retranscribe::resolve_audio_path`). `null` when no audio
   * was saved for this session (audio retention was off, or the session
   * predates stage-0.7). Fed to `convertFileSrc` for the `<audio>` player
   * (Task 4).
   */
  audio_path: string | null;
  /** The `## Summary` section body (stage 0.11 enhance), `""` without one. */
  summary: string;
  /** What generated `summary` ("claude" / "codex"), `null` without one. */
  summary_model: string | null;
}

/** Mirrors `enhance::EnhanceProviderInfo`: one row of the Settings AI section. */
export interface EnhanceProviderInfo {
  id: string;
  name: string;
  available: boolean;
  version: string | null;
}

/** Mirrors `models::ModelInfo` (apps/desktop/src-tauri/src/models.rs). */
export interface ModelInfo {
  id: string;
  name: string;
  /**
   * Short descriptive subtitle shown under `name` (stage 0.8.x curation),
   * `null` when there's nothing worth saying beyond the name itself
   * (Parakeet, and every retired entry). Mirrors `ModelSpec::tagline`.
   */
  tagline: string | null;
  size_mb: number;
  downloaded: boolean;
  active: boolean;
  /**
   * `"live" | "retranscribe" | "summary"`: the Settings dialog's model
   * manager groups rows by this field (Recording vs Re-transcribe) rather
   * than testing a bare bool. Replaces the stage-0.7 `selectable` field
   * outright now that the model manager UI (this field's only remaining
   * consumer) reads `role` directly.
   */
  role: string;
  /**
   * Real bytes on disk for this model's downloaded files, `null` when not
   * downloaded (stage 0.8): actual usage rather than the advertised
   * `size_mb`, which can differ from what a real extracted archive occupies.
   */
  size_on_disk_bytes: number | null;
  /**
   * Whether this model has been curated out of the offered lineup (stage
   * 0.8.x): still visible and deletable if it's on disk, but never
   * downloadable or selectable again. Mirrors `ModelSpec::retired`. A
   * retired model only ever appears in `list_models`'s result while it's
   * downloaded (see `models::list_models_at`'s docs), so the Settings
   * dialog's retired row always renders size + delete only.
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

export type DownloadPhase = "download" | "extract" | "done" | "error";

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

export function getConfig(): Promise<AppConfig> {
  return invoke("get_config");
}

export function setOutputDir(path: string): Promise<void> {
  return invoke("set_output_dir", { path });
}

export function setActiveModel(id: string): Promise<void> {
  return invoke("set_active_model", { id });
}

/**
 * Sets and persists which registered whisper model `retranscribe_session`
 * uses (stage 0.8). Mirrors `set_retranscribe_model`
 * (apps/desktop/src-tauri/src/lib.rs). Rejects an unknown id or one that
 * isn't a re-transcribe-role model (e.g. the live Parakeet id).
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
 * Deletes a downloaded model's files to reclaim disk (stage 0.8). Mirrors
 * `models::delete_model` (apps/desktop/src-tauri/src/models.rs). Rejects
 * (with a plain, user-facing message) if a download of `id` is in flight, a
 * re-transcribe job is currently running, or `id` is the live model
 * (deleting it would break recording). Deleting the currently selected
 * re-transcribe model is allowed.
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
 * "Keep call audio" checkbox (Task 5).
 */
export function setKeepAudio(keepAudio: boolean): Promise<void> {
  return invoke("set_keep_audio", { keepAudio });
}

/**
 * Sets and persists which format new recordings are encoded to (stage-0.7
 * format change). Mirrors `config::set_audio_format`
 * (apps/desktop/src-tauri/src/config.rs). Only affects future encodes: an
 * already-recorded session's audio file keeps whatever format it was
 * written in.
 */
export function setAudioFormat(format: string): Promise<void> {
  return invoke("set_audio_format", { format });
}

/**
 * Sets and persists whether `stop_recording` automatically queues a
 * re-transcribe job for the session that just stopped (stage 0.9). Mirrors
 * `config::set_auto_retranscribe` (apps/desktop/src-tauri/src/config.rs).
 * Only takes effect when `keep_audio` is also on -- the Settings dialog
 * (Task 2) disables this toggle when it's off.
 */
export function setAutoRetranscribe(autoRetranscribe: boolean): Promise<void> {
  return invoke("set_auto_retranscribe", { autoRetranscribe });
}

/**
 * Sets and persists whether a successful re-transcribe (manual or
 * automatic) deletes the session's audio file afterward (stage 0.9). Mirrors
 * `config::set_delete_audio_after_retranscribe`
 * (apps/desktop/src-tauri/src/config.rs). Irreversible: once the audio is
 * gone, the call can never be re-transcribed again -- the Settings dialog
 * (Task 2) states this plainly in the toggle's helper text.
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
 * Enhances a note with the configured AI CLI (stage 0.11): the user's rough
 * notes lead, the transcript backs them, and the result replaces the note's
 * `## Summary` section (never the notes). Mirrors `enhance::enhance_session`.
 * Rejects the recording/finalizing session, an empty transcript, a missing
 * provider CLI, and a second enhance while one is running. Resolves with the
 * generated summary text once it's on disk.
 */
export function enhanceSession(path: string): Promise<string> {
  return invoke("enhance_session", { path });
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
 * one-at-a-time slot; exactly one terminal `done`/`error` follows, including
 * for failures that never reach the slot (validation, a missing provider).
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
 * Sets one provider's model override (stage 0.11); an empty string clears
 * it back to the CLI's default. Mirrors `config::set_enhance_model`.
 */
export function setEnhanceModel(provider: string, model: string): Promise<void> {
  return invoke("set_enhance_model", { provider, model });
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
 * Full-text search over the output folder (stage 0.10): case-insensitive
 * substring match against each session's title, notes and transcript.
 * Mirrors `sessions::search_sessions` (apps/desktop/src-tauri/src/sessions.rs).
 * A blank query returns the same list as `listSessions`, with empty snippets.
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
 * Moves a session's `.md` and its audio file(s) to the macOS Trash (stage
 * 0.10) -- recoverable from the Finder, not a hard delete. Mirrors
 * `sessions::delete_session` (apps/desktop/src-tauri/src/sessions.rs).
 * Rejects (with a plain, user-facing message) the currently-recording
 * session, one still being finalized, and one with an in-flight or queued
 * re-transcribe job.
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
 * Starts an offline re-transcription of `path` using whichever whisper
 * model `AppConfig.retranscribe_model` currently names (stage 0.8: picked
 * from the Settings dialog's model manager), replacing its `## Transcript`
 * section outright once done (title/notes/date/duration preserved, `asr:`
 * updated); if `AppConfig.delete_audio_after_retranscribe` is on, a
 * SUCCESSFUL pass also deletes the session's audio file afterward (stage
 * 0.9). Mirrors `retranscribe::retranscribe_session`
 * (apps/desktop/src-tauri/src/retranscribe.rs). Rejects immediately if the
 * session is recording, currently being finalized, its audio is missing, or
 * the selected model isn't downloaded. A re-transcribe already running for
 * a DIFFERENT session no longer causes a rejection (stage 0.9): this one
 * queues behind it (FIFO) and the returned promise still resolves once THIS
 * session's own job finishes, not before; progress in between (for
 * whichever session is actually running right now) arrives via
 * `retranscribe-progress` events.
 */
export function retranscribeSession(path: string): Promise<void> {
  return invoke("retranscribe_session", { path });
}
