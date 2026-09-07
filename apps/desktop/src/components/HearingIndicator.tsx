import { cn } from "@/lib/utils";
import type { HearingStatus } from "@/lib/ipc";

/**
 * A channel counts as "heard" while speech was detected within this window.
 * Long enough that normal turn-taking pauses don't flicker the dot, short
 * enough that a dead channel reads as dead within one glance.
 */
export const HEARD_WITHIN_MS = 6000;

function describe(label: string, silentMs: number): string {
  if (silentMs <= HEARD_WITHIN_MS) return `Hearing ${label}`;
  const secs = Math.round(silentMs / 1000);
  if (secs < 60) return `Nothing from ${label} for ${secs}s`;
  return `Nothing from ${label} for ${Math.round(secs / 60)} min`;
}

/**
 * Two live dots in the toolbar while recording: you, them. Each lights while
 * its channel has produced speech recently. The two ways a call records as
 * silence (a mic blocked by the hardened runtime, a system-audio tap without
 * its permission) used to look identical to a working recording until the
 * call was over; now they look like a dot that never lights.
 */
export function HearingIndicator({ status }: { status: HearingStatus | null }) {
  const channels: [string, number | null][] = [
    ["you", status?.me_silent_ms ?? null],
    ["them", status?.them_silent_ms ?? null],
  ];
  return (
    <div className="flex items-center gap-2.5" aria-label="Who is being heard" role="status">
      {channels.map(([label, silentMs]) => {
        const heard = silentMs !== null && silentMs <= HEARD_WITHIN_MS;
        return (
          <span
            key={label}
            title={silentMs === null ? "Listening..." : describe(label, silentMs)}
            className="flex items-center gap-1.5 text-[11px] tabular-nums text-muted-foreground"
          >
            <span
              data-testid={`hearing-${label}`}
              data-heard={heard ? "true" : "false"}
              aria-hidden="true"
              className={cn(
                "size-1.5 rounded-full transition-colors duration-300",
                heard ? "bg-success" : "bg-foreground/20",
              )}
            />
            {label}
          </span>
        );
      })}
    </div>
  );
}
