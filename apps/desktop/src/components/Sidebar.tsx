import { Search, Settings, SquarePen, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";
import type { SessionMeta } from "@/lib/ipc";

interface SidebarProps {
  sessions: SessionMeta[];
  selectedPath: string | null;
  /** The path currently being recorded, if any; shows a pulsing dot next to that row's title. */
  activePath: string | null;
  onSelect: (path: string) => void;
  onNewSession: () => void;
  /** Row-hover trash button; App owns the confirm dialog and the IPC call. */
  onDelete: (path: string) => void;
  /** Current search text; `sessions` is already the filtered result for it. */
  searchQuery: string;
  /** Keystroke-level; App debounces the actual `searchSessions` IPC call. */
  onSearchChange: (query: string) => void;
  /** Footer Settings button; App owns the dialog. */
  onOpenSettings: () => void;
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
  sessions,
  selectedPath,
  activePath,
  onSelect,
  onNewSession,
  onDelete,
  searchQuery,
  onSearchChange,
  onOpenSettings,
}: SidebarProps) {
  const groups = groupSessions(sessions);
  const searching = searchQuery.trim() !== "";

  return (
    <aside className="flex h-full w-56 shrink-0 flex-col border-r border-sidebar-border bg-sidebar text-sidebar-foreground">
      {/* The window's title bar is an overlay (tauri.conf.json
          `titleBarStyle: Overlay`), so the sidebar background runs to the
          very top and the traffic lights float over this strip. It doubles
          as the drag handle for that region. */}
      <div data-tauri-drag-region className="flex h-14 shrink-0 items-center justify-end gap-0.5 px-2.5">
        <Button
          variant="ghost"
          size="icon"
          className="size-6 rounded-md text-muted-foreground hover:bg-foreground/[0.07] hover:text-foreground active:bg-foreground/[0.14] dark:text-[#8B8578]"
          aria-label="Focus search"
          title="Search"
          onClick={() => document.getElementById("session-search")?.focus()}
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

      <div className="px-3 pt-1.5">
        <div className="relative">
          <Search
            aria-hidden="true"
            className="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground"
          />
          <Input
            id="session-search"
            type="search"
            placeholder="Search"
            aria-label="Search sessions"
            value={searchQuery}
            onChange={(e) => onSearchChange(e.target.value)}
            // h-8 + alpha fill AND a real border: fill alone washed out on
            // the sidebar surface (owner contrast review). Same
            // alpha-over-foreground trick as the row hovers so one value
            // reads correctly in both themes.
            className="h-[30px] rounded-[7px] border-sidebar-border bg-background pl-8 pr-2 text-[12.5px] shadow-[0_1px_2px_rgb(30_27_22/0.04)] placeholder:text-muted-foreground focus-visible:border-foreground/25 focus-visible:bg-background"
          />
        </div>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto px-3 pb-3">
        {sessions.length === 0 ? (
          <p className="px-2 py-4 text-center text-xs text-muted-foreground">
            {searching ? "no matches" : "no sessions yet"}
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
                const duration = formatDuration(session.duration_secs);
                return (
                  // A wrapper div, not one button: the delete affordance is
                  // its own overlaid button (nesting buttons is invalid
                  // HTML), revealed on row hover in place of the timestamp.
                  <div key={session.path} className="group relative">
                    <button
                      type="button"
                      onClick={() => onSelect(session.path)}
                      className={cn(
                        "h-auto w-full rounded-[7px] px-2.5 py-2 text-left font-normal",
                        selected
                          ? "bg-background text-foreground shadow-[0_1px_2px_rgb(30_27_22/0.07)] hover:bg-background/80"
                          : "text-foreground/75 hover:bg-foreground/[0.05] hover:text-foreground active:bg-foreground/[0.10]",
                        recording
                          ? "flex items-center gap-2 bg-background shadow-[0_1px_2px_rgb(30_27_22/0.07)] dark:border dark:border-destructive/[0.28] dark:bg-destructive/[0.14]"
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
                    {!recording && (
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
                );
              })}
            </div>
          ))
        )}
      </div>

      <div className="border-t border-sidebar-border px-3 py-2.5">
        <Button
          variant="ghost"
          className="h-8 w-full justify-start gap-2 rounded-[7px] px-2 text-[12.5px] text-muted-foreground transition-colors duration-150 hover:bg-foreground/[0.08] hover:text-foreground active:bg-foreground/[0.15]"
          onClick={onOpenSettings}
        >
          <Settings className="size-4" />
          Settings
        </Button>
      </div>
    </aside>
  );
}
