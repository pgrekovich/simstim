import type { RetranscribeProgress } from "@/lib/ipc";

/**
 * Which session (if any) currently has a re-transcribe job running, and its
 * latest phase text. Lifted up to App (Task 4 security-review fix) instead
 * of living only in SessionView's own local state: SessionView is
 * remounted by `key={path}` on every session switch (see its class doc
 * comment), which used to reset a local "is a job running" boolean back to
 * `false` on remount even though the backend job (global, one at a time --
 * see `retranscribe.rs`'s module docs) was still genuinely in progress,
 * silently re-enabling its "Re-transcribe" button on switching away and
 * back mid-job. State that must survive a child's remount has to live in a
 * parent that doesn't remount, so this lives in App and is passed down as a
 * prop; `path` doubles as "a job is running" (`null` means none).
 */
export type RetranscribeJob = { path: string; text: string } | null;

/**
 * Renders a `retranscribe-progress` event's phase into the short status
 * text shown while a re-transcribe job is running (Task 4). Shared between
 * `App` (which owns `RetranscribeJob` above) and `SessionView` (which reads
 * that state back out for its own `path`).
 */
export function describeRetranscribeProgress(payload: RetranscribeProgress): string {
  switch (payload.phase) {
    case "loading-model":
      return "loading model...";
    case "transcribing": {
      const who = payload.channel ? ` (${payload.channel})` : "";
      const detail = payload.message ? ` ${payload.message}` : "";
      return `transcribing${who}${detail}...`;
    }
    case "writing":
      return "writing...";
    case "done":
      return "done";
    case "error":
      return payload.message ?? "re-transcribe failed";
  }
}
