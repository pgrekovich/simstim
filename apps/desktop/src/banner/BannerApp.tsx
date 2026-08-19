import { useEffect, useState } from "react";
import { emitTo, listen } from "@tauri-apps/api/event";
import { ChevronDown } from "lucide-react";
import { Button } from "@/components/ui/button";
import appIcon from "../../src-tauri/icons/128x128.png";
import {
  bannerCurrentState,
  bannerDismiss,
  getConfig,
  showMainWindow,
  type BannerStatePayload,
} from "@/lib/ipc";
import { initTheme } from "@/lib/theme";

/** How long the detected banner stays up before dismissing itself. */
export const AUTO_DISMISS_MS = 30_000;
const TICK_MS = 100;

/**
 * The floating banner window's whole UI (main.tsx renders this instead of
 * App when the window label is "banner"). The backend owns visibility: it
 * pushes `banner-state` and shows/hides the native window; this component
 * just renders the pushed state. Start/Stop forward to the main window so
 * recording keeps its single code path (App's handleStart/handleStop).
 */
export default function BannerApp() {
  const [state, setState] = useState<BannerStatePayload>({ kind: "hidden" });
  const [remainingMs, setRemainingMs] = useState(AUTO_DISMISS_MS);
  const [hovered, setHovered] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);

  useEffect(() => {
    getConfig()
      .then((config) => initTheme(config.theme))
      .catch(() => {
        // Same defensive shape as App: get_config does not actually reject.
      });
  }, []);

  useEffect(() => {
    let cancelled = false;
    // listen() must be registered synchronously (before any await): a push
    // arriving between mount and a late subscription would be lost.
    const unlisten = listen<BannerStatePayload>("banner-state", (event) => {
      setState(event.payload);
    });
    bannerCurrentState()
      .then((seed) => {
        // Seed only if nothing was pushed meanwhile: a push is fresher.
        if (!cancelled) setState((current) => (current.kind === "hidden" ? seed : current));
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      unlisten.then((f) => f());
    };
  }, []);

  useEffect(() => {
    // Any pushed state change closes the dropdown; a detected state also
    // resets the timer AND the hover flag: when the banner hides with the
    // pointer inside it (Start, auto-stop, cleared) the card unmounts and
    // onMouseLeave never fires, which would leave the NEXT banner's timer
    // permanently paused.
    setMenuOpen(false);
    if (state.kind !== "detected") return;
    setRemainingMs(AUTO_DISMISS_MS);
    setHovered(false);
  }, [state]);

  useEffect(() => {
    if (state.kind !== "detected" || hovered) return;
    const id = setInterval(() => setRemainingMs((ms) => Math.max(0, ms - TICK_MS)), TICK_MS);
    return () => clearInterval(id);
  }, [state, hovered]);

  useEffect(() => {
    if (state.kind !== "detected" || remainingMs > 0) return;
    // Hide locally first so this effect cannot re-fire; the backend hides
    // the native window in response to the dismiss.
    setState({ kind: "hidden" });
    void bannerDismiss();
  }, [state, remainingMs]);

  if (state.kind === "hidden") return null;

  const detected = state.kind === "detected";

  function onPrimary() {
    if (state.kind === "detected") {
      void emitTo("main", "banner-start", {
        bundle_id: state.bundle_id,
        app_name: state.app_name,
      });
    } else {
      void emitTo("main", "banner-stop");
    }
  }

  // Granola-style layout (owner e2e feedback): no close button (the
  // detected state dismisses itself, the ended state has Dismiss in the
  // dropdown), one prominent split button with the app icon so the popup
  // is identifiable, secondary actions behind the chevron.
  return (
    <div
      role="status"
      className="flex h-screen w-screen items-start p-2"
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
    >
      <div className="relative w-full">
        <div className="relative overflow-hidden rounded-xl border bg-background shadow-lg">
          <div className="flex items-center gap-3 p-3">
            <div className="min-w-0 flex-1">
              <p className="text-sm font-semibold">
                {detected ? "Call detected" : "Call seems over"}
              </p>
              {state.kind === "detected" && (
                <p className="truncate text-xs text-muted-foreground">{state.app_name}</p>
              )}
            </div>
            <div className="flex items-stretch overflow-hidden rounded-lg shadow-sm">
              <Button className="h-10 gap-2 rounded-none px-3 text-sm" onClick={onPrimary}>
                <img src={appIcon} alt="" className="size-5 rounded" />
                {detected ? "Start recording" : "Stop"}
              </Button>
              <Button
                className="h-10 rounded-none border-l border-primary-foreground/20 px-1.5"
                aria-label="More options"
                onClick={() => setMenuOpen((open) => !open)}
              >
                <ChevronDown />
              </Button>
            </div>
          </div>
          {detected && (
            <div
              className="absolute inset-x-0 bottom-0 h-0.5 bg-primary"
              style={{ width: `${(remainingMs / AUTO_DISMISS_MS) * 100}%` }}
            />
          )}
        </div>
        {menuOpen && (
          <div className="absolute right-0 top-full z-10 mt-1 min-w-44 rounded-lg border bg-background p-1 shadow-lg">
            <button
              className="w-full rounded-md px-2.5 py-1.5 text-left text-sm hover:bg-muted"
              onClick={() => {
                setMenuOpen(false);
                void showMainWindow();
              }}
            >
              Open simstim
            </button>
            {!detected && (
              <button
                className="w-full rounded-md px-2.5 py-1.5 text-left text-sm hover:bg-muted"
                onClick={() => {
                  setMenuOpen(false);
                  void bannerDismiss();
                }}
              >
                Dismiss
              </button>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
