import {
  Children,
  isValidElement,
  lazy,
  Suspense,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type MouseEvent,
  type ReactNode,
} from "react";
import type { Components } from "react-markdown";
import {
  Check,
  ChevronRight,
  CircleCheck,
  CircleQuestionMark,
  Hourglass,
  UserRound,
  type LucideIcon,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import {
  openLoopsSave,
  openLoopsToggle,
  type ChatSource,
  type OpenLoops,
} from "@/lib/ipc";

const MarkdownRenderer = lazy(() => import("@/components/MarkdownRenderer"));

/** Remembers whether Done is expanded; a per-viewer convenience only. */
const DONE_OPEN_KEY = "simstim.openLoops.doneOpen";

/** The four sections Simstim manages, by their fixed headings. */
const SECTION_KEYS: Record<string, "mine" | "waiting" | "questions" | "done"> = {
  mine: "mine",
  "waiting for": "waiting",
  "open questions": "questions",
  done: "done",
};

/**
 * One icon and tint per section, so the list scans like Reminders' smart
 * lists: who owes the next step, at a glance.
 */
const SECTION_LOOK: Record<"mine" | "waiting" | "questions" | "done", { icon: LucideIcon; tint: string }> = {
  mine: { icon: UserRound, tint: "text-primary" },
  waiting: { icon: Hourglass, tint: "text-loops-waiting" },
  questions: { icon: CircleQuestionMark, tint: "text-loops-question" },
  done: { icon: CircleCheck, tint: "text-success" },
};

interface Chunk {
  /** Heading text, `null` for anything above the first heading. */
  heading: string | null;
  section: "mine" | "waiting" | "questions" | "done" | null;
  text: string;
  /** 1-based file line of the chunk's first text line. */
  startLine: number;
}

/**
 * Splits the body at `#`/`##` headings (outside code fences), keeping each
 * chunk's file line so a rendered task maps back to its exact line.
 */
export function splitBody(body: string, bodyLine: number): Chunk[] {
  const chunks: Chunk[] = [];
  let current: Chunk & { lines: string[] } = {
    heading: null,
    section: null,
    text: "",
    startLine: bodyLine,
    lines: [],
  };
  let fenced = false;
  body.split("\n").forEach((line, index) => {
    if (line.trimStart().startsWith("```")) fenced = !fenced;
    const heading = !fenced && /^#{1,2} /.test(line) ? line.replace(/^#{1,2} /, "").trim() : null;
    if (heading === null) {
      current.lines.push(line);
      return;
    }
    chunks.push({ ...current, text: current.lines.join("\n") });
    current = {
      heading,
      section: SECTION_KEYS[heading.toLowerCase()] ?? null,
      text: "",
      startLine: bodyLine + index + 1,
      lines: [],
    };
  });
  chunks.push({ ...current, text: current.lines.join("\n") });
  return chunks
    .map(({ heading, section, text, startLine }) => ({ heading, section, text, startLine }))
    .filter((chunk) => chunk.heading !== null || chunk.text.trim() !== "");
}

/**
 * A link to a call file in the notes folder, as Simstim writes them
 * (`2026-09-09%2010-00-00%20call.md#t=00:00:16`). Anything else is an
 * ordinary link.
 */
export function callTarget(href: string | undefined): ChatSource | null {
  if (!href || /^[a-z][a-z0-9+.-]*:/i.test(href)) return null;
  const [path, fragment] = href.split("#", 2);
  let file: string;
  try {
    file = decodeURIComponent(path);
  } catch {
    return null;
  }
  if (!file.toLowerCase().endsWith(".md") || file.includes("/")) return null;
  const time = fragment?.startsWith("t=") ? fragment.slice(2) : null;
  return { file, timestamp: time && /^\d{2}:\d{2}:\d{2}$/.test(time) ? time : null };
}

function countTasks(text: string): number {
  return text.split("\n").filter((line) => /^[-*+] \[[ xX]\] /.test(line)).length;
}

/**
 * Splits an item's inline content at its first ` · `: the action stays in
 * ink, the date and source link go quiet on a line of their own. Nested
 * lists stay outside.
 */
function splitMeta(children: ReactNode): ReactNode {
  const parts = Children.toArray(children);
  const blocks = parts.filter((part) => isValidElement(part) && (part.type === "ul" || part.type === "ol"));
  const inline = parts.filter((part) => !blocks.includes(part));
  const index = inline.findIndex((part) => typeof part === "string" && part.includes(" · "));
  if (index === -1) {
    return (
      <>
        <span className="open-loop-action">{inline}</span>
        {blocks}
      </>
    );
  }
  const text = inline[index] as string;
  const at = text.indexOf(" · ");
  return (
    <>
      <span className="open-loop-action">
        {inline.slice(0, index)}
        {text.slice(0, at)}
      </span>
      <span className="open-loop-meta">
        {text.slice(at + " · ".length)}
        {inline.slice(index + 1)}
      </span>
      {blocks}
    </>
  );
}

function formatUpdated(iso: string): string | null {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return null;
  return date.toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

/** Done starts open: a checked item moves there and must stay in sight. */
function readDoneOpen(): boolean {
  try {
    return window.localStorage.getItem(DONE_OPEN_KEY) !== "false";
  } catch {
    return true;
  }
}

interface OpenLoopsViewProps {
  /** `null` until the first read. */
  loops: OpenLoops | null;
  /** An update is running or queued. */
  running: boolean;
  /** The running update's current step, if it reports one. */
  progress?: string | null;
  /** The last update's failure, until the next success. */
  error: string | null;
  /** A toggle or hand edit wrote the file; App keeps the copy. */
  onChange: (next: OpenLoops) => void;
  /** Update now, or the first build. */
  onUpdate: () => void;
  /** A link to a call was clicked; App owns the navigation. */
  onOpenCitation: (source: ChatSource) => void;
  /** Opens the file in the system editor. */
  onOpenFile: () => void;
}

/**
 * The Open loops file as a pane: its sections rendered with live checkboxes
 * and call links, a quiet provenance line, and the raw Markdown one click
 * away. The file stays the state; every action is a write to it.
 */
export function OpenLoopsView({
  loops,
  running,
  progress = null,
  error,
  onChange,
  onUpdate,
  onOpenCitation,
  onOpenFile,
}: OpenLoopsViewProps) {
  const [doneOpen, setDoneOpen] = useState(readDoneOpen);
  const [editing, setEditing] = useState<{ draft: string; base: string } | null>(null);
  const [conflict, setConflict] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    try {
      window.localStorage.setItem(DONE_OPEN_KEY, String(doneOpen));
    } catch {
      // Remembering the fold is a convenience.
    }
  }, [doneOpen]);

  useEffect(() => {
    if (editing && textareaRef.current && document.activeElement !== textareaRef.current) {
      textareaRef.current.focus();
    }
  }, [editing]);

  // The editor grows with its text so the pane keeps one scrollbar.
  const draft = editing?.draft;
  useLayoutEffect(() => {
    const textarea = textareaRef.current;
    if (!textarea || draft === undefined) return;
    textarea.style.height = "auto";
    textarea.style.height = `${textarea.scrollHeight}px`;
  }, [draft]);

  const bodyLines = loops?.body.split("\n") ?? [];
  const lineAt = (line: number) => bodyLines[line - (loops?.body_line ?? 1)] ?? "";

  async function toggle(line: number, expected: string, done: boolean) {
    setActionError(null);
    try {
      onChange(await openLoopsToggle(line, expected, done));
    } catch (err) {
      setActionError(String(err));
    }
  }

  function beginEditing(event: MouseEvent<HTMLDivElement>) {
    if (!loops || running) return;
    if ((event.target as HTMLElement).closest("a, button, input, summary")) return;
    if (window.getSelection()?.toString()) return;
    setActionError(null);
    setConflict(false);
    setEditing({ draft: loops.body, base: loops.body });
  }

  async function save(force: boolean) {
    if (!editing) return;
    if (!force && editing.draft === editing.base) {
      setEditing(null);
      return;
    }
    try {
      onChange(await openLoopsSave(editing.draft, force ? null : editing.base));
      setEditing(null);
      setConflict(false);
    } catch (err) {
      if (String(err).includes("changed while you were editing")) {
        setConflict(true);
      } else {
        setActionError(String(err));
      }
    }
  }

  function handleEditorKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (event.key === "Escape" || (event.key === "Enter" && (event.metaKey || event.ctrlKey))) {
      event.preventDefault();
      event.currentTarget.blur();
    }
  }

  function components(chunk: Chunk): Components {
    return {
      input({ node, ...props }) {
        void node;
        // Task checkboxes are drawn by `li` below, which knows the line.
        return props.type === "checkbox" ? null : <input {...props} />;
      },
      li({ node, className, children, ...props }) {
        if (!className?.includes("task-list-item")) {
          return (
            <li className={className} {...props}>
              {children}
            </li>
          );
        }
        const line = chunk.startLine + (node?.position?.start.line ?? 1) - 1;
        const raw = lineAt(line);
        const checked = /^\s*[-*+] \[[xX]\]/.test(raw);
        const label = raw.replace(/^\s*[-*+] \[[ xX]\] /, "").split(" · ")[0];
        return (
          <li className={cn(className, "open-loop")} data-checked={checked ? "true" : undefined}>
            {/* A drawn circle, not the native box: WebKit renders a checked
                native checkbox inconsistently across themes. */}
            <button
              type="button"
              role="checkbox"
              aria-checked={checked}
              aria-label={label}
              disabled={running || editing !== null}
              onClick={() => void toggle(line, raw, !checked)}
              className="open-loop-check"
            >
              {checked && <Check aria-hidden="true" strokeWidth={3} className="size-[10px]" />}
            </button>
            <span className="min-w-0 flex-1">{splitMeta(children)}</span>
          </li>
        );
      },
      a({ node, href, children, ...props }) {
        void node;
        const target = callTarget(href);
        if (!target) {
          return (
            <a href={href} target="_blank" rel="noreferrer" {...props}>
              {children}
            </a>
          );
        }
        return (
          <a
            href={href}
            title={target.timestamp ? `Open the call at ${target.timestamp}` : "Open the call"}
            onClick={(event) => {
              event.preventDefault();
              onOpenCitation(target);
            }}
          >
            {children}
          </a>
        );
      },
    };
  }

  const openCount = loops?.exists ? loops.open_count : 0;
  const updated = loops?.updated ? formatUpdated(loops.updated) : null;
  const chunks = loops?.exists ? splitBody(loops.body, loops.body_line) : [];
  const visible = chunks.filter((chunk) => chunk.text.trim() !== "" || chunk.section === null);
  const nothingOpen = loops?.exists && visible.every((chunk) => chunk.section === "done");

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-12 shrink-0 items-center gap-2.5 px-14">
        <h1 className="font-serif text-[15px] font-semibold">Open loops</h1>
        {openCount > 0 && (
          <span className="inline-flex h-[22px] items-center rounded-full bg-foreground/[0.06] px-2.5 text-[11px] tabular-nums text-quiet-foreground">
            {openCount} open
          </span>
        )}
      </div>

      {loops && !loops.exists ? (
        <div className="flex min-h-0 flex-1 flex-col items-center justify-center gap-5 px-14 pb-12">
          <p className="max-w-sm text-balance text-center font-serif text-[14px] leading-[1.6] text-muted-foreground">
            What you promised, what others promised you and the questions still open,
            gathered from your calls into one file beside your notes.
          </p>
          <Button
            type="button"
            onClick={onUpdate}
            disabled={running}
            className="h-9 rounded-[9px] px-4 text-[13px] font-semibold shadow-none"
          >
            {running && <span aria-hidden="true" className="working-spinner size-3" />}
            {running ? (progress ?? "Reading the last four weeks...") : "Build from the last four weeks"}
          </Button>
          {error && !running && (
            <p role="alert" className="max-w-sm text-center text-[12px] text-destructive">
              {error}
            </p>
          )}
        </div>
      ) : (
        <div className="min-h-0 flex-1 overflow-y-auto">
          <div className="flex w-full flex-col px-14 pb-12">
            {loops && (
              <p className="pb-3 text-[11px] text-muted-foreground">
                {running ? (
                  <span className="inline-flex items-center gap-1.5">
                    <span aria-hidden="true" className="working-spinner size-2.5" />
                    {progress ?? "Updating..."}
                  </span>
                ) : (
                  <>
                    {error ? (
                      <span className="text-destructive">Couldn't update: {error}</span>
                    ) : (
                      updated && <span>Updated {updated}</span>
                    )}
                    {(error || updated) && " · "}
                    <button
                      type="button"
                      onClick={onUpdate}
                      disabled={editing !== null}
                      title={
                        loops.unread > 0
                          ? "Read older calls that have no Open loops yet, one request each"
                          : "Merge the last four weeks of calls again"
                      }
                      className="underline-offset-2 hover:text-foreground hover:underline disabled:pointer-events-none disabled:opacity-50"
                    >
                      {error
                        ? "Try again"
                        : loops.unread > 0
                          ? `Read ${loops.unread} older ${loops.unread === 1 ? "call" : "calls"}`
                          : "Update now"}
                    </button>
                    {" · "}
                    <button
                      type="button"
                      onClick={onOpenFile}
                      className="underline-offset-2 hover:text-foreground hover:underline"
                    >
                      Open file
                    </button>
                  </>
                )}
              </p>
            )}

            {actionError && (
              <p role="alert" className="pb-3 text-[12px] text-destructive">
                {actionError}
              </p>
            )}

            {editing ? (
              <div className="flex flex-col gap-2">
                {conflict && (
                  <p role="alert" className="text-[12px] text-destructive">
                    The file changed while you were editing.{" "}
                    <button
                      type="button"
                      onMouseDown={(event) => event.preventDefault()}
                      onClick={() => void save(true)}
                      className="font-semibold underline underline-offset-2"
                    >
                      Keep mine
                    </button>
                    {" · "}
                    <button
                      type="button"
                      onMouseDown={(event) => event.preventDefault()}
                      onClick={() => {
                        setEditing(null);
                        setConflict(false);
                      }}
                      className="font-semibold underline underline-offset-2"
                    >
                      Use the file
                    </button>
                  </p>
                )}
                <textarea
                  ref={textareaRef}
                  aria-label="Open loops Markdown"
                  value={editing.draft}
                  onChange={(event) => setEditing({ ...editing, draft: event.target.value })}
                  onBlur={() => void save(false)}
                  onKeyDown={handleEditorKeyDown}
                  spellCheck={false}
                  className="w-full resize-none overflow-hidden border-0 bg-transparent p-0 font-serif text-[15px] leading-[1.65] text-card-foreground outline-none focus-visible:ring-0"
                />
              </div>
            ) : (
              <div className="open-loops cursor-text" onClick={beginEditing}>
                {nothingOpen && (
                  <p className="pb-6 pt-2 font-serif text-[14px] text-muted-foreground">
                    Nothing open. New promises and questions appear here after your calls.
                  </p>
                )}
                {visible.map((chunk) => {
                  const isDone = chunk.section === "done";
                  const body = (
                    <div className="markdown-content">
                      <Suspense fallback={null}>
                        <MarkdownRenderer source={chunk.text} components={components(chunk)} />
                      </Suspense>
                    </div>
                  );
                  return (
                    <section key={`${chunk.startLine}-${chunk.heading ?? ""}`} className="pb-5">
                      {/* Section headers match the sidebar's group headers:
                          sentence case, in ink, smaller than the page title,
                          led by the section's icon. Done is a divider below
                          the open sections. */}
                      {chunk.heading !== null &&
                        (isDone ? (
                          <div className="mb-2 mt-1 flex items-center gap-3">
                            <span aria-hidden="true" className="h-px flex-1 bg-border" />
                            <button
                              type="button"
                              aria-expanded={doneOpen}
                              onClick={() => setDoneOpen((open) => !open)}
                              className="flex items-center gap-1.5 rounded-[5px] px-1 text-[13px] font-semibold text-muted-foreground outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/45"
                            >
                              <CircleCheck aria-hidden="true" className="size-[14px] text-success" />
                              Done
                              <span className="text-[12px] font-normal tabular-nums">
                                {countTasks(chunk.text)}
                              </span>
                              <ChevronRight
                                aria-hidden="true"
                                className={cn("size-[13px] transition-transform", doneOpen && "rotate-90")}
                              />
                            </button>
                            <span aria-hidden="true" className="h-px flex-1 bg-border" />
                          </div>
                        ) : (
                          <h2 className="mb-1.5 flex items-center gap-1.5 text-[13px] font-semibold text-foreground">
                            {chunk.section &&
                              (() => {
                                const { icon: Icon, tint } = SECTION_LOOK[chunk.section];
                                return <Icon aria-hidden="true" className={cn("size-[14px] shrink-0", tint)} />;
                              })()}
                            {chunk.heading}
                          </h2>
                        ))}
                      {(!isDone || doneOpen) && body}
                    </section>
                  );
                })}
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
