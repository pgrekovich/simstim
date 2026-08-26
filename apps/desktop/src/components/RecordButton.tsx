import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

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
  /**
   * `"default"` is the original large centered-screen pill (Stage 0.5).
   * `"compact"` is the small header pill used by the Stage 0.6 shell
   * (Task 5): same green/red + pulse + elapsed logic, just sized to sit
   * inline next to the status text and gear button.
   */
  size?: "default" | "compact";
}

export function RecordButton({
  recording,
  disabled,
  startedAt,
  onClick,
  size = "default",
}: RecordButtonProps) {
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    if (!recording) return;
    // `now` may be stale (e.g. still holding the mount-time value if
    // recording started well after mount): reset it the instant recording
    // flips true instead of waiting for the first 1s interval tick, so the
    // very first elapsed render is a real "just started" 00:00 rather than
    // an unrelated negative value that only reads as 00:00 because of the
    // Math.max clamp below.
    setNow(Date.now());
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, [recording]);

  const elapsed = recording && startedAt !== null ? formatElapsed(now - startedAt) : null;

  return (
    <Button
      variant={recording ? "destructive" : "default"}
      disabled={disabled}
      onClick={onClick}
      className={cn(
        "font-mono font-semibold tracking-[0.02em] shadow-none",
        size === "compact" ? "h-[30px] rounded-full px-3.5 text-[11.5px]" : "h-12 rounded-full px-8 text-sm",
        recording
          ? "border border-destructive/25 bg-destructive/10 text-destructive hover:bg-destructive/16"
          : "bg-foreground text-background hover:bg-foreground/90"
      )}
    >
      {recording ? (
        <>
          <span className="recording-tally size-2 rounded-[2px] bg-current" />
          Stop {elapsed}
        </>
      ) : size === "compact" ? (
        "Record"
      ) : (
        "Start recording"
      )}
    </Button>
  );
}
