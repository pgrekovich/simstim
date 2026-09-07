import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { CornerDownLeft, MessageCircleQuestion, Search } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from "@/components/ui/dialog";
import { cn, formatDuration } from "@/lib/utils";
import { searchSessions, type SearchHit, type SessionMeta } from "@/lib/ipc";

interface GlobalSearchProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  sessions: SessionMeta[];
  onOpenSession: (path: string, spoken?: SpokenSearchTarget) => void;
  /**
   * Hands the typed query to the Ask your notes surface as a question
   * (stage 4.2, design 8a's bottom row): reachable as the list's last row
   * and directly via ⌘↵ from the input. Absent, the row doesn't render.
   */
  onAskAllNotes?: (question: string) => void;
  /**
   * Hands the typed query to the Ask this call drawer for the OPEN note
   * (2026-09-04 ruling: one field reaches everything; the sidebar's own
   * ask row is gone). Absent when no note is open, and the row doesn't
   * render.
   */
  onAskThisCall?: (question: string) => void;
}

export interface SpokenSearchTarget {
  query: string;
  timestamp?: string;
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
  onAskAllNotes,
  onAskThisCall,
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
  // The ask rows are navigable like results but aren't SearchRows: they sit
  // after the results, only with a non-blank query -- "this call" first
  // (the narrower, usually intended scope when a note is open), then "all
  // notes" last.
  const askCallAvailable = onAskThisCall !== undefined && query.trim() !== "";
  const askAvailable = onAskAllNotes !== undefined && query.trim() !== "";
  const askCallIndex = rows.length;
  const askAllIndex = rows.length + (askCallAvailable ? 1 : 0);
  const navigableCount = rows.length + (askCallAvailable ? 1 : 0) + (askAvailable ? 1 : 0);

  function askAll() {
    if (!askAvailable) return;
    onAskAllNotes?.(query.trim());
    handleOpenChange(false);
  }

  function askThisCall() {
    if (!askCallAvailable) return;
    onAskThisCall?.(query.trim());
    handleOpenChange(false);
  }
  const resultCount = query.trim() === ""
    ? rows.length
    : hits.reduce((total, hit) => total + hit.total_matches, 0);

  useEffect(() => {
    setActiveIndex((current) => Math.min(current, Math.max(0, navigableCount - 1)));
  }, [navigableCount]);

  useEffect(() => {
    const active = document.getElementById(`global-search-result-${activeIndex}`);
    if (active && typeof active.scrollIntoView === "function") {
      active.scrollIntoView({ block: "nearest" });
    }
  }, [activeIndex]);

  function openRow(row: SearchRow) {
    if (row.kind === "spoken") {
      onOpenSession(row.path, { query: query.trim(), timestamp: row.timestamp });
    } else {
      onOpenSession(row.path);
    }
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
      setActiveIndex((index) => (navigableCount === 0 ? 0 : (index + 1) % navigableCount));
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      setActiveIndex((index) =>
        navigableCount === 0 ? 0 : (index - 1 + navigableCount) % navigableCount,
      );
    } else if (event.key === "Enter" && event.metaKey) {
      // ⌘↵ asks regardless of which row is active (design 8a).
      event.preventDefault();
      askAll();
    } else if (event.key === "Enter") {
      event.preventDefault();
      if (rows[activeIndex]) {
        openRow(rows[activeIndex]);
      } else if (askCallAvailable && activeIndex === askCallIndex) {
        askThisCall();
      } else if (askAvailable && activeIndex === askAllIndex) {
        askAll();
      }
    }
  }

  /** One of the two ask rows at the list's tail (same look, different scope). */
  function renderAskRow(index: number, label: string, ariaLabel: string, hint: string, onPick: () => void) {
    const active = activeIndex === index;
    return (
      <button
        type="button"
        role="option"
        id={`global-search-result-${index}`}
        aria-label={ariaLabel}
        aria-selected={active}
        onMouseMove={() => setActiveIndex(index)}
        onClick={onPick}
        className={cn(
          "flex w-full items-center gap-3 rounded-[7px] px-3 py-2.5 text-left",
          active ? "bg-foreground text-background" : "hover:bg-foreground/[0.05]",
        )}
      >
        <MessageCircleQuestion
          aria-hidden="true"
          className={cn("size-[15px] shrink-0", active ? "text-background/80" : "text-muted-foreground")}
        />
        <span className="min-w-0 flex-1 truncate text-[13.5px]">
          {label}: <span className="italic">“{query.trim()}”</span>
        </span>
        <span
          className={cn(
            "inline-flex h-[17px] shrink-0 items-center rounded-[4px] px-[5px] text-[11px]",
            active ? "bg-background/20 text-background" : "bg-foreground/[0.07] text-quiet-foreground",
          )}
        >
          {hint}
        </span>
      </button>
    );
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
            <span className={cn("shrink-0 text-[11px] tabular-nums", metadataClass)}>
              {[formatDate(row.date), duration].filter(Boolean).join(" · ")}
            </span>
          </span>
        ) : (
          <span className="flex flex-col gap-1">
            <span className="flex items-center gap-2">
              <span
                className={cn(
                  "inline-flex h-[17px] shrink-0 items-center rounded-[4px] border px-1.5 text-[10px] font-semibold uppercase tracking-[0.04em]",
                  active
                    ? "border-background/65 text-background"
                    : row.speaker === "Them"
                      ? "border-[var(--voice-remote)] text-[var(--voice-remote-foreground)]"
                      : "border-foreground bg-foreground text-background",
                )}
              >
                {row.speaker ?? "Said"}
              </span>
              <span className={cn("min-w-0 truncate text-[11px] tabular-nums", metadataClass)}>
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
          <span className="shrink-0 text-[11px] tabular-nums text-muted-foreground">
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
                  <p className="px-3 pb-1.5 pt-2 text-[11px] font-semibold text-muted-foreground">
                    Notes
                  </p>
                  {noteRows.map(renderRow)}
                </section>
              )}
              {spokenRows.length > 0 && (
                <section aria-label="Spoken">
                  <p className="px-3 pb-1.5 pt-3 text-[11px] font-semibold text-muted-foreground">
                    Spoken
                  </p>
                  {spokenRows.map(renderRow)}
                </section>
              )}
            </>
          )}
          {(askCallAvailable || askAvailable) && !error && (
            <div className="mt-2 flex flex-col gap-px border-t border-foreground/[0.09] pt-1.5">
              {askCallAvailable &&
                renderAskRow(
                  askCallIndex,
                  "Ask this call",
                  `Ask this call: ${query.trim()}`,
                  "↵",
                  askThisCall,
                )}
              {askAvailable &&
                renderAskRow(
                  askAllIndex,
                  "Ask across all notes",
                  `Ask across all notes: ${query.trim()}`,
                  "⌘↵",
                  askAll,
                )}
            </div>
          )}
        </div>

        <div className="flex h-[38px] items-center gap-4 border-t bg-card px-[18px] text-[11px] text-muted-foreground">
          <span>↑↓ move</span>
          <span className="flex items-center gap-1"><CornerDownLeft className="size-3" /> open</span>
          <span className="flex-1" />
          <span>esc</span>
        </div>
      </DialogContent>
    </Dialog>
  );
}
