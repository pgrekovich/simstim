import { FileText, Folder, MessageCircleQuestion, PanelLeftClose, Search, SquarePen, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import { cn, formatDuration } from "@/lib/utils";
import type { SessionMeta } from "@/lib/ipc";

interface SidebarProps {
  /** Pane width in px, owned (and persisted) by App's resize handling. */
  width?: number;
  sessions: SessionMeta[];
  selectedPath: string | null;
  /** The path currently being recorded, if any; shows a pulsing dot next to that row's title. */
  activePath: string | null;
  /**
   * Paths with machine work in flight (a re-transcribe or an enhance),
   * mapped to the lowercase status word shown under the title
   * ("transcribing" / "summarizing"). Owned by App, which watches both
   * backends' progress events; a row in this map shows the indeterminate
   * work spinner where the recording row shows its tally dot.
   */
  busyPaths?: Record<string, string>;
  onSelect: (path: string) => void;
  onNewSession: () => void;
  /** Hides the note list while preserving the selected note and recording. */
  onCollapse?: () => void;
  /** Opens the global search palette. */
  onOpenSearch?: () => void;
  /** Opens the Ask your notes surface (stage 4.2); the pinned row above the list. */
  onOpenAsk?: () => void;
  /** Whether the Ask your notes surface currently owns the main pane. */
  askActive?: boolean;
  /** Row-hover trash button; App owns the confirm dialog and the IPC call. */
  onDelete: (path: string) => void;
  /** Row context menu's "Open file": the note in the system editor. */
  onOpenFile?: (path: string) => void;
  /** Row context menu's "Show in Finder". */
  onRevealInFinder?: (path: string) => void;
}

interface SessionGroup {
  label: string;
  sessions: SessionMeta[];
}

function startOfDay(date: Date): Date {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate());
}

