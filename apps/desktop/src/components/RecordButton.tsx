import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";

function formatElapsed(ms: number): string {
  const totalSeconds = Math.max(0, Math.floor(ms / 1000));
  const mm = Math.floor(totalSeconds / 60)
    .toString()
    .padStart(2, "0");
  const ss = (totalSeconds % 60).toString().padStart(2, "0");
  return `${mm}:${ss}`;
}

interface RecordButtonProps {
  /** True from the moment `start_recording` resolves until `stop_recording` is called (covers both the "loading model" and "recording" status states). */
  recording: boolean;
  disabled?: boolean;
  /** `Date.now()` at the moment recording started; used to compute the elapsed timer. */
  startedAt: number | null;
  onClick: () => void;
}

export function RecordButton({
  recording,
  disabled,
  startedAt,
  onClick,
}: RecordButtonProps) {
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    if (!recording) return;
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, [recording]);

  const elapsed = recording && startedAt !== null ? formatElapsed(now - startedAt) : null;

  return (
    <Button
      variant={recording ? "destructive" : "default"}
      disabled={disabled}
      onClick={onClick}
      className="h-14 rounded-full px-10 text-lg"
    >
      {recording ? (
        <>
          <span className="size-2 animate-pulse rounded-full bg-current" />
          Stop {elapsed}
        </>
      ) : (
        "Start recording"
      )}
    </Button>
  );
}
