import { useEffect, useRef } from "react";
import { Card } from "@/components/ui/card";

/** Matches the `[hh:mm:ss] Who: text` format `session::format_line` emits. */
const LINE_PATTERN = /^\[(\d{2}:\d{2}:\d{2})]\s+(Me|Them):\s?(.*)$/;

interface ParsedLine {
  raw: string;
  timestamp?: string;
  speaker?: "Me" | "Them";
  text?: string;
}

function parseLine(raw: string): ParsedLine {
  const match = LINE_PATTERN.exec(raw);
  if (!match) return { raw };
  const [, timestamp, speaker, text] = match;
  return { raw, timestamp, speaker: speaker as "Me" | "Them", text };
}

/** Parses a `hh:mm:ss` timestamp (as `parseLine` extracts it) into whole seconds. */
function timestampToSeconds(timestamp: string): number {
  const [hours, minutes, seconds] = timestamp.split(":").map(Number);
  return hours * 3600 + minutes * 60 + seconds;
}

interface TranscriptViewProps {
  lines: string[];
  /**
   * Live (not-yet-finalized) draft text per channel, from the
   * `transcript-interim` event. Only meaningful for the session currently
   * being recorded; omit (or pass empty strings) for a static/inactive
   * session. Rendered dimmed+italic after the finalized `lines`, one per
   * channel, cleared by the caller on the event's empty-text signal.
   */
  interimMe?: string;
  interimThem?: string;
  /**
   * Click-to-seek (Task 4, stage-0.7): called with the offset (in seconds)
   * parsed from a finalized line's `[hh:mm:ss]` timestamp when that line is
   * clicked or activated via keyboard. Lines only become interactive
   * (rendered as a real `<button>`, not a plain `<p>`) when this is
   * provided -- the caller only passes it once an `<audio>` element exists
   * to seek (i.e. the session has an audio file). A line with no
   * recognizable timestamp (falls through `parseLine`'s regex) and the
   * interim/draft lines below are never seekable, regardless of this prop:
   * there is no offset to seek to for either.
   */
  onSeek?: (offsetSecs: number) => void;
}

export function TranscriptView({ lines, interimMe, interimThem, onSeek }: TranscriptViewProps) {
  const scrollRef = useRef<HTMLDivElement>(null);
  // Tracks whether the user was scrolled to the bottom right before this
  // render's lines changed. Auto-scroll only replays that intent: it never
  // yanks the view back down if the user has scrolled up to read history.
  const wasAtBottomRef = useRef(true);
  const isEmpty = lines.length === 0 && !interimMe && !interimThem;

  function handleScroll() {
    const el = scrollRef.current;
    if (!el) return;
    const distanceFromBottom = el.scrollHeight - el.scrollTop - el.clientHeight;
    wasAtBottomRef.current = distanceFromBottom < 24;
  }

  useEffect(() => {
    const el = scrollRef.current;
    if (!el || !wasAtBottomRef.current) return;
    el.scrollTop = el.scrollHeight;
  }, [lines, interimMe, interimThem]);

  return (
    <Card className="min-h-0 flex-1 flex-col overflow-hidden border-0 bg-transparent p-0 shadow-none">
      <div ref={scrollRef} onScroll={handleScroll} className="h-full overflow-y-auto px-6 pb-5 pt-[6px]">
        {isEmpty ? (
          <p className="flex h-full items-center justify-center text-center font-serif text-[14px] text-muted-foreground">
            transcript will appear here
          </p>
        ) : (
          <div className="flex flex-col gap-[14px]">
            {lines.map((raw, i) => {
              const parsed = parseLine(raw);
              if (!parsed.timestamp) {
                return (
                  <p key={i} className="whitespace-pre-wrap font-serif text-[15px] leading-[1.6] text-card-foreground">
                    {raw}
                  </p>
                );
              }
              const content = (
                <>
                  <span className="flex items-baseline gap-[9px]">
                    <span
                      className={
                        parsed.speaker === "Them"
                          ? "inline-flex h-[17px] items-center rounded-[4px] border border-[var(--voice-remote)] px-[7px] font-mono text-[9.5px] font-semibold uppercase tracking-[0.08em] text-[var(--voice-remote-foreground)]"
                          : "inline-flex h-[17px] items-center rounded-[4px] border border-foreground bg-foreground px-[7px] font-mono text-[9.5px] font-semibold uppercase tracking-[0.08em] text-background"
                      }
                    >
                      {parsed.speaker}
                    </span>
                    <span className="font-mono text-[10px] text-muted-foreground">
                      {parsed.timestamp}
                    </span>
                  </span>
                  <span className="block whitespace-pre-wrap font-serif text-[15px] leading-[1.6] text-card-foreground">
                    {parsed.text}
                  </span>
                </>
              );
              if (!onSeek) {
                return (
                  <div key={i} className="flex flex-col gap-[3px]">
                    {content}
                  </div>
                );
              }
              const offsetSecs = timestampToSeconds(parsed.timestamp);
              return (
                <button
                  key={i}
                  type="button"
                  onClick={() => onSeek(offsetSecs)}
                  className="-mx-2 flex w-[calc(100%+1rem)] flex-col gap-[3px] rounded-[7px] px-2 text-left transition-colors hover:bg-foreground/[0.05] focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
                >
                  {content}
                </button>
              );
            })}
            {interimMe && (
              <div className="flex flex-col gap-[3px]">
                <span className="flex items-baseline gap-[9px]">
                  <span className="inline-flex h-[17px] items-center rounded-[4px] border border-foreground bg-foreground px-[7px] font-mono text-[9.5px] font-semibold uppercase not-italic tracking-[0.08em] text-background">Me</span>
                  <span className="font-mono text-[10px] not-italic text-muted-foreground">···</span>
                </span>
                <p className="whitespace-pre-wrap font-serif text-[15px] italic leading-[1.6] text-muted-foreground">{interimMe}</p>
              </div>
            )}
            {interimThem && (
              <div className="flex flex-col gap-[3px]">
                <span className="flex items-baseline gap-[9px]">
                  <span className="inline-flex h-[17px] items-center rounded-[4px] border border-[var(--voice-remote)] px-[7px] font-mono text-[9.5px] font-semibold uppercase not-italic tracking-[0.08em] text-[var(--voice-remote-foreground)]">Them</span>
                  <span className="font-mono text-[10px] not-italic text-muted-foreground">···</span>
                </span>
                <p className="whitespace-pre-wrap font-serif text-[15px] italic leading-[1.6] text-muted-foreground">{interimThem}</p>
              </div>
            )}
          </div>
        )}
      </div>
    </Card>
  );
}