/** "Today" / "Yesterday" / a localized short date ("Aug 13"), by local calendar day. */
function dayLabel(dateStr: string, now: Date): string {
  const d = new Date(dateStr);
  const diffDays = Math.round(
    (startOfDay(now).getTime() - startOfDay(d).getTime()) / (24 * 60 * 60 * 1000)
  );
  if (diffDays === 0) return "Today";
  if (diffDays === 1) return "Yesterday";
  return d.toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

function formatTime(dateStr: string): string {
  const d = new Date(dateStr);
  const hh = d.getHours().toString().padStart(2, "0");
  const mm = d.getMinutes().toString().padStart(2, "0");
  return `${hh}:${mm}`;
}

/**
 * Groups sessions into day buckets in the order they arrive. `list_sessions`
 * already sorts by date desc, so a single pass merging into the trailing
 * group only when its label matches is enough -- no need to re-sort or
 * collect-then-group by key, since same-day sessions are always adjacent.
 */
function groupSessions(sessions: SessionMeta[]): SessionGroup[] {
  const now = new Date();
  const groups: SessionGroup[] = [];
  for (const session of sessions) {
    const label = dayLabel(session.date, now);
    const last = groups[groups.length - 1];
    if (last && last.label === label) {
      last.sessions.push(session);
    } else {
      groups.push({ label, sessions: [session] });
    }
  }
  return groups;
}

export function Sidebar({
  width = 280,
  sessions,
  selectedPath,
  activePath,
  busyPaths,
  onSelect,
  onNewSession,
  onCollapse,
  onOpenSearch = () => {},
  onOpenAsk,
  askActive = false,
  onDelete,
  onOpenFile,
  onRevealInFinder,
}: SidebarProps) {
  const groups = groupSessions(sessions);

  return (
    <aside
      style={{ width }}
      className="flex h-full shrink-0 flex-col border-r border-sidebar-border bg-sidebar text-sidebar-foreground"
    >
      {/* The window's title bar is an overlay (tauri.conf.json
          `titleBarStyle: Overlay`), so the sidebar background runs to the
          very top and the traffic lights float over this strip. It doubles
          as the drag handle for that region. */}
      <div data-tauri-drag-region className="flex h-14 shrink-0 items-center justify-start gap-0.5 pl-[88px] pr-2.5">
        <Button
          variant="ghost"
          size="icon"
          className="size-6 rounded-md text-muted-foreground hover:bg-foreground/[0.07] hover:text-foreground active:bg-foreground/[0.14] dark:text-[#8B8578]"
          aria-label="Collapse sidebar"
          title="Collapse sidebar (⌘\\)"
          onClick={onCollapse}
        >
          <PanelLeftClose className="size-[15px]" />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          className="size-6 rounded-md text-muted-foreground hover:bg-foreground/[0.07] hover:text-foreground active:bg-foreground/[0.14] dark:text-[#8B8578]"
          aria-label="Search all notes"
          title="Search (⌘K)"
          onClick={onOpenSearch}
        >
          <Search className="size-[15px]" />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          className="size-6 rounded-md text-muted-foreground hover:bg-foreground/[0.07] hover:text-foreground active:bg-foreground/[0.14] dark:text-[#8B8578]"
          aria-label="New note"
          title="New note"
          onClick={onNewSession}
        >
          <SquarePen className="size-[15px]" />
        </Button>
      </div>

      {/* Pinned above the list, not inside it (owner ruling 2026-09-04):
          the surface's one entry point must not scroll away with a long
          note history. */}
      {onOpenAsk && (
        <div className="shrink-0 px-3">
          <button
            type="button"
            onClick={onOpenAsk}
            className={cn(
              "mt-1.5 flex h-[30px] w-full items-center gap-2 rounded-[7px] border px-2.5 text-left",
              askActive
                ? "border-[var(--voice-remote)]/55 bg-background text-foreground shadow-[0_1px_2px_rgb(30_27_22/0.06)]"
                : "border-foreground/[0.11] bg-background text-quiet-foreground hover:border-foreground/[0.2] hover:bg-card hover:text-foreground active:bg-foreground/[0.06]",
            )}
          >
            <MessageCircleQuestion
              aria-hidden="true"
              className="size-[14px] shrink-0 text-[var(--voice-remote)]"
            />
            <span className="min-w-0 flex-1 truncate text-[12.5px] font-medium">Ask your notes</span>
            {/* Below ~205px the label itself needs the room: at the pane's
                180px minimum the badge was what pushed "Ask your notes" into
                an ellipsis, and the words beat the shortcut hint. */}
            {width >= 205 && (
              <span className="inline-flex h-[17px] shrink-0 items-center rounded-[4px] bg-foreground/[0.07] px-[5px] font-mono text-[10.5px] text-quiet-foreground">
                ⌘L
              </span>
            )}
          </button>
        </div>
      )}

      <div className="min-h-0 flex-1 overflow-y-auto px-3 pb-3">
        {sessions.length === 0 ? (
          <p className="px-2 py-4 text-center text-xs text-muted-foreground">
            no sessions yet
          </p>
        ) : (
          groups.map((group) => (
            <div key={group.label} className="flex flex-col gap-px">
              <div className="px-2 pb-2 pt-[14px] font-mono text-[10px] font-medium uppercase tracking-[0.08em] text-muted-foreground dark:text-[#7A7466]">
                {group.label}
              </div>
              {group.sessions.map((session) => {
                const selected = selectedPath === session.path;
                const recording = activePath === session.path;
                // Recording wins over a (theoretically simultaneous) busy
                // word: red tally is the loudest state this list has.
                const busyWord = recording ? undefined : busyPaths?.[session.path];
                const duration = formatDuration(session.duration_secs);
                return (
                  // A wrapper div, not one button: the delete affordance is
                  // its own overlaid button (nesting buttons is invalid
                  // HTML), revealed on row hover in place of the timestamp.
                  // The whole row also answers a right click (macOS reflex)
                  // with the same actions the toolbar overflow menu offers
                  // for the OPEN note, so any listed note is one gesture
                  // from its file.
                  <ContextMenu key={session.path}>
                    <ContextMenuTrigger asChild>
                  <div className="group relative">
                    <button
                      type="button"
                      onClick={() => onSelect(session.path)}
                      className={cn(
                        "h-auto w-full rounded-[7px] px-2.5 py-2 text-left font-normal",
                        selected
                          ? "bg-background text-foreground shadow-[0_1px_2px_rgb(30_27_22/0.07)] hover:bg-background/80"
                          : "text-quiet-foreground hover:bg-foreground/[0.05] hover:text-foreground active:bg-foreground/[0.10]",
                        recording
                          ? "flex items-center gap-2 bg-background shadow-[0_1px_2px_rgb(30_27_22/0.07)] dark:border dark:border-destructive/[0.28] dark:bg-destructive/[0.14]"
                          : busyWord
                            ? "flex items-center gap-2"
                            : "flex flex-col gap-px"
                      )}
                    >
                      {recording ? (
                        <>
                          <span
                            data-testid="recording-dot"
                            aria-hidden="true"
                            className="recording-tally size-1.5 shrink-0 rounded-full bg-destructive"
                          />
                          <span className="flex min-w-0 flex-1 flex-col gap-0.5">
                            <span className="truncate text-[12.5px] font-medium text-foreground">{session.title}</span>
                            <span className="font-mono text-[10.5px] text-destructive">recording</span>
                          </span>
                        </>
                      ) : busyWord ? (
                        <>
                          {/* Machine work, not capture: the quiet ink work
                              spinner where the recording row has its red
                              tally, and the status word in muted mono. */}
                          <span
                            data-testid="working-dot"
                            aria-hidden="true"
                            className="working-spinner size-3 shrink-0 text-muted-foreground"
                          />
                          <span className="flex min-w-0 flex-1 flex-col gap-0.5">
                            <span className="truncate text-[12.5px] text-inherit">{session.title}</span>
                            <span className="font-mono text-[10.5px] text-muted-foreground">{busyWord}</span>
                          </span>
                        </>
                      ) : (
                        <>
                          <span className={cn("w-full truncate pr-6 text-[12.5px]", selected && "font-medium")}>{session.title}</span>
                          <span className="font-mono text-[10.5px] text-muted-foreground dark:text-[#7A7466]">
                            {formatTime(session.date)}
                            {duration && ` · ${duration}`}
                          </span>
                        </>
                      )}
                    </button>
                    {!recording && !busyWord && (
                      <button
                        type="button"
                        // The accessible name deliberately does NOT include
                        // the session title: row queries (tests and screen
                        // readers alike) match rows by title, and a delete
                        // button carrying the same words would collide with
                        // its own row (14 existing queries broke that way).
                        aria-label="Move to Trash"
                        title="Move to Trash"
                        onClick={() => onDelete(session.path)}
                        // pointer-events gating, not just opacity: an
                        // invisible-but-clickable overlay would steal clicks
                        // aimed at the timestamp area of the row button.
                        className="pointer-events-none absolute right-1.5 top-1.5 rounded p-1 text-muted-foreground opacity-0 hover:text-destructive focus-visible:pointer-events-auto focus-visible:opacity-100 group-hover:pointer-events-auto group-hover:opacity-100"
                      >
                        <Trash2 className="size-3.5" />
                      </button>
                    )}
                  </div>
                    </ContextMenuTrigger>
                    <ContextMenuContent>
                      <ContextMenuItem onSelect={() => onOpenFile?.(session.path)}>
                        <FileText />
                        Open file
                      </ContextMenuItem>
                      <ContextMenuItem onSelect={() => onRevealInFinder?.(session.path)}>
                        <Folder />
                        Show in Finder
                      </ContextMenuItem>
                      <ContextMenuSeparator />
                      {/* Same gate as the hover trash button: the recording
                          session and one with machine work in flight can't
                          be deleted (the backend rejects both). */}
                      <ContextMenuItem
                        disabled={recording || Boolean(busyWord)}
                        onSelect={() => onDelete(session.path)}
                      >
                        <Trash2 />
                        Move to Trash
                      </ContextMenuItem>
                    </ContextMenuContent>
                  </ContextMenu>
                );
              })}
            </div>
          ))
        )}
      </div>
    </aside>
  );
}
