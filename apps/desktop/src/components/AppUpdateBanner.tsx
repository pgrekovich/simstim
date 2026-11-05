import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { Download, LoaderCircle } from "lucide-react";
import { Button } from "@/components/ui/button";
import { checkAppUpdate, installAppUpdate, type AppUpdate, type AppUpdateProgress } from "@/lib/ipc";

/** Progress text while an update installs: "Downloading 43%", "Installing...", "Restarting...". */
export function describeUpdateProgress(progress: AppUpdateProgress): string {
  switch (progress.phase) {
    case "downloading": {
      if (progress.total === null || progress.total === 0) return "Downloading...";
      const percent = Math.min(100, Math.round((progress.received / progress.total) * 100));
      return `Downloading ${percent}%`;
    }
    case "installing":
      return "Installing...";
    case "restarting":
      return "Restarting...";
    case "error":
      return progress.message ?? "update failed";
  }
}

/**
 * Offers a newer Simstim release above the notes. The backend checks the
 * project's GitHub releases on launch and focus, at most once a day; this
 * banner only shows what it found. Installing is a click, never automatic,
 * and the app restarts into the new version when the signed archive has
 * been verified and swapped in. Offline checks stay quiet.
 */
export function AppUpdateBanner() {
  const [update, setUpdate] = useState<AppUpdate | null>(null);
  const [dismissedVersion, setDismissedVersion] = useState<string | null>(null);
  const [progress, setProgress] = useState<AppUpdateProgress | null>(null);
  const [error, setError] = useState<string | null>(null);
  const installing = useRef(false);
  const mounted = useRef(true);

  useEffect(() => {
    mounted.current = true;
    let checking = false;
    async function check() {
      if (checking || installing.current) return;
      checking = true;
      try {
        const status = await checkAppUpdate();
        if (mounted.current) setUpdate(status.update);
      } catch {
        // Offline checks stay quiet; Settings shows the error on an explicit check.
      } finally {
        checking = false;
      }
    }
    void check();
    window.addEventListener("focus", check);
    const unlistenChecked = listen("app-update-checked", () => { void check(); }).catch(() => () => {});
    const unlistenProgress = listen<AppUpdateProgress>("app-update-progress", ({ payload }) => {
      if (!mounted.current) return;
      if (payload.phase === "error") {
        setProgress(null);
        setError(payload.message ?? "update failed");
        return;
      }
      setProgress(payload);
    }).catch(() => () => {});
    return () => {
      mounted.current = false;
      window.removeEventListener("focus", check);
      void unlistenChecked.then((stop) => stop());
      void unlistenProgress.then((stop) => stop());
    };
  }, []);

  async function install() {
    if (installing.current) return;
    installing.current = true;
    setError(null);
    setProgress({ phase: "downloading", received: 0, total: null, message: null });
    try {
      // Resolves only when the install failed before restart; success restarts the app.
      await installAppUpdate();
    } catch (failure) {
      if (mounted.current) setError(String(failure));
    } finally {
      installing.current = false;
      if (mounted.current) setProgress(null);
    }
  }

  const busy = progress !== null;
  if (!busy && (update === null || dismissedVersion === update.latest_version)) return null;

  return (
    <aside aria-label="Simstim update" className="mx-5 mb-3 flex shrink-0 flex-wrap items-center gap-x-3 gap-y-2 rounded-lg border border-primary/20 bg-primary/5 px-3 py-2.5">
      {busy ? <LoaderCircle aria-hidden="true" className="size-4 animate-spin text-muted-foreground" /> : <Download aria-hidden="true" className="size-4 text-muted-foreground" />}
      <div className="min-w-0 flex-1 text-[12px]">
        <p role="status" className="font-medium">
          {busy ? describeUpdateProgress(progress) : `Simstim ${update?.latest_version} is available`}
        </p>
        {!busy && update && (
          <p className="mt-0.5 text-muted-foreground">
            You have {update.current_version}. Your notes, models and settings stay where they are.
          </p>
        )}
        {error && <p role="alert" className="mt-1 text-destructive [overflow-wrap:anywhere]">{error}</p>}
      </div>
      {!busy && (
        <>
          <Button size="xs" onClick={() => void install()}>Update and restart</Button>
          <Button variant="ghost" size="xs" onClick={() => {
            setDismissedVersion(update?.latest_version ?? null);
            setError(null);
          }}>Later</Button>
        </>
      )}
    </aside>
  );
}
