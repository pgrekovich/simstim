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
}

/** Mirrors `models::ModelInfo` (apps/desktop/src-tauri/src/models.rs). */
export interface ModelInfo {
  id: string;
  name: string;
  size_mb: number;
  downloaded: boolean;
  active: boolean;
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

export function startRecording(): Promise<string> {
  return invoke("start_recording");
}

export function stopRecording(): Promise<string> {
  return invoke("stop_recording");
}

export function setTheme(theme: string): Promise<void> {
  return invoke("set_theme", { theme });
}

export function listSessions(): Promise<SessionMeta[]> {
  return invoke("list_sessions");
}

export function readSession(path: string): Promise<SessionData> {
  return invoke("read_session", { path });
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
