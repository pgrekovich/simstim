import { Fragment, useEffect, useRef, type ReactNode } from "react";
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

interface TurnItem {
  /** Index into the original `lines` array: keys find-match highlighting and refs. */
  index: number;
  timestamp: string;
  text: string;
}

type Block =
  | { kind: "raw"; index: number; raw: string }
  | { kind: "turn"; speaker: "Me" | "Them"; items: TurnItem[] };

/**
 * Groups consecutive lines from one speaker into a single visual turn
 * (owner request, 2026-09-01): the transcript shows one ME/THEM label per
 * turn instead of one per ASR chunk, with the turn's first timestamp. This
 * is display-only -- the file keeps one timestamped line per chunk, and
 * each chunk stays its own seekable fragment inside the turn. A line the
 * pattern can't parse renders as-is and breaks the grouping around it.
 */
function groupIntoBlocks(lines: string[]): Block[] {
  const blocks: Block[] = [];
  lines.forEach((raw, index) => {
    const parsed = parseLine(raw);
    if (!parsed.timestamp || !parsed.speaker) {
      blocks.push({ kind: "raw", index, raw });
      return;
    }
    const item: TurnItem = { index, timestamp: parsed.timestamp, text: parsed.text ?? "" };
    const last = blocks[blocks.length - 1];
    if (last && last.kind === "turn" && last.speaker === parsed.speaker) {
      last.items.push(item);
    } else {
      blocks.push({ kind: "turn", speaker: parsed.speaker, items: [item] });
    }
  });
  return blocks;
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
  /**
   * Scroll-to-citation (stage 4.2): centers the transcript on the line
   * nearest `timestamp` when `id` changes (the same monotonic-id recipe as
   * `TranscriptFindRequest`). Nearest, not exact: a chat citation's second
   * usually names a real line, but a re-transcribe between the answer and
   * the click can shift timestamps slightly.
   */
  jumpRequest?: { id: number; timestamp: string } | null;
}

/**
 * The index of the parseable line whose timestamp is nearest to `timestamp`
 * (ties go to the earlier line), or -1 when no line parses at all.
 */
export function nearestLineIndex(lines: string[], timestamp: string): number {
  const target = timestampToSeconds(timestamp);
  let best = -1;
  let bestDistance = Number.POSITIVE_INFINITY;
  lines.forEach((raw, index) => {
    const parsed = parseLine(raw);
    if (!parsed.timestamp) return;
    const distance = Math.abs(timestampToSeconds(parsed.timestamp) - target);
    if (distance < bestDistance) {
      best = index;
      bestDistance = distance;
    }
  });
  return best;
}

export function TranscriptView({
  lines,
  interimMe,
  interimThem,
  onSeek,
  matches = [],
  activeMatchIndex = -1,
  jumpRequest = null,
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

  const handledJumpRef = useRef(0);
  useEffect(() => {
    if (!jumpRequest || handledJumpRef.current === jumpRequest.id) return;
    handledJumpRef.current = jumpRequest.id;
    const index = nearestLineIndex(lines, jumpRequest.timestamp);
    if (index === -1) return;
    wasAtBottomRef.current = false;
    const line = lineRefs.current[index];
    if (line && typeof line.scrollIntoView === "function") {
      line.scrollIntoView({ block: "center" });
    }
  }, [jumpRequest, lines]);

  return (
    <Card className="min-h-0 flex-1 flex-col overflow-hidden border-0 bg-transparent p-0 shadow-none">
      <div ref={scrollRef} onScroll={handleScroll} className="h-full overflow-y-auto px-6 pb-5 pt-[6px]">
        {isEmpty ? (
          <p className="flex h-full items-center justify-center text-center font-serif text-[14px] text-muted-foreground">
            transcript will appear here
          </p>
        ) : (
          <div className="flex flex-col gap-[14px]">
            {groupIntoBlocks(lines).map((block) => {
              if (block.kind === "raw") {
                return (
                  <p
                    key={block.index}
                    ref={(element) => { lineRefs.current[block.index] = element; }}
                    className="whitespace-pre-wrap font-serif text-[15px] leading-[1.6] text-card-foreground"
                  >
                    {highlightedText(block.raw, block.index, matches, activeMatchIndex)}
                  </p>
                );
              }
              return (
                <div key={block.items[0].index} className="flex flex-col gap-[3px]">
                  <span className="flex items-baseline gap-[9px]">
                    <span
                      className={
                        block.speaker === "Them"
                          ? "inline-flex h-[17px] items-center rounded-[4px] border border-[var(--voice-remote)] px-[7px] font-mono text-[9.5px] font-semibold uppercase tracking-[0.08em] text-[var(--voice-remote-foreground)]"
                          : "inline-flex h-[17px] items-center rounded-[4px] border border-foreground bg-foreground px-[7px] font-mono text-[9.5px] font-semibold uppercase tracking-[0.08em] text-background"
                      }
                    >
                      {block.speaker}
                    </span>
                    <span className="font-mono text-[10px] text-muted-foreground">
                      {block.items[0].timestamp}
                    </span>
                  </span>
                  {/* One flowing paragraph per turn; every ASR chunk inside
                      stays its own seekable fragment (its timestamp is on
                      the hover title, since only the turn's first one is
                      printed). */}
                  <p className="whitespace-pre-wrap font-serif text-[15px] leading-[1.6] text-card-foreground">
                    {block.items.map((item, j) => {
                      const body = highlightedText(item.text, item.index, matches, activeMatchIndex);
                      return (
                        <Fragment key={item.index}>
                          {j > 0 && " "}
                          {onSeek ? (
                            <button
                              ref={(element) => { lineRefs.current[item.index] = element; }}
                              type="button"
                              title={item.timestamp}
                              onClick={() => onSeek(timestampToSeconds(item.timestamp))}
                              className="-mx-0.5 inline rounded-[5px] px-0.5 text-left align-baseline transition-colors [box-decoration-break:clone] hover:bg-foreground/[0.05] focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
                            >
                              {body}
                            </button>
                          ) : (
                            <span ref={(element) => { lineRefs.current[item.index] = element; }}>
                              {body}
                            </span>
                          )}
                        </Fragment>
                      );
                    })}
                  </p>
                </div>
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
