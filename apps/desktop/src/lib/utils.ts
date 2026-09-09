import { clsx, type ClassValue } from "clsx"
import { twMerge } from "tailwind-merge"

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
}

/**
 * Format duration consistently for sidebar, note metadata and search
 * results. Return null when no duration is available.
 *
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
