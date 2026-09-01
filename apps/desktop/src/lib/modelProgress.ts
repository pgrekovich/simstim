import type { DownloadProgress } from "@/lib/ipc";

export function downloadProgressPercent(progress: DownloadProgress | undefined): number {
  if (!progress || progress.phase !== "download" || progress.total <= 0) return 0;
  return Math.min(100, Math.max(0, (progress.received / progress.total) * 100));
}

/**
 * Download events can cross the IPC boundary close enough together to be
 * delivered out of order. Keep the visible state monotonic so the bar never
 * jumps backwards, and do not let a late byte event replace a terminal state.
 */
export function mergeDownloadProgress(
  current: DownloadProgress | undefined,
  incoming: DownloadProgress,
): DownloadProgress {
  if (incoming.phase !== "download") return incoming;
  if (!current) return incoming;
  if (current.phase !== "download") return current;

  const currentPercent = downloadProgressPercent(current);
  const incomingPercent = downloadProgressPercent(incoming);
  if (current.total > 0 && (incoming.total <= 0 || incomingPercent < currentPercent)) {
    return current;
  }
  return incoming;
}
