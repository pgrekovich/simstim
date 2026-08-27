import { useEffect, useRef, type ReactNode } from "react";
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

export interface TranscriptMatch {
  lineIndex: number;
  start: number;
  end: number;
}

/** Plain, case-insensitive matches over finalized spoken text. */
export function findTranscriptMatches(lines: string[], query: string): TranscriptMatch[] {
  const needle = query.trim();
  if (needle === "") return [];
  const escaped = needle.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

  const matches: TranscriptMatch[] = [];
  lines.forEach((raw, lineIndex) => {
    const parsed = parseLine(raw);
    const text = parsed.text ?? raw;
    for (const match of text.matchAll(new RegExp(escaped, "giu"))) {
      const start = match.index;
      matches.push({ lineIndex, start, end: start + match[0].length });
    }
  });
  return matches;
}

function highlightedText(
  text: string,
  lineIndex: number,
  matches: TranscriptMatch[],
  activeMatchIndex: number,
): ReactNode {
  const lineMatches = matches
    .map((match, matchIndex) => ({ match, matchIndex }))
    .filter(({ match }) => match.lineIndex === lineIndex);
  if (lineMatches.length === 0) return text;

  const parts: ReactNode[] = [];
  let cursor = 0;
  lineMatches.forEach(({ match, matchIndex }) => {
    if (match.start > cursor) parts.push(text.slice(cursor, match.start));
    const active = matchIndex === activeMatchIndex;
    parts.push(
      <mark
        key={`${match.start}:${match.end}`}
        data-current-match={active ? "true" : undefined}
        className={
          active
            ? "rounded-[3px] bg-destructive px-0.5 text-primary-foreground"
            : "rounded-[3px] bg-[rgb(168_132_60_/_28%)] px-0.5 text-inherit dark:bg-[rgb(201_162_77_/_28%)]"
        }
      >
        {text.slice(match.start, match.end)}
      </mark>,
    );
    cursor = match.end;
  });
  if (cursor < text.length) parts.push(text.slice(cursor));
  return parts;
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
  matches?: TranscriptMatch[];
  activeMatchIndex?: number;
}

export function TranscriptView({
  lines,
  interimMe,
  interimThem,
  onSeek,
  matches = [],
  activeMatchIndex = -1,
}: TranscriptViewProps) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const lineRefs = useRef<Array<HTMLElement | null>>([]);
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

  useEffect(() => {
    const match = matches[activeMatchIndex];
    if (!match) return;
    wasAtBottomRef.current = false;
    const line = lineRefs.current[match.lineIndex];
    if (line && typeof line.scrollIntoView === "function") {
      line.scrollIntoView({ block: "center" });
    }
  }, [activeMatchIndex, matches]);

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
                  <p
                    key={i}
                    ref={(element) => { lineRefs.current[i] = element; }}
                    className="whitespace-pre-wrap font-serif text-[15px] leading-[1.6] text-card-foreground"
                  >
                    {highlightedText(raw, i, matches, activeMatchIndex)}
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
                    {highlightedText(parsed.text ?? "", i, matches, activeMatchIndex)}
                  </span>
                </>
              );
              if (!onSeek) {
                return (
                  <div
                    key={i}
                    ref={(element) => { lineRefs.current[i] = element; }}
                    className="flex flex-col gap-[3px]"
                  >
                    {content}
                  </div>
                );
              }
              const offsetSecs = timestampToSeconds(parsed.timestamp);
              return (
                <button
                  key={i}
                  ref={(element) => { lineRefs.current[i] = element; }}
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
