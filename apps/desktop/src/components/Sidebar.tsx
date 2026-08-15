import { Plus } from "lucide-react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import type { SessionMeta } from "@/lib/ipc";

interface SidebarProps {
  sessions: SessionMeta[];
  selectedPath: string | null;
  /** The path currently being recorded, if any; shows a pulsing dot next to that row's title. */
  activePath: string | null;
  onSelect: (path: string) => void;
  onNewSession: () => void;
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
  sessions,
  selectedPath,
  activePath,
  onSelect,
  onNewSession,
}: SidebarProps) {
  const groups = groupSessions(sessions);

  return (
    <aside className="flex h-full w-60 shrink-0 flex-col border-r bg-sidebar text-sidebar-foreground">
      <div className="border-b border-sidebar-border px-2 py-2">
        <Button
          variant="ghost"
          className="w-full justify-start gap-2 hover:bg-sidebar-accent hover:text-sidebar-accent-foreground"
          onClick={onNewSession}
        >
          <Plus className="size-4" />
          New note
        </Button>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto px-2 pb-3">
        {sessions.length === 0 ? (
          <p className="px-2 py-4 text-center text-xs text-muted-foreground">
            no sessions yet
          </p>
        ) : (
          groups.map((group) => (
            <div key={group.label} className="mb-2">
              <div className="px-2 py-1 text-xs font-medium text-muted-foreground">
                {group.label}
              </div>
              {group.sessions.map((session) => {
                const selected = selectedPath === session.path;
                const recording = activePath === session.path;
                return (
                  <button
                    key={session.path}
                    type="button"
                    onClick={() => onSelect(session.path)}
                    className={cn(
                      "flex w-full cursor-pointer items-center gap-2 rounded-md border-l-2 border-transparent px-2 py-1.5 text-left text-sm transition-colors",
                      // The hover tint and the selected background must not
                      // both apply at once: a bare `hover:bg-sidebar-accent/50`
                      // alongside the selected row's full-opacity
                      // `bg-sidebar-accent` would still win on hover (later in
                      // the cascade, matching twMerge's same-bucket dedupe),
                      // visibly *dimming* the selected row instead of
                      // reinforcing it. So the selected branch gets its own
                      // same-color hover instead, keeping full opacity.
                      selected
                        ? "border-primary bg-sidebar-accent font-medium text-sidebar-accent-foreground hover:bg-sidebar-accent"
                        : "hover:bg-sidebar-accent/50"
                    )}
                  >
                    <span className="flex min-w-0 flex-1 items-center gap-1.5">
                      {recording && (
                        <span
                          data-testid="recording-dot"
                          aria-hidden="true"
                          className="size-1.5 shrink-0 animate-pulse rounded-full bg-destructive"
                        />
                      )}
                      <span className="min-w-0 truncate">{session.title}</span>
                      {/* Appended AFTER the title text so it doesn't become
                          the row button's primary accessible name (App.test.tsx
                          matches the active row by /^untitled/i). */}
                      {recording && <span className="sr-only">recording</span>}
                    </span>
                    <span className="shrink-0 text-xs text-muted-foreground">
                      {formatTime(session.date)}
                    </span>
                  </button>
                );
              })}
            </div>
          ))
        )}
      </div>
    </aside>
  );
}
