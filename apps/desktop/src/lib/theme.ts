/**
 * Applies and watches the UI theme. Mirrors the `theme` field of
 * `AppConfig` (see src/lib/ipc.ts): `"system" | "light" | "dark"`.
 *
 * The `dark` class on `<html>` is the single toggle every shadcn `dark:`
 * utility keys off (`@custom-variant dark (&:is(.dark *));` in index.css).
 * index.html applies a pre-paint guess from `matchMedia` before React mounts
 * or `get_config` resolves; `initTheme` below reconciles that guess against
 * the persisted config once it loads.
 */

const DARK_QUERY = "(prefers-color-scheme: dark)";

function setDarkClass(dark: boolean) {
  document.documentElement.classList.toggle("dark", dark);
}

// Module-level, not per-caller: both App's initial `initTheme` call and a
// later `applyTheme` call from the settings dialog must share one "current
// system-mode listener" slot, or switching away from "system" in settings
// would leave App's original matchMedia listener running and fighting the
// user's explicit choice.
let unsubscribeCurrent: (() => void) | null = null;

/**
 * Applies `theme` to the document, replacing whatever theme (and any live
 * `matchMedia` subscription) was previously applied. For `"system"`,
 * subscribes to OS theme-change events so the app follows a live toggle
 * (e.g. macOS switching at sunset) without a restart.
 */
export function applyTheme(theme: string): void {
  unsubscribeCurrent?.();
  unsubscribeCurrent = null;

  if (theme === "dark") {
    setDarkClass(true);
    return;
  }

  if (theme === "light") {
    setDarkClass(false);
    return;
  }

  // "system" (and any unrecognized value): follow the OS live.
  const media = window.matchMedia(DARK_QUERY);
  setDarkClass(media.matches);
  const handleChange = (event: MediaQueryListEvent) => setDarkClass(event.matches);
  media.addEventListener("change", handleChange);
  unsubscribeCurrent = () => media.removeEventListener("change", handleChange);
}

/** Call once, after `get_config` resolves, with the persisted `theme`. */
export function initTheme(theme: string): void {
  applyTheme(theme);
}
