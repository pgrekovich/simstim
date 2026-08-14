import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { Settings } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { RecordButton } from "@/components/RecordButton";
import { TranscriptView } from "@/components/TranscriptView";
import { getConfig, listModels, startRecording, stopRecording } from "@/lib/ipc";

/**
 * The engine (VAD + ASR) loads in a worker thread after `start_recording`
 * resolves, so there is no explicit "model ready" signal from the backend.
 * We show "loading model" until the first transcript line arrives (which
 * only happens once the engine is up and has actually transcribed
 * something) or until this fallback fires, whichever comes first. If the
 * call is silent for the whole window, this heuristic still flips the
 * status to "recording" so the UI doesn't get stuck saying "loading model"
 * forever.
 */
const MODEL_LOAD_FALLBACK_MS = 15_000;

type Status =
  | { kind: "idle" }
  | { kind: "loading-model" }
  | { kind: "recording" }
  | { kind: "saved"; path: string }
  | { kind: "error"; message: string };

function App() {
  const [status, setStatus] = useState<Status>({ kind: "idle" });
  const [lines, setLines] = useState<string[]>([]);
  const [recordingStartedAt, setRecordingStartedAt] = useState<number | null>(null);
  const [modelReady, setModelReady] = useState(true);
  const [busy, setBusy] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const loadingFallbackRef = useRef<number | null>(null);

  useEffect(() => {
    let cancelled = false;
    Promise.all([getConfig(), listModels()])
      .then(([config, models]) => {
        if (cancelled) return;
        const active = models.find((m) => m.id === config.active_model);
        setModelReady(Boolean(active?.downloaded));
      })
      .catch(() => {
        // Can't determine model status: don't block Start on a guess, let
        // start_recording's own "model not downloaded" error surface instead.
      });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    const unlisten = listen<string>("transcript-line", (event) => {
      setLines((prev) => [...prev, event.payload]);
      setStatus((prev) => (prev.kind === "loading-model" ? { kind: "recording" } : prev));
      if (loadingFallbackRef.current !== null) {
        window.clearTimeout(loadingFallbackRef.current);
        loadingFallbackRef.current = null;
      }
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  function clearLoadingFallback() {
    if (loadingFallbackRef.current !== null) {
      window.clearTimeout(loadingFallbackRef.current);
      loadingFallbackRef.current = null;
    }
  }

  async function handleStart() {
    if (busy) return;
    setBusy(true);
    setLines([]);
    try {
      await startRecording();
      setRecordingStartedAt(Date.now());
      setStatus({ kind: "loading-model" });
      clearLoadingFallback();
      loadingFallbackRef.current = window.setTimeout(() => {
        setStatus((prev) => (prev.kind === "loading-model" ? { kind: "recording" } : prev));
        loadingFallbackRef.current = null;
      }, MODEL_LOAD_FALLBACK_MS);
    } catch (err) {
      setStatus({ kind: "error", message: String(err) });
    } finally {
      setBusy(false);
    }
  }

  async function handleStop() {
    if (busy) return;
    setBusy(true);
    clearLoadingFallback();
    try {
      const path = await stopRecording();
      setRecordingStartedAt(null);
      setStatus({ kind: "saved", path });
    } catch (err) {
      setStatus({ kind: "error", message: String(err) });
    } finally {
      setBusy(false);
    }
  }

  const sessionActive = status.kind === "loading-model" || status.kind === "recording";

  return (
    <main className="mx-auto flex h-screen max-w-2xl flex-col gap-6 p-6">
      <header className="flex items-center justify-between">
        <span className="font-mono text-sm lowercase text-muted-foreground">simstim</span>
        <Button
          variant="ghost"
          size="icon"
          aria-label="Settings"
          onClick={() => setSettingsOpen(true)}
        >
          <Settings />
        </Button>
      </header>

      <div className="flex flex-col items-center gap-3">
        <RecordButton
          recording={sessionActive}
          disabled={busy || (!sessionActive && !modelReady)}
          startedAt={recordingStartedAt}
          onClick={sessionActive ? handleStop : handleStart}
        />
        {!sessionActive && !modelReady && (
          <div className="flex items-center gap-2 text-sm text-muted-foreground">
            <span>Model not downloaded</span>
            <Button variant="outline" size="sm" onClick={() => setSettingsOpen(true)}>
              Open settings
            </Button>
          </div>
        )}
      </div>

      <p
        className={
          status.kind === "error"
            ? "text-center text-sm text-destructive"
            : "text-center text-sm text-muted-foreground"
        }
      >
        {status.kind === "idle" && "idle"}
        {status.kind === "loading-model" && "loading model"}
        {status.kind === "recording" && "recording"}
        {status.kind === "saved" && `saved to ${status.path}`}
        {status.kind === "error" && status.message}
      </p>

      <TranscriptView lines={lines} />

      <Dialog open={settingsOpen} onOpenChange={setSettingsOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Settings</DialogTitle>
          </DialogHeader>
          <p className="text-sm text-muted-foreground">coming in the next task</p>
        </DialogContent>
      </Dialog>
    </main>
  );
}

export default App;
