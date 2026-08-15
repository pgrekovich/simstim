import { clsx, type ClassValue } from "clsx"
import { twMerge } from "tailwind-merge"

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
}

/**
 * Truncates the middle of a long string, keeping the start and end intact
 * (useful for file paths, where the interesting parts are the leading
 * directory and the trailing filename, not whatever's in between).
 */
export function truncateMiddle(value: string, maxLen = 60): string {
  if (value.length <= maxLen) return value;
  const keep = Math.floor((maxLen - 1) / 2);
  return `${value.slice(0, keep)}…${value.slice(value.length - keep)}`;
}
