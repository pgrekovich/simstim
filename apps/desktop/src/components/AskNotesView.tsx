import { useEffect, useState } from "react";
import { Eraser } from "lucide-react";
import { Button } from "@/components/ui/button";
import { ChatConversation, type ChatAskRequest } from "@/components/Chat";
import {
  chatClear,
  storageStats,
  type ChatSource,
  type SessionMeta,
} from "@/lib/ipc";
import { quickQuestionsFor, usePrompts } from "@/lib/prompts";

interface AskNotesViewProps {
  sessions: SessionMeta[];
  /** A question handed in from the ⌘K palette, sent on arrival. */
  askRequest?: ChatAskRequest | null;
  /** A citation chip was clicked; App owns the navigation. */
  onOpenCitation: (source: ChatSource) => void;
}

/** "61 h" / "45 min" for the header badge, from summed audio seconds. */
export function formatHours(audioSecs: number): string {
  if (audioSecs >= 3600) return `${Math.round(audioSecs / 3600)} h`;
  return `${Math.max(1, Math.round(audioSecs / 60))} min`;
}

/**
 * Ask your notes (stage 4.2, design 8b): the full-pane conversation over the
 * whole notes folder. The conversation itself lives in the backend's
 * ChatState; this surface adds the header (badge from `storage_stats`,
 * clear-conversation) and the folder-scope wiring.
 */
export function AskNotesView({ sessions, askRequest, onOpenCitation }: AskNotesViewProps) {
  const [badge, setBadge] = useState<string | null>(null);
  // Remounts the conversation after a clear so it re-seeds from the (now
  // empty) backend history -- the conversation is backend-owned state.
  const [clearTick, setClearTick] = useState(0);
  const { prompts } = usePrompts();

  useEffect(() => {
    let cancelled = false;
    storageStats()
      .then((stats) => {
        if (cancelled) return;
        const notes = `${stats.note_count} ${stats.note_count === 1 ? "note" : "notes"}`;
        setBadge(stats.audio_secs > 0 ? `${notes} · ${formatHours(stats.audio_secs)}` : notes);
      })
      .catch(() => {
        // The badge is decoration; asking works without it.
      });
    return () => {
      cancelled = true;
    };
  }, []);

  async function handleClear() {
    try {
      await chatClear(null);
      setClearTick((tick) => tick + 1);
    } catch {
      // Rejected while an ask is in flight; the running box explains itself.
    }
  }

  const noteCount = badge?.split(" · ")[0];

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-12 shrink-0 items-center justify-between gap-4 px-14">
        <div className="flex items-center gap-2.5">
          <h1 className="font-serif text-[15px] font-semibold">Ask your notes</h1>
          {badge && (
            <span className="inline-flex h-[22px] items-center rounded-full bg-foreground/[0.06] px-2.5 text-[11px] tabular-nums text-quiet-foreground">
              {badge}
            </span>
          )}
        </div>
        <Button
          variant="ghost"
          size="icon"
          className="size-6 rounded-md text-muted-foreground"
          aria-label="Clear conversation"
          title="Clear conversation"
          onClick={() => void handleClear()}
        >
          <Eraser className="size-[14px]" />
        </Button>
      </div>
      <ChatConversation
        key={clearTick}
        scopePath={null}
        variant="pane"
        sessions={sessions}
        placeholder={noteCount ? `Ask anything about ${noteCount}...` : "Ask anything about your notes..."}
        askRequest={askRequest}
        quickQuestions={quickQuestionsFor(prompts, "notes")}
        onOpenCitation={onOpenCitation}
        emptyState={
          <div className="flex h-full items-center justify-center">
            <p className="max-w-sm text-center font-serif text-[14px] leading-[1.6] text-muted-foreground">
              Ask across every call you've recorded: decisions, promises,
              numbers, who said what. Answers cite the note and the second.
            </p>
          </div>
        }
      />
    </div>
  );
}
