import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { emitTo, listen } from "@tauri-apps/api/event";
import { ChevronDown, X } from "lucide-react";
import appIcon from "../../src-tauri/icons/128x128.png";
import {
  bannerCurrentState,
  bannerDismiss,
  bannerPainted,
  getConfig,
  showMainWindow,
  type BannerStatePayload,
} from "@/lib/ipc";
import { initTheme } from "@/lib/theme";

/** How long the detected banner stays up before dismissing itself. */
export const AUTO_DISMISS_MS = 30_000;

/**
 * The floating banner window's whole UI (main.tsx renders this instead of
 * App when the window label is "banner"). The backend owns visibility: it
 * pushes `banner-state` and shows/hides the native window; this component
 * just renders the pushed state. Start/Stop forward to the main window so
 * recording keeps its single code path (App's handleStart/handleStop).
 */
export default function BannerApp() {
  const [state, setState] = useState<BannerStatePayload>({ kind: "hidden" });
  /** Auto-dismiss time left; counts down only while the pointer is outside. */
  const remainingMsRef = useRef(AUTO_DISMISS_MS);
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

  // The backend shows the native window only after this confirmation, so
  // the card is in the DOM by the time the panel is ordered front. Layout
  // effect: runs after the DOM commit, before the browser paints.
  useLayoutEffect(() => {
    if (state.kind === "hidden" || state.seq === undefined) return;
    void bannerPainted(state.seq);
  }, [state]);

  useEffect(() => {
    // Any pushed state change closes the dropdown; a detected state also
    // resets the timer AND the hover flag: when the banner hides with the
    // pointer inside it (Start, auto-stop, cleared) the card unmounts and
    // onMouseLeave never fires, which would leave the NEXT banner's timer
    // permanently paused.
    setMenuOpen(false);
    if (state.kind !== "detected") return;
    remainingMsRef.current = AUTO_DISMISS_MS;
    setHovered(false);
  }, [state]);

  // One timeout for the whole countdown instead of a 100 ms interval that
  // re-rendered the card 300 times per banner. Hovering unschedules it and
  // keeps the time left; leaving reschedules the remainder.
  useEffect(() => {
    if (state.kind !== "detected" || hovered) return;
    const startedAt = Date.now();
    const id = setTimeout(() => {
      // Hide locally first so nothing re-fires; the backend hides the
      // native window in response to the dismiss.
      setState({ kind: "hidden" });
      void bannerDismiss();
    }, remainingMsRef.current);
    return () => {
      clearTimeout(id);
      remainingMsRef.current = Math.max(0, remainingMsRef.current - (Date.now() - startedAt));
    };
  }, [state, hovered]);

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

  // macOS notification layout from the design reference: app icon, message,
  // one compact split action, and a close affordance that appears on hover.
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
        <div className="relative rounded-[18px] border border-border bg-popover shadow-[0_1px_2px_rgba(0,0,0,0.12),0_3px_8px_rgba(0,0,0,0.16)] dark:shadow-[0_1px_2px_rgba(0,0,0,0.5),0_3px_8px_rgba(0,0,0,0.5)]">
          <div className="flex items-center gap-3 py-[11px] pl-[13px] pr-3">
            <img src={appIcon} alt="" className="size-[38px] shrink-0 rounded-[9px]" />
            <div className="min-w-0 flex-1">
              <p className="text-[13px] font-semibold leading-4">
                {detected ? "Call detected" : "Call seems over"}
              </p>
              {state.kind === "detected" && (
                <p className="truncate text-[12px] leading-4 text-muted-foreground">
                  {state.app_name}
                </p>
              )}
            </div>
            <div
              className={
                detected
                  ? "flex h-[34px] shrink-0 items-stretch overflow-hidden rounded-[9px] bg-primary text-primary-foreground"
                  : "flex h-[34px] shrink-0 items-stretch overflow-hidden rounded-[9px] bg-destructive text-white"
              }
            >
              <button
                type="button"
                className="px-[13px] text-[13px] font-semibold leading-none hover:bg-white/10 dark:hover:bg-black/[0.06]"
                onClick={onPrimary}
              >
                {detected ? "Start recording" : "Stop"}
              </button>
              <button
                type="button"
                className="flex w-[30px] items-center justify-center border-l border-background/20 hover:bg-white/10 dark:hover:bg-black/[0.06]"
                aria-label="More options"
                onClick={() => setMenuOpen((open) => !open)}
              >
                <ChevronDown className={menuOpen ? "size-3 rotate-180" : "size-3"} />
              </button>
            </div>
          </div>
        </div>

        {/* Hover-only close, like Granola: the banner is self-dismissing, so
            a permanent X would be visual noise on every call. */}
        {hovered && (
          <button
            aria-label="Dismiss"
            className="absolute -left-[7px] -top-[7px] flex size-[22px] items-center justify-center rounded-full border border-border bg-popover text-muted-foreground shadow-[0_1px_3px_rgba(0,0,0,0.2)] hover:bg-muted hover:text-foreground"
            onClick={() => void bannerDismiss()}
          >
            <X className="size-3" />
          </button>
        )}

        {menuOpen && (
          <div className="absolute right-0 top-full z-10 mt-0.5 w-56 rounded-[12px] border border-border bg-popover p-[5px] shadow-[0_1px_2px_rgba(0,0,0,0.12),0_3px_8px_rgba(0,0,0,0.16)]">
            <button
              className="flex h-[29px] w-full items-center rounded-[7px] px-[9px] text-left text-[13px] hover:bg-accent hover:text-accent-foreground"
              onClick={() => {
                setMenuOpen(false);
                void showMainWindow();
              }}
            >
              Open Simstim
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
