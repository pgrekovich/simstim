import { useEffect, useState } from "react";
import { emitTo, listen } from "@tauri-apps/api/event";
import { ChevronDown, X } from "lucide-react";
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

  // Granola-style layout on macOS conventions (owner e2e feedback): a
  // notification-shaped card, one prominent split button carrying the app
  // icon so the popup is identifiable at a glance, secondary actions behind
  // the chevron, and a close affordance that only appears on hover.
  //
  // The card is drawn in CSS rather than with a native NSVisualEffectView
  // material (tauri's window effects): the material fills the whole window,
  // and this window is deliberately taller than the card so the dropdown
  // has room, which would leave a frosted slab hanging under the card.
  // CSS also keeps the banner identical on Windows and Linux.
  return (
    <div
      role="status"
      className="flex h-screen w-screen items-start p-2"
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
    >
      <div className="relative w-full">
        <div className="relative overflow-hidden rounded-[13px] border border-border bg-popover shadow-[0_12px_34px_rgba(20,17,12,0.28)]">
          <div className="flex items-center gap-3 py-2.5 pl-3.5 pr-2.5">
            <div className="min-w-0 flex-1">
              <p className="text-[13px] font-medium leading-tight">
                {detected ? "Call detected" : "Call seems over"}
              </p>
              {state.kind === "detected" && (
                <p className="truncate font-mono text-[10.5px] leading-tight text-muted-foreground">
                  {state.app_name}
                </p>
              )}
            </div>
            {/* Deliberately not the default primary button: in dark mode
                --primary is near-white, which reads as a glaring slab on a
                dark card. A translucent surface matches macOS controls. */}
            <div className="flex items-stretch overflow-hidden rounded-[9px] border border-border bg-card shadow-sm">
              <Button
                variant="ghost"
                className={detected ? "h-9 gap-2 rounded-none px-2.5 font-mono text-[11.5px] font-semibold hover:bg-foreground/[0.06]" : "h-9 gap-2 rounded-none px-2.5 font-mono text-[11.5px] font-semibold text-destructive hover:bg-destructive/10 hover:text-destructive"}
                onClick={onPrimary}
              >
                <img src={appIcon} alt="" className="size-5 rounded-[5px]" />
                {detected ? "Start recording" : "Stop"}
              </Button>
              <Button
                variant="ghost"
                className="h-9 rounded-none border-l border-border px-1.5 text-muted-foreground hover:bg-foreground/[0.06]"
                aria-label="More options"
                onClick={() => setMenuOpen((open) => !open)}
              >
                <ChevronDown />
              </Button>
            </div>
          </div>
          {detected && (
            <div className="absolute inset-x-0 bottom-0 h-[3px]">
              <div
                className="h-full rounded-full bg-[var(--voice-remote)]"
                style={{ width: `${(remainingMs / AUTO_DISMISS_MS) * 100}%` }}
              />
            </div>
          )}
        </div>

        {/* Hover-only close, like Granola: the banner is self-dismissing, so
            a permanent X would be visual noise on every call. */}
        {hovered && (
          <button
            aria-label="Dismiss"
            className="absolute -left-1.5 -top-1.5 flex size-[22px] items-center justify-center rounded-full border border-border bg-popover text-muted-foreground shadow-md hover:text-foreground"
            onClick={() => void bannerDismiss()}
          >
            <X className="size-3" />
          </button>
        )}

        {menuOpen && (
          <div className="absolute right-0 top-full z-10 mt-1.5 min-w-48 rounded-[10px] border border-border bg-popover p-1 shadow-[0_10px_30px_rgba(20,17,12,0.25)]">
            <button
              className="w-full rounded-md px-2.5 py-1.5 text-left text-[12.5px] hover:bg-foreground/[0.06]"
              onClick={() => {
                setMenuOpen(false);
                void showMainWindow();
              }}
            >
              Open simstim
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
