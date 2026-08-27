import { useEffect, useState } from "react";
import { emitTo, listen } from "@tauri-apps/api/event";
import {
  getConfig,
  menuBarCurrentState,
  menuBarHide,
  showMainWindow,
  type MenuBarStatePayload,
} from "@/lib/ipc";
import { initTheme } from "@/lib/theme";

function formatElapsed(startedAtMs: number, now: number): string {
  const total = Math.max(0, Math.floor((now - startedAtMs) / 1000));
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const seconds = total % 60;
  return hours > 0
    ? `${String(hours).padStart(2, "0")}:${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}`
    : `${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}`;
}

export default function MenuBarApp({ previewState }: { previewState?: MenuBarStatePayload }) {
  const [state, setState] = useState<MenuBarStatePayload>(previewState ?? { kind: "idle" });
  const [now, setNow] = useState(Date.now());

  useEffect(() => {
    if (previewState) return;
    getConfig()
      .then((config) => initTheme(config.theme))
      .catch(() => {});
  }, [previewState]);

  useEffect(() => {
    if (previewState) return;
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
  }, [previewState]);

  useEffect(() => {
    if (state.kind !== "recording") return;
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, [state]);

  async function openWindow() {
    await showMainWindow();
    await menuBarHide();
  }

  async function addNote() {
    await emitTo("main", "menu-bar-add-note");
    await menuBarHide();
  }

  async function toggleRecording() {
    await emitTo("main", "menu-bar-toggle-recording");
    await menuBarHide();
  }

  const recording = state.kind === "recording";
  return (
    <main className="flex h-screen w-screen items-start justify-center pt-2.5" aria-label="simstim menu bar">
      <section className="relative w-[296px] overflow-hidden rounded-xl border border-border bg-popover text-popover-foreground shadow-[0_14px_26px_rgba(20,17,12,0.32)] dark:shadow-[0_14px_26px_rgba(0,0,0,0.52)]">
        {recording && (
          <div className="h-0.5 bg-[linear-gradient(90deg,transparent_0%,var(--destructive)_28%,var(--destructive)_72%,transparent_100%)]" />
        )}
        <div className="flex flex-col gap-[11px] px-3.5 pb-3 pt-[13px]">
          <div className="flex min-w-0 items-center gap-2">
            <span
              className={
                recording
                  ? "size-1.5 shrink-0 rounded-full bg-destructive motion-safe:animate-pulse"
                  : "size-1.5 shrink-0 rounded-full bg-muted-foreground/45"
              }
            />
            <p className="min-w-0 flex-1 truncate font-serif text-[14px] font-semibold leading-[18px]">
              {recording ? state.title : "Ready to record"}
            </p>
            {recording && (
              <span className="shrink-0 font-mono text-[11.5px] font-medium text-muted-foreground">
                {formatElapsed(state.started_at_ms, now)}
              </span>
            )}
          </div>

          <p className="font-mono text-[10.5px] leading-[14px] text-muted-foreground">
            {recording ? "Recording locally" : "⌘⇧N starts a new recording"}
          </p>

          <div className="h-px bg-border" />

          <div className="flex items-center gap-[7px]">
            {recording ? (
              <button
                type="button"
                className="flex h-[30px] flex-1 items-center justify-center rounded-full bg-foreground font-mono text-[11.5px] font-semibold text-background outline-none hover:opacity-85 focus-visible:ring-2 focus-visible:ring-ring"
                onClick={() => void addNote()}
              >
                Add note
              </button>
            ) : (
              <button
                type="button"
                className="flex h-[30px] flex-1 items-center justify-center rounded-full bg-foreground font-mono text-[11.5px] font-semibold text-background outline-none hover:opacity-85 focus-visible:ring-2 focus-visible:ring-ring"
                onClick={() => void toggleRecording()}
              >
                Start recording
              </button>
            )}
            <button
              type="button"
              className="flex h-[30px] flex-1 items-center justify-center rounded-full border border-border bg-muted/70 font-mono text-[11.5px] font-medium outline-none hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring"
              onClick={() => void openWindow()}
            >
              Open window
            </button>
            {recording && (
              <button
                type="button"
                aria-label="Stop"
                className="flex size-[30px] shrink-0 items-center justify-center rounded-full border border-destructive/30 bg-destructive/10 text-destructive outline-none hover:bg-destructive/20 focus-visible:ring-2 focus-visible:ring-ring"
                onClick={() => void toggleRecording()}
              >
                <span className="size-[9px] rounded-sm bg-current" />
              </button>
            )}
          </div>
        </div>
      </section>
    </main>
  );
}
