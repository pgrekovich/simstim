import { clsx, type ClassValue } from "clsx"
import { twMerge } from "tailwind-merge"

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
}

/**
 * "31:14", or "1:02:44" once there's an hour. `null` in, `null` out: a
 * session without audio has no duration to show. Shared by the sidebar rows
 * and the note's title meta line so the two can never drift apart.
 */
export function formatDuration(durationSecs: number | null): string | null {
  if (durationSecs === null) return null;
  const whole = Math.max(0, Math.floor(durationSecs));
  const hours = Math.floor(whole / 3600);
  const minutes = Math.floor((whole % 3600) / 60);
  const seconds = whole % 60;
  return hours > 0
    ? `${hours}:${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}`
    : `${minutes}:${String(seconds).padStart(2, "0")}`;
}
