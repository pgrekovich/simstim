import { MessageCircleQuestion, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { ChatConversation, type ChatAskRequest } from "@/components/Chat";
import type { ChatSource, SessionMeta } from "@/lib/ipc";
import { quickQuestionsFor, usePrompts } from "@/lib/prompts";

interface AskCallDrawerProps {
  /** Pane width in px, owned (and persisted) by App's resize handling. */
  width?: number;
  /** The open note this drawer converses with (its chat scope). */
  path: string;
  sessions: SessionMeta[];
  /**
   * Milliseconds epoch when the current recording started, `null` when this
   * note isn't being recorded. Stamps questions with the recording clock
   * ("You asked · 00:15:04", design 8c).
   */
  recordingStartedAt: number | null;
  /** A question handed in from the ⌘K palette's "Ask this call" row. */
  askRequest?: ChatAskRequest | null;
  onClose: () => void;
  /** A citation chip was clicked; App owns the navigation. */
  onOpenCitation: (source: ChatSource) => void;
}

/** Elapsed `hh:mm:ss` since `startedAt`, the same clock the Stop button shows. */
export function recordingClock(startedAt: number, now = Date.now()): string {
  const total = Math.max(0, Math.floor((now - startedAt) / 1000));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  return [h, m, s].map((n) => String(n).padStart(2, "0")).join(":");
}

/**
 * Ask this call (stage 4.2, design 8c): the 300px right drawer conversing
 * with the open note -- past calls and the one being recorded right now
 * alike (the backend snapshots the note's on-disk transcript per ask; chat
 * is CLI-only, so a mid-call ask never touches the live pipeline).
 */
export function AskCallDrawer({
  width = 300,
  path,
  sessions,
  recordingStartedAt,
  askRequest = null,
  onClose,
  onOpenCitation,
}: AskCallDrawerProps) {
  const { prompts } = usePrompts();
  return (
    <aside
      aria-label="Ask this call"
      style={{ width }}
      className="flex h-full shrink-0 flex-col border-l border-foreground/[0.12] bg-sidebar"
    >
      <div className="flex h-14 shrink-0 items-center justify-between border-b border-foreground/[0.09] pl-[18px] pr-3">
        <div className="flex items-center gap-2">
          <MessageCircleQuestion aria-hidden="true" className="size-[14px] text-quiet-foreground" />
          <h2 className="text-[12.5px] font-semibold">Ask this call</h2>
        </div>
        <Button
          variant="ghost"
          size="icon"
          className="size-6 rounded-md text-muted-foreground"
          aria-label="Close"
          title="Close (⌘J)"
          onClick={onClose}
        >
          <X className="size-[14px]" />
        </Button>
      </div>
      <ChatConversation
        scopePath={path}
        variant="drawer"
        sessions={sessions}
        placeholder="Ask about this call..."
        askRequest={askRequest}
        quickQuestions={quickQuestionsFor(prompts, "call")}
        askedAt={() =>
          recordingStartedAt !== null ? recordingClock(recordingStartedAt) : null
        }
        onOpenCitation={onOpenCitation}
        emptyState={
          <p className="pt-6 text-center font-serif text-[13px] leading-[1.6] text-muted-foreground">
            {recordingStartedAt !== null
              ? "Catch up without stopping the call: what was decided, what you missed, what to have ready."
              : "Ask what was agreed, promised or numbered in this call."}
          </p>
        }
      />
    </aside>
  );
}
