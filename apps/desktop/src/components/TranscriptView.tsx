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

interface TranscriptViewProps {
  lines: string[];
}

export function TranscriptView({ lines }: TranscriptViewProps) {
  const scrollRef = useRef<HTMLDivElement>(null);
  // Tracks whether the user was scrolled to the bottom right before this
  // render's lines changed. Auto-scroll only replays that intent: it never
  // yanks the view back down if the user has scrolled up to read history.
  const wasAtBottomRef = useRef(true);

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
  }, [lines]);

  return (
    <Card className="min-h-0 flex-1 flex-col overflow-hidden p-0">
      <div ref={scrollRef} onScroll={handleScroll} className="h-full overflow-y-auto p-4">
        {lines.length === 0 ? (
          <p className="flex h-full items-center justify-center text-center text-[13px] text-muted-foreground">
            transcript will appear here
          </p>
        ) : (
          <div className="space-y-1 font-mono text-[13px] leading-relaxed">
            {lines.map((raw, i) => {
              const parsed = parseLine(raw);
              if (!parsed.timestamp) {
                return (
                  <p key={i} className="whitespace-pre-wrap">
                    {raw}
                  </p>
                );
              }
              return (
                <p key={i} className="whitespace-pre-wrap">
                  <span className="text-muted-foreground">[{parsed.timestamp}]</span>{" "}
                  <span
                    className={
                      parsed.speaker === "Them"
                        ? "text-sky-600 dark:text-sky-400"
                        : "text-foreground"
                    }
                  >
                    {parsed.speaker}:
                  </span>{" "}
                  {parsed.text}
                </p>
              );
            })}
          </div>
        )}
      </div>
    </Card>
  );
}
