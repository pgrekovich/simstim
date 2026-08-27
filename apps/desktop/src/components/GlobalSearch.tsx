import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { CornerDownLeft, Search } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from "@/components/ui/dialog";
import { cn } from "@/lib/utils";
import { searchSessions, type SearchHit, type SessionMeta } from "@/lib/ipc";

interface GlobalSearchProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  sessions: SessionMeta[];
  onOpenSession: (path: string) => void;
}

interface SearchRow {
  id: string;
  kind: "note" | "spoken";
  path: string;
  title: string;
  date: string;
  durationSecs: number | null;
  speaker?: "Me" | "Them";
  timestamp?: string;
  text?: string;
}

const TRANSCRIPT_LINE = /^\[(\d{2}:\d{2}:\d{2})]\s+(Me|Them):\s*(.*)$/;

export function parseTranscriptSnippet(text: string): {
  timestamp?: string;
  speaker?: "Me" | "Them";
  text: string;
} {
  const match = TRANSCRIPT_LINE.exec(text);
  if (!match) return { text };
  return {
    timestamp: match[1],
    speaker: match[2] as "Me" | "Them",
    text: match[3],
  };
}

export function highlightSegments(
  text: string,
  query: string,
): { text: string; match: boolean }[] {
  const needle = query.trim().toLowerCase();
  if (needle === "") return [{ text, match: false }];
  const lower = text.toLowerCase();
  const segments: { text: string; match: boolean }[] = [];
  let cursor = 0;
  for (;;) {
    const at = lower.indexOf(needle, cursor);
    if (at === -1) break;
    if (at > cursor) segments.push({ text: text.slice(cursor, at), match: false });
    segments.push({ text: text.slice(at, at + needle.length), match: true });
    cursor = at + needle.length;
  }
  if (cursor < text.length) segments.push({ text: text.slice(cursor), match: false });
  return segments.length > 0 ? segments : [{ text, match: false }];
}

function Highlighted({ text, query, inverted = false }: { text: string; query: string; inverted?: boolean }) {
  return highlightSegments(text, query).map((segment, index) =>
    segment.match ? (
      <mark
        key={index}
        className={cn(
          "rounded-[2px] px-0.5",
          inverted ? "bg-background/20 text-background" : "bg-primary/15 text-foreground",
        )}
      >
        {segment.text}
      </mark>
    ) : (
      <span key={index}>{segment.text}</span>
    ),
  );
}

function formatDate(dateStr: string): string {
  const date = new Date(dateStr);
  const now = new Date();
  const start = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
  const target = new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();
  const days = Math.round((start - target) / 86_400_000);
  if (days === 0) return "today";
  if (days === 1) return "yesterday";
  return date.toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
  });
}

function formatDuration(durationSecs: number | null): string | null {
  if (durationSecs === null) return null;
  const whole = Math.max(0, Math.floor(durationSecs));
  const hours = Math.floor(whole / 3600);
  const minutes = Math.floor((whole % 3600) / 60);
  const seconds = whole % 60;
  return hours > 0
    ? `${hours}:${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}`
    : `${minutes}:${String(seconds).padStart(2, "0")}`;
}

function rowsFromHits(hits: SearchHit[], query: string): SearchRow[] {
  const blank = query.trim() === "";
  const notes: SearchRow[] = [];
  const spoken: SearchRow[] = [];
  for (const hit of hits) {
    if (blank || hit.snippets.some((snippet) => snippet.section !== "transcript")) {
      notes.push({
        id: `note:${hit.path}`,
        kind: "note",
        path: hit.path,
        title: hit.title,
        date: hit.date,
        durationSecs: hit.duration_secs,
      });
    }
    hit.snippets.forEach((snippet, index) => {
      if (snippet.section !== "transcript") return;
      const parsed = parseTranscriptSnippet(snippet.text);
      spoken.push({
        id: `spoken:${hit.path}:${index}`,
        kind: "spoken",
        path: hit.path,
        title: hit.title,
        date: hit.date,
        durationSecs: hit.duration_secs,
        ...parsed,
      });
    });
  }
  return [...notes, ...spoken];
}

