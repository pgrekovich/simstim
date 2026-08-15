import { cn } from "@/lib/utils";
import type { SessionMeta } from "@/lib/ipc";

interface SidebarProps {
  sessions: SessionMeta[];
  selectedPath: string | null;
  onSelect: (path: string) => void;
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

export function Sidebar({ sessions, selectedPath, onSelect }: SidebarProps) {
  const groups = groupSessions(sessions);

  return (
    <aside className="flex h-full w-60 shrink-0 flex-col border-r bg-sidebar text-sidebar-foreground">
      <div className="px-4 py-3">
        <span className="font-mono text-sm lowercase text-muted-foreground">simstim</span>
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
              {group.sessions.map((session) => (
                <button
                  key={session.path}
                  type="button"
                  onClick={() => onSelect(session.path)}
                  className={cn(
                    "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm hover:bg-sidebar-accent",
                    selectedPath === session.path &&
                      "bg-sidebar-accent text-sidebar-accent-foreground"
                  )}
                >
                  <span className="min-w-0 flex-1 truncate">{session.title}</span>
                  <span className="shrink-0 text-xs text-muted-foreground">
                    {formatTime(session.date)}
                  </span>
                </button>
              ))}
            </div>
          ))
        )}
      </div>
    </aside>
  );
}
