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
  /**
   * Whether recordings keep a stereo audio file (wav during capture, flac
   * after stop) next to the transcript. Defaults to `true`. No settings UI
   * yet (Task 5 adds the toggle).
   */
  keep_audio: boolean;
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
}

/** Mirrors `models::ModelInfo` (apps/desktop/src-tauri/src/models.rs). */
export interface ModelInfo {
  id: string;
  name: string;
  size_mb: number;
  downloaded: boolean;
  active: boolean;
  /**
   * Whether this model can become the live ASR engine. `false` for whisper
   * (stage 0.7): it is a re-transcribe-only engine, so the Settings UI must
   * not offer a "Use" button for it, driven by this field rather than a
   * hardcoded model id.
   */
  selectable: boolean;
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

export function listModels(): Promise<ModelInfo[]> {
  return invoke("list_models");
}

export function downloadModel(id: string): Promise<void> {
  return invoke("download_model", { id });
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
export function startRecording(path: string | null): Promise<string> {
  return invoke("start_recording", { path });
}

export function stopRecording(): Promise<string> {
  return invoke("stop_recording");
}

export function setTheme(theme: string): Promise<void> {
  return invoke("set_theme", { theme });
}

/**
 * Sets and persists whether recordings keep an audio file alongside the
 * transcript. Mirrors `config::set_keep_audio`
 * (apps/desktop/src-tauri/src/config.rs). No settings UI calls this yet
 * (Task 5 adds the toggle).
 */
export function setKeepAudio(keepAudio: boolean): Promise<void> {
  return invoke("set_keep_audio", { keepAudio });
}

export function listSessions(): Promise<SessionMeta[]> {
  return invoke("list_sessions");
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
 * Starts an offline re-transcription of `path` using the whisper
 * large-v3-turbo model, replacing its `## Transcript` section outright once
 * done (title/notes/date/duration preserved, `asr:` updated). Mirrors
 * `retranscribe::retranscribe_session`
 * (apps/desktop/src-tauri/src/retranscribe.rs). Resolves once the job
 * finishes (or rejects immediately if the session is recording, its audio
 * is missing, the model isn't downloaded, or another job is already
 * running); progress in between arrives via `retranscribe-progress` events.
 */
export function retranscribeSession(path: string): Promise<void> {
  return invoke("retranscribe_session", { path });
}
