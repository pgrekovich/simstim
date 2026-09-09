import type { RetranscribeProgress } from "@/lib/ipc";

/**
 * Track the current re-transcription job in App so it survives session-view
 * remounts.
 *
 */
export type RetranscribeJob = { path: string; text: string } | null;

/**
 * Renders a `retranscribe-progress` event's phase into the short status text
 * shown while a re-transcribe job is running. Shared between `App` (which
 * owns `RetranscribeJob` above) and `SessionView` (which reads that state
 * back out for its own `path`).
 *
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

/**
 * Recognize audio-cleanup failure after successful transcription. Refresh
 * the transcript even though the terminal event is an error.
 *
 */
export function isTranscriptUpdatedButAudioDeleteFailed(message: string | null | undefined): boolean {
  return message != null && message.includes("transcript updated, but the audio could not be deleted");
}