/** Keyboard-first local search over titles, notes, and transcript lines. */
export function GlobalSearch({
  open,
  onOpenChange,
  sessions,
  onOpenSession,
}: GlobalSearchProps) {
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [activeIndex, setActiveIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!open) return;
    setQuery("");
    setError(null);
    setActiveIndex(0);
    window.requestAnimationFrame(() => inputRef.current?.focus());
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const trimmed = query.trim();
    if (trimmed === "") {
      setLoading(false);
      setError(null);
      setHits(
        sessions.map((session) => ({ ...session, snippets: [], total_matches: 0 })),
      );
      return;
    }
    let cancelled = false;
    setLoading(true);
    setError(null);
    const timer = window.setTimeout(() => {
      searchSessions(query)
        .then((next) => {
          if (!cancelled) setHits(next);
        })
        .catch((cause) => {
          if (!cancelled) {
            setHits([]);
            setError(String(cause));
          }
        })
        .finally(() => {
          if (!cancelled) setLoading(false);
        });
    }, 150);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [open, query, sessions]);

  const rows = useMemo(() => rowsFromHits(hits, query), [hits, query]);
  const noteRows = rows.filter((row) => row.kind === "note");
  const spokenRows = rows.filter((row) => row.kind === "spoken");
  const resultCount = query.trim() === ""
    ? rows.length
    : hits.reduce((total, hit) => total + hit.total_matches, 0);

  useEffect(() => {
    setActiveIndex((current) => Math.min(current, Math.max(0, rows.length - 1)));
  }, [rows.length]);

  useEffect(() => {
    const active = document.getElementById(`global-search-result-${activeIndex}`);
    if (active && typeof active.scrollIntoView === "function") {
      active.scrollIntoView({ block: "nearest" });
    }
  }, [activeIndex]);

  function openRow(row: SearchRow) {
    onOpenSession(row.path);
    handleOpenChange(false);
  }

  function handleOpenChange(nextOpen: boolean) {
    if (!nextOpen) {
      setQuery("");
      setError(null);
      setActiveIndex(0);
    }
    onOpenChange(nextOpen);
  }

  function handleKeyDown(event: KeyboardEvent<HTMLInputElement>) {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setActiveIndex((index) => (rows.length === 0 ? 0 : (index + 1) % rows.length));
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      setActiveIndex((index) => (rows.length === 0 ? 0 : (index - 1 + rows.length) % rows.length));
    } else if (event.key === "Enter" && rows[activeIndex]) {
      event.preventDefault();
      openRow(rows[activeIndex]);
    }
  }

  function renderRow(row: SearchRow) {
    const index = rows.indexOf(row);
    const duration = formatDuration(row.durationSecs);
    const active = activeIndex === index;
    const metadataClass = active ? "text-background/60" : "text-muted-foreground";
    return (
      <button
        key={row.id}
        id={`global-search-result-${index}`}
        type="button"
        role="option"
        aria-label={
          row.kind === "spoken"
            ? `${row.speaker ?? "Spoken"} in ${row.title}${row.timestamp ? ` at ${row.timestamp}` : ""}: ${row.text ?? ""}`
            : `${row.title}, ${[formatDate(row.date), duration].filter(Boolean).join(", ")}`
        }
        aria-selected={activeIndex === index}
        onMouseMove={() => setActiveIndex(index)}
        onClick={() => openRow(row)}
        className={cn(
          "w-full rounded-[7px] px-3 py-2 text-left",
          active ? "bg-foreground text-background" : "hover:bg-foreground/[0.05]",
        )}
      >
        {row.kind === "note" ? (
          <span className="flex items-baseline justify-between gap-3">
            <span className="min-w-0 truncate text-[13.5px] font-medium">
              <Highlighted text={row.title} query={query} inverted={active} />
            </span>
            <span className={cn("shrink-0 font-mono text-[10px]", metadataClass)}>
              {[formatDate(row.date), duration].filter(Boolean).join(" · ")}
            </span>
          </span>
        ) : (
          <span className="flex flex-col gap-1">
            <span className="flex items-center gap-2">
              <span
                className={cn(
                  "inline-flex h-[17px] shrink-0 items-center rounded-[4px] border px-1.5 font-mono text-[9px] font-semibold uppercase tracking-[0.06em]",
                  active
                    ? "border-background/65 text-background"
                    : row.speaker === "Them"
                      ? "border-[var(--voice-remote)] text-[var(--voice-remote-foreground)]"
                      : "border-foreground bg-foreground text-background",
                )}
              >
                {row.speaker ?? "Said"}
              </span>
              <span className={cn("min-w-0 truncate font-mono text-[10px]", metadataClass)}>
                {row.title}{row.timestamp ? ` · ${row.timestamp}` : ""}
              </span>
            </span>
            {row.text && (
              <span className={cn(
                "line-clamp-2 font-serif text-[14px] leading-[1.5]",
                active ? "text-background/90" : "text-quiet-foreground",
              )}>
                <Highlighted text={row.text} query={query} inverted={active} />
              </span>
            )}
          </span>
        )}
      </button>
    );
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent
        showCloseButton={false}
        overlayClassName="!bg-foreground/30"
        className="top-16 grid max-h-[min(620px,calc(100vh-96px))] w-[600px] max-w-[calc(100%-32px)] translate-y-0 grid-rows-[auto_minmax(0,1fr)_auto] gap-0 overflow-hidden rounded-xl border-border bg-popover p-0 shadow-[0_28px_70px_rgb(10_8_4/0.32)] sm:max-w-[600px]"
        onOpenAutoFocus={(event) => {
          event.preventDefault();
          inputRef.current?.focus();
        }}
      >
        <DialogTitle className="sr-only">Search all notes</DialogTitle>
        <DialogDescription className="sr-only">
          Search note titles, notes, and spoken transcript text.
        </DialogDescription>

        <div className="flex h-[52px] items-center gap-3 border-b px-[18px]">
          <Search aria-hidden="true" className="size-4 shrink-0 text-muted-foreground" />
          <input
            ref={inputRef}
            type="search"
            aria-label="Search all notes"
            aria-controls="global-search-results"
            aria-activedescendant={rows[activeIndex] ? `global-search-result-${activeIndex}` : undefined}
            value={query}
            onChange={(event) => {
              setQuery(event.target.value);
              setActiveIndex(0);
            }}
            onKeyDown={handleKeyDown}
            placeholder="Search notes and transcripts"
            className="min-w-0 flex-1 bg-transparent text-[15px] outline-none placeholder:text-muted-foreground"
          />
          <span className="shrink-0 font-mono text-[10px] text-muted-foreground">
            {loading ? "searching" : `${resultCount} ${resultCount === 1 ? "result" : "results"}`}
          </span>
        </div>

        <div id="global-search-results" role="listbox" className="min-h-0 overflow-y-auto p-2">
          {error ? (
            <div role="alert" className="px-3 py-8 text-center text-[12.5px] text-destructive">
              Could not search: {error}
            </div>
          ) : rows.length === 0 && !loading ? (
            <div className="px-3 py-8 text-center text-[12.5px] text-muted-foreground">
              {query.trim() === "" ? "No notes yet" : `No matches for “${query}”`}
            </div>
          ) : (
            <>
              {noteRows.length > 0 && (
                <section aria-label="Notes">
                  <p className="px-3 pb-1.5 pt-2 font-mono text-[9.5px] font-medium uppercase tracking-[0.08em] text-muted-foreground">
                    Notes
                  </p>
                  {noteRows.map(renderRow)}
                </section>
              )}
              {spokenRows.length > 0 && (
                <section aria-label="Spoken">
                  <p className="px-3 pb-1.5 pt-3 font-mono text-[9.5px] font-medium uppercase tracking-[0.08em] text-muted-foreground">
                    Spoken
                  </p>
                  {spokenRows.map(renderRow)}
                </section>
              )}
            </>
          )}
        </div>

        <div className="flex h-[38px] items-center gap-4 border-t bg-card px-[18px] font-mono text-[10px] text-muted-foreground">
          <span>↑↓ move</span>
          <span className="flex items-center gap-1"><CornerDownLeft className="size-3" /> open</span>
          <span className="flex-1" />
          <span>esc</span>
        </div>
      </DialogContent>
    </Dialog>
  );
}
