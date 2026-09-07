import { clsx, type ClassValue } from "clsx"
import { twMerge } from "tailwind-merge"

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
}

/**
 * A call's length in words a person reads at a glance: "12 min", "1 h 55 min",
 * "< 1 min" (2026-09-07 ruling: "13:21 · 11:51" read as two clock times).
 * `null` in, `null` out: a session without audio has no duration to show.
 * Shared by the sidebar rows, the note's date line and the search palette so
 * the three can never drift apart.
 */
export function formatDuration(durationSecs: number | null): string | null {
  if (durationSecs === null) return null;
  const minutes = Math.round(Math.max(0, durationSecs) / 60);
  if (minutes < 1) return "< 1 min";
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  if (hours === 0) return `${minutes} min`;
  return rest === 0 ? `${hours} h` : `${hours} h ${rest} min`;
}
