import { FileText, MessageSquareText, StickyNote } from "lucide-react";
import type { SearchHit, SearchSnippet } from "@/lib/ipc";

interface SearchResultsProps {
  hits: SearchHit[];
  query: string;
  /** Opens the note (App clears the search, so the session view takes over). */
  onOpen: (path: string) => void;
}

/**
 * How much of a long line survives around the first match: matches deep
 * inside an hour-long transcript line would otherwise be cut off by
 * truncation before the highlight is ever visible.
 */
const CONTEXT_CHARS = 60;

/**
 * Windows `text` around its first case-insensitive `query` occurrence and
 * splits it into plain/highlighted segments. Exported for tests.
 */
export function highlightSegments(
  text: string,
  query: string
): { text: string; match: boolean }[] {
  const needle = query.trim().toLowerCase();
  if (needle === "") return [{ text, match: false }];

  const lower = text.toLowerCase();
  const first = lower.indexOf(needle);
  if (first === -1) return [{ text, match: false }];

  // Window the line around the first match so the highlight is on screen.
  let start = 0;
  let windowed = text;
  let prefix = "";
  if (first > CONTEXT_CHARS) {
    start = first - CONTEXT_CHARS;
    windowed = text.slice(start);
    prefix = "…";
  }
  const windowedLower = lower.slice(start);

  const segments: { text: string; match: boolean }[] = [];
  if (prefix) segments.push({ text: prefix, match: false });
  let cursor = 0;
  for (;;) {
    const at = windowedLower.indexOf(needle, cursor);
    if (at === -1) break;
    if (at > cursor) segments.push({ text: windowed.slice(cursor, at), match: false });
    segments.push({ text: windowed.slice(at, at + needle.length), match: true });
    cursor = at + needle.length;
  }
  if (cursor < windowed.length) segments.push({ text: windowed.slice(cursor), match: false });
  return segments;
}

function Highlighted({ text, query }: { text: string; query: string }) {
  return (
    <>
      {highlightSegments(text, query).map((seg, i) =>
        seg.match ? (
          // Tokens, not <mark>'s UA default: browser yellow ignores the
          // dark theme entirely.
          <mark key={i} className="rounded-[3px] bg-primary/20 px-0.5 text-foreground">
            {seg.text}
          </mark>
        ) : (
          <span key={i}>{seg.text}</span>
        )
      )}
    </>
  );
}

function snippetIcon(section: SearchSnippet["section"]) {
  const cls = "mt-0.5 size-3.5 shrink-0 text-muted-foreground";
  switch (section) {
    case "title":
      return <FileText aria-hidden="true" className={cls} />;
    case "notes":
      return <StickyNote aria-hidden="true" className={cls} />;
    case "transcript":
      return <MessageSquareText aria-hidden="true" className={cls} />;
  }
}

function formatDate(dateStr: string): string {
  const d = new Date(dateStr);
  return d.toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
  });
}

/**
 * The main-pane search view (stage 0.10): one card per matching note with
 * its first matching lines, query occurrences highlighted, so a hit is
 * visible in context instead of being a bare sidebar row. Clicking a card
 * opens the note.
 */
export function SearchResults({ hits, query, onOpen }: SearchResultsProps) {
  return (
    <div className="flex h-full min-h-0 flex-col">
      <p className="pb-3 text-sm text-muted-foreground">
        {hits.length === 0
          ? `no matches for "${query}"`
          : `${hits.length} ${hits.length === 1 ? "note matches" : "notes match"} "${query}"`}
      </p>

      <div className="min-h-0 flex-1 space-y-2 overflow-y-auto pb-2">
        {hits.map((hit) => (
          <button
            key={hit.path}
            type="button"
            onClick={() => onOpen(hit.path)}
            className="block w-full rounded-lg border bg-card px-4 py-3 text-left transition-colors duration-150 hover:bg-foreground/[0.06] active:bg-foreground/[0.10]"
          >
            <div className="flex items-baseline justify-between gap-3">
              <span className="min-w-0 truncate text-sm font-medium">
                <Highlighted text={hit.title} query={query} />
              </span>
              <span className="shrink-0 text-xs text-muted-foreground">
                {formatDate(hit.date)}
              </span>
            </div>

            {hit.snippets.filter((s) => s.section !== "title").length > 0 && (
              <ul className="mt-2 space-y-1">
                {hit.snippets
                  .filter((s) => s.section !== "title")
                  .map((snippet, i) => (
                    <li key={i} className="flex items-start gap-2">
                      {snippetIcon(snippet.section)}
                      <span className="min-w-0 truncate text-xs text-muted-foreground">
                        <Highlighted text={snippet.text} query={query} />
                      </span>
                    </li>
                  ))}
              </ul>
            )}

            {hit.total_matches > hit.snippets.length && (
              <p className="mt-1.5 text-xs text-muted-foreground/70">
                {hit.total_matches} matches in this note
              </p>
            )}
          </button>
        ))}
      </div>
    </div>
  );
}
