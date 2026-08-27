import { useEffect, useState } from "react";
import { emitTo, listen } from "@tauri-apps/api/event";
import { ExternalLink, Square } from "lucide-react";
import {
  getConfig,
  menuBarCurrentState,
  menuBarHide,
  showMainWindow,
  type MenuBarStatePayload,
} from "@/lib/ipc";
import { initTheme } from "@/lib/theme";

function ReelMark({ recording }: { recording: boolean }) {
  return (
    <svg aria-hidden="true" viewBox="0 0 36 36" className="size-7 text-foreground">
      <rect x="11" y="17" width="14" height="2" fill="currentColor" opacity={recording ? 1 : 0.52} />
      <circle cx="11" cy="18" r="6" fill="none" stroke="currentColor" strokeWidth="3" opacity={recording ? 1 : 0.52} />
      <circle cx="25" cy="18" r="6" fill="none" stroke="currentColor" strokeWidth="3" opacity={recording ? 1 : 0.52} />
      {recording && (
        <>
          <circle cx="11" cy="18" r="2.6" fill="currentColor" />
          <circle cx="25" cy="18" r="2.6" fill="currentColor" />
        </>
      )}
    </svg>
  );
}

function formatElapsed(startedAtMs: number, now: number): string {
  const total = Math.max(0, Math.floor((now - startedAtMs) / 1000));
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const seconds = total % 60;
  return hours > 0
    ? `${String(hours).padStart(2, "0")}:${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}`
    : `${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}`;
}

export default function MenuBarApp() {
  const [state, setState] = useState<MenuBarStatePayload>({ kind: "idle" });
  const [now, setNow] = useState(Date.now());

  useEffect(() => {
    getConfig()
      .then((config) => initTheme(config.theme))
      .catch(() => {});
  }, []);

  useEffect(() => {
    let pushed = false;
    const unlisten = listen<MenuBarStatePayload>("menu-bar-state", (event) => {
      pushed = true;
      setState(event.payload);
      setNow(Date.now());
    });
    menuBarCurrentState()
      .then((seed) => {
        if (!pushed) setState(seed);
      })
      .catch(() => {});
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  useEffect(() => {
    if (state.kind !== "recording") return;
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, [state]);

  async function openWindow() {
    await showMainWindow();
    await menuBarHide();
  }

  async function toggleRecording() {
    await emitTo("main", "menu-bar-toggle-recording");
    await menuBarHide();
  }

  const recording = state.kind === "recording";
  return (
    <main className="flex h-screen w-screen items-start p-2" aria-label="simstim menu bar">
      <section className="relative w-full overflow-hidden rounded-xl border border-border bg-popover text-popover-foreground shadow-[0_16px_44px_rgba(20,17,12,0.34)] dark:shadow-[0_16px_44px_rgba(0,0,0,0.62)]">
        {recording && (
          <div className="h-0.5 bg-[linear-gradient(90deg,transparent_0%,var(--destructive)_28%,var(--destructive)_72%,transparent_100%)]" />
        )}
        <div className="flex h-[102px] items-center gap-3 px-4">
          <div className="flex size-10 shrink-0 items-center justify-center rounded-[10px] bg-muted">
            <ReelMark recording={recording} />
          </div>
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2">
              {recording && <span className="size-1.5 shrink-0 rounded-full bg-destructive motion-safe:animate-pulse" />}
              <p className="truncate font-serif text-[15px] font-semibold leading-5">
                {recording ? state.title : "Ready to record"}
              </p>
            </div>
            <p className="mt-1 font-mono text-[11px] text-muted-foreground">
              {recording ? `Recording locally · ${formatElapsed(state.started_at_ms, now)}` : "⌘⇧N starts a new recording"}
            </p>
          </div>
        </div>
        <div className="mx-4 h-px bg-border" />
        <div className="flex h-[64px] items-center gap-2 px-4">
          <button
            type="button"
            className={
              recording
                ? "flex h-8 flex-1 items-center justify-center gap-2 rounded-full border border-destructive/30 bg-destructive/10 font-mono text-[11.5px] font-semibold text-destructive outline-none hover:bg-destructive/20 focus-visible:ring-2 focus-visible:ring-ring"
                : "flex h-8 flex-1 items-center justify-center rounded-full bg-foreground font-mono text-[11.5px] font-semibold text-background outline-none hover:opacity-85 focus-visible:ring-2 focus-visible:ring-ring"
            }
            onClick={() => void toggleRecording()}
          >
            {recording && <Square className="size-2.5 fill-current" />}
            {recording ? "Stop" : "Start recording"}
          </button>
          <button
            type="button"
            className="flex h-8 flex-1 items-center justify-center gap-1.5 rounded-full border border-border bg-muted/70 font-mono text-[11.5px] font-medium outline-none hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring"
            onClick={() => void openWindow()}
          >
            Open window
            <ExternalLink className="size-3" />
          </button>
        </div>
      </section>
    </main>
  );
}
