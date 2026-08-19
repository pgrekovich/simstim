import { useEffect, useState } from "react";
import { emitTo, listen } from "@tauri-apps/api/event";
import { X } from "lucide-react";
import { Button } from "@/components/ui/button";
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
    if (state.kind === "detected") setRemainingMs(AUTO_DISMISS_MS);
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

  return (
    <div
      role="status"
      className="flex h-screen w-screen items-start p-2"
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
    >
      <div className="relative w-full overflow-hidden rounded-xl border bg-background shadow-lg">
        {state.kind === "detected" ? (
          <>
            <div className="flex items-center gap-2 p-3 pb-1">
              <div className="min-w-0 flex-1">
                <p className="text-sm font-medium">Call detected</p>
                <p className="truncate text-xs text-muted-foreground">{state.app_name}</p>
              </div>
              <Button
                size="sm"
                onClick={() =>
                  void emitTo("main", "banner-start", {
                    bundle_id: state.bundle_id,
                    app_name: state.app_name,
                  })
                }
              >
                Start recording
              </Button>
              <Button
                variant="ghost"
                size="icon-sm"
                aria-label="Dismiss"
                onClick={() => void bannerDismiss()}
              >
                <X />
              </Button>
            </div>
            <div className="flex px-2 pb-2">
              <Button
                variant="ghost"
                size="xs"
                className="text-muted-foreground"
                onClick={() => void showMainWindow()}
              >
                Open simstim
              </Button>
            </div>
            <div
              className="absolute inset-x-0 bottom-0 h-0.5 bg-primary"
              style={{ width: `${(remainingMs / AUTO_DISMISS_MS) * 100}%` }}
            />
          </>
        ) : (
          <div className="flex items-center gap-2 p-3">
            <p className="min-w-0 flex-1 text-sm font-medium">Call seems over</p>
            <Button size="sm" onClick={() => void emitTo("main", "banner-stop")}>
              Stop
            </Button>
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label="Dismiss"
              onClick={() => void bannerDismiss()}
            >
              <X />
            </Button>
          </div>
        )}
      </div>
    </div>
  );
}
