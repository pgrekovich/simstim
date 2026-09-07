import {
  useEffect,
  useRef,
  useState,
  type KeyboardEvent,
} from "react";
import { listen } from "@tauri-apps/api/event";
import { CornerDownLeft, Square, Zap } from "lucide-react";
import { MarkdownContent } from "@/components/Markdown";
import { cn } from "@/lib/utils";
import {
  chatAsk,
  chatCancel,
  chatHistory,
  type ChatMessage,
  type ChatProgress,
  type ChatSource,
  type SessionMeta,
  type QuickQuestion,
} from "@/lib/ipc";

/**
 * One question the surface's owner wants sent programmatically (the ⌘K
 * palette's "Ask across all notes" handoff, a followup chip from outside).
 * The `id` makes repeats of the same text distinct, same recipe as
 * `TranscriptFindRequest`.
 */
export interface ChatAskRequest {
  id: number;
  question: string;
}

interface ChatConversationProps {
  /** `null` asks the whole folder; a session path asks that note. */
  scopePath: string | null;
  /** Sizing/typography: the full-pane surface vs the 300px drawer. */
  variant: "pane" | "drawer";
  /** For labeling source chips with note titles. */
  sessions: SessionMeta[];
  placeholder: string;
  /**
   * A citation chip was clicked. The component resolves the cited file to a
   * known session where possible; the parent owns the navigation itself.
   */
  onOpenCitation: (source: ChatSource) => void;
  /**
   * Supplies the display-only "asked at" stamp for a question (the recording
   * clock, for a mid-call ask). Absent for surfaces with no such clock.
   */
  askedAt?: () => string | null;
  /** A question handed in from outside (⌘K), sent as soon as it arrives. */
  askRequest?: ChatAskRequest | null;
  /**
   * The user's quick questions for this scope (prompts.toml): offered as
   * chips while the conversation is empty, and through the composer's
   * "/" menu and lightning button at any time. Sent as their prompt text,
   * shown by their name.
   */
  quickQuestions?: QuickQuestion[];
  /** Rendered when the conversation is still empty (the quiet hint). */
  emptyState?: React.ReactNode;
}

/** "00:04:30" -> "04:30" for chip labels; hour-long timestamps stay whole. */
export function shortTimestamp(timestamp: string): string {
  return timestamp.startsWith("00:") ? timestamp.slice(3) : timestamp;
}

/**
 * The label for a source chip: the cited note's sidebar title when the file
 * resolves to a known session, the raw file name otherwise, and the
 * mockups' "↩ hh:mm:ss" shape for a bare timestamp (the current call).
 */
export function sourceChipLabel(source: ChatSource, sessions: SessionMeta[]): string {
  if (source.file === null) {
    return source.timestamp ? `↩ ${source.timestamp}` : "";
  }
  const fileName = source.file;
  const session = sessions.find((s) => s.path.endsWith(`/${fileName}`) || s.path === fileName);
  const title = session?.title ?? fileName.replace(/\.md$/, "");
  return source.timestamp ? `${title} · ${shortTimestamp(source.timestamp)}` : title;
}

/** Distinct cited notes, for the answer box's "FROM N NOTES" label. */
export function distinctSourceCount(sources: ChatSource[]): number {
  const files = new Set<string>();
  let currentCall = false;
  for (const source of sources) {
    if (source.file === null) {
      currentCall = true;
    } else {
      files.add(source.file);
    }
  }
  return files.size + (currentCall ? 1 : 0);
}

/**
 * One conversation surface (stage 4.2), shared by Ask your notes (pane) and
 * Ask this call (drawer). The BACKEND owns the conversation (`ChatState`,
 * in-memory, per scope); this component is a projection of it: it seeds
 * from `chat_history` on mount, sends questions with `chat_ask`, and
 * refetches whenever the scope's `chat-progress` reports a terminal phase --
 * so a remount (switching sessions, closing and reopening the surface, a
 * webview reload) can never lose the conversation or an in-flight ask.
 *
 * A failed ask restores the question into the input for a retry; a
 * cancelled one ("cancelled" from `chat_cancel`) does the same but shows no
 * error -- the user just stopped it.
 */
export function ChatConversation({
  scopePath,
  variant,
  sessions,
  placeholder,
  onOpenCitation,
  askedAt,
  askRequest,
  emptyState,
  quickQuestions = [],
}: ChatConversationProps) {
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [running, setRunning] = useState(false);
  // The in-flight question's text, known only when THIS mounted instance
  // sent it (a remount mid-ask still shows the thinking box, just without
  // the question text -- the backend only stores the exchange on success).
  const [pendingQuestion, setPendingQuestion] = useState<string | null>(null);
  const [pendingAskedAt, setPendingAskedAt] = useState<string | null>(null);
  // The in-flight quick question's name (shown instead of its prompt).
  const [pendingName, setPendingName] = useState<string | null>(null);
  // The "/" menu's keyboard cursor, reset whenever the filter changes.
  const [menuIndex, setMenuIndex] = useState(0);
  // Mirror of `pendingQuestion` for the event listener: restoring a failed
  // question into the input must not happen inside a state updater (updaters
  // must be pure), so the handler reads the current value from here instead.
  const pendingQuestionRef = useRef<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const handledAskRequestRef = useRef(0);
  // The scope string `chat-progress` events carry for this conversation.
  const scope = scopePath ?? "all";

  // Opening the surface means the user wants to type: focus immediately
  // (the rAF matches GlobalSearch -- the element must be laid out first).
  useEffect(() => {
    const frame = window.requestAnimationFrame(() => inputRef.current?.focus());
    return () => window.cancelAnimationFrame(frame);
  }, [scopePath]);

  useEffect(() => {
    let cancelled = false;
    chatHistory(scopePath)
      .then((history) => {
        if (cancelled) return;
        setMessages(history.messages);
        setRunning(history.running);
      })
      .catch(() => {
        // A failed seed just leaves an empty conversation; asking still works.
      });
    return () => {
      cancelled = true;
    };
  }, [scopePath]);

  // The event stream is the source of truth for lifecycle: `running` claims,
  // exactly one terminal `done`/`error` per ask (see chat.rs). On a terminal
  // phase the conversation is refetched rather than patched locally.
  useEffect(() => {
    const unlisten = listen<ChatProgress>("chat-progress", (event) => {
      const payload = event.payload;
      if (payload.scope !== scope) return;
      if (payload.phase === "running") {
        setRunning(true);
        return;
      }
      setRunning(false);
      if (payload.phase === "error") {
        // Put the failed question back in the input for a retry; a cancel
        // ("cancelled") restores it quietly, without the error box.
        const question = pendingQuestionRef.current;
        if (question !== null) setDraft((current) => (current === "" ? question : current));
        pendingQuestionRef.current = null;
        setPendingQuestion(null);
        setPendingName(null);
        setPendingAskedAt(null);
        if (payload.message !== "cancelled") {
          setError(payload.message ?? "chat failed");
        }
        return;
      }
      // "done": the stored conversation now carries the full exchange.
      pendingQuestionRef.current = null;
      setPendingQuestion(null);
      setPendingName(null);
      setPendingAskedAt(null);
      setError(null);
      chatHistory(scopePath)
        .then((history) => {
          setMessages(history.messages);
          setRunning(history.running);
        })
        .catch(() => {});
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, [scope, scopePath]);

  // Keep the newest exchange in view as messages and the thinking box land.
  useEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [messages, running, pendingQuestion]);

  function send(question: string, name?: string) {
    const trimmed = question.trim();
    if (trimmed === "" || running) return;
    const stamp = askedAt?.() ?? null;
    pendingQuestionRef.current = name ? null : trimmed;
    setPendingQuestion(trimmed);
    setPendingName(name ?? null);
    setPendingAskedAt(stamp);
    setError(null);
    setDraft("");
    // Fire and forget: `chat-progress` events drive everything visible, and
    // the rejection is surfaced by the same "error" event (chat.rs emits
    // exactly one terminal event per ask, validation failures included).
    chatAsk(scopePath, trimmed, stamp ?? undefined, name).catch(() => {});
  }

  /** A quick question: its prompt is the question, its name is what shows. */
  function sendQuick(question: QuickQuestion) {
    send(question.prompt, question.name);
  }

  // The composer's "/" menu: type "/" (or press the lightning button) and
  // the quick questions list opens above the input, filtered by what
  // follows the slash. Scales to many questions where chips would not.
  const slashFilter = draft.startsWith("/") ? draft.slice(1).trim().toLowerCase() : null;
  const menuItems =
    slashFilter === null
      ? []
      : quickQuestions.filter((q) => slashFilter === "" || q.name.toLowerCase().includes(slashFilter));
  const menuOpen = slashFilter !== null && quickQuestions.length > 0;
  useEffect(() => {
    setMenuIndex(0);
  }, [slashFilter]);

  function openQuickMenu() {
    setDraft("/");
    inputRef.current?.focus();
  }

  // A question handed in from outside (the ⌘K palette handoff).
  useEffect(() => {
    if (!askRequest || handledAskRequestRef.current === askRequest.id) return;
    handledAskRequestRef.current = askRequest.id;
    send(askRequest.question);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [askRequest]);

  function handleKeyDown(event: KeyboardEvent<HTMLInputElement>) {
    if (menuOpen) {
      if (event.key === "ArrowDown") {
        event.preventDefault();
        setMenuIndex((i) => Math.min(i + 1, Math.max(menuItems.length - 1, 0)));
        return;
      }
      if (event.key === "ArrowUp") {
        event.preventDefault();
        setMenuIndex((i) => Math.max(i - 1, 0));
        return;
      }
      if (event.key === "Escape") {
        event.preventDefault();
        setDraft("");
        return;
      }
      if (event.key === "Enter") {
        event.preventDefault();
        const picked = menuItems[menuIndex];
        if (picked) sendQuick(picked);
        return;
      }
    }
    if (event.key === "Enter") {
      event.preventDefault();
      send(draft);
    }
  }

  const pane = variant === "pane";

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div
        ref={scrollRef}
        className={cn(
          "min-h-0 flex-1 overflow-y-auto",
          pane ? "px-14 pb-5 pt-2" : "px-[18px] pb-4 pt-4",
        )}
      >
        {messages.length === 0 && pendingQuestion === null && !running && error === null ? (
          <>
            {emptyState ?? null}
            {quickQuestions.length > 0 && (
              // The first few quick questions as chips, the rest one "/" away:
              // a long list must never turn the empty state into a wall.
              <div
                aria-label="Quick questions"
                className={cn("flex flex-wrap justify-center gap-1.5", pane ? "pt-5" : "pt-4")}
              >
                {quickQuestions.slice(0, 4).map((question) => (
                  <button
                    key={question.id}
                    type="button"
                    title={question.prompt}
                    onClick={() => sendQuick(question)}
                    className={cn(
                      "inline-flex min-h-7 items-center rounded-[14px] border border-foreground/[0.14] px-3 py-[4.5px] text-left leading-[1.4] text-quiet-foreground hover:border-foreground/[0.28] hover:bg-foreground/[0.05]",
                      pane ? "text-[12px]" : "text-[11.5px]",
                    )}
                  >
                    {question.name}
                  </button>
                ))}
                {quickQuestions.length > 4 && (
                  <button
                    type="button"
                    onClick={openQuickMenu}
                    className={cn(
                      "inline-flex min-h-7 items-center rounded-[14px] px-3 py-[4.5px] text-quiet-foreground hover:bg-foreground/[0.05]",
                      pane ? "text-[12px]" : "text-[11.5px]",
                    )}
                  >
                    {quickQuestions.length - 4} more
                  </button>
                )}
              </div>
            )}
          </>
        ) : (
          <div className={cn("flex flex-col", pane ? "gap-5" : "gap-3.5")}>
            {messages.map((message, index) =>
              message.role === "user" ? (
                <div key={index} className="flex flex-col gap-1.5">
                  <span className="text-[11px] font-semibold text-muted-foreground">
                    You asked{message.asked_at ? ` · ${message.asked_at}` : ""}
                  </span>
                  <p
                    className={cn(
                      "font-serif leading-[1.5] text-foreground",
                      pane ? "text-[17px]" : "text-[14.5px]",
                    )}
                  >
                    {message.question_name ?? message.text}
                  </p>
                </div>
              ) : (
                <div
                  key={index}
                  className={cn(
                    "flex flex-col gap-2 rounded-[10px] border border-foreground/[0.14] bg-foreground/[0.03] dark:bg-foreground/[0.05]",
                    pane ? "px-4 py-3.5" : "px-3.5 py-3",
                  )}
                >
                  <div className="flex items-baseline justify-between gap-3">
                    <span className="text-[11px] font-semibold text-muted-foreground">
                      {distinctSourceCount(message.sources) > 0
                        ? `From ${distinctSourceCount(message.sources)} ${distinctSourceCount(message.sources) === 1 ? "note" : "notes"}`
                        : "Answer"}
                    </span>
                    {message.label && (
                      <span className="shrink-0 text-[11px] text-muted-foreground">
                        {message.label}
                      </span>
                    )}
                  </div>
                  {/* `chat-answer` (index.css) wins over .markdown-content's
                      serif/15px; the surface sets the size via the var. */}
                  <MarkdownContent
                    className="chat-answer"
                    style={{ "--chat-answer-size": pane ? "13.5px" : "12px" } as React.CSSProperties}
                  >
                    {message.text}
                  </MarkdownContent>
                  {message.sources.length > 0 && (
                    <div className="flex flex-wrap gap-1.5 pt-0.5">
                      {message.sources.map((source, sourceIndex) => (
                        <button
                          key={sourceIndex}
                          type="button"
                          onClick={() => onOpenCitation(source)}
                          className="inline-flex h-6 items-center rounded-full border border-foreground/[0.16] bg-background px-2.5 text-[11px] tabular-nums text-quiet-foreground hover:border-foreground/[0.3] hover:bg-foreground/[0.06]"
                        >
                          {sourceChipLabel(source, sessions)}
                        </button>
                      ))}
                    </div>
                  )}
                  {/* Followups only on the newest answer: stale suggestions
                      under old answers would fight the current thread. */}
                  {index === messages.length - 1 && message.followups.length > 0 && (
                    <div className="flex flex-wrap gap-1.5 pt-1">
                      {message.followups.map((followup) => (
                        // No fixed height: a long suggestion wraps into a
                        // taller pill instead of overflowing a 28px one.
                        <button
                          key={followup}
                          type="button"
                          disabled={running}
                          onClick={() => send(followup)}
                          className={cn(
                            "inline-flex min-h-7 items-center rounded-[14px] border border-foreground/[0.14] px-3 py-[4.5px] text-left leading-[1.4] text-quiet-foreground hover:border-foreground/[0.28] hover:bg-foreground/[0.05] disabled:opacity-50",
                            pane ? "text-[12px]" : "text-[11.5px]",
                          )}
                        >
                          {followup}
                        </button>
                      ))}
                    </div>
                  )}
                </div>
              ),
            )}
            {(pendingQuestion !== null || running) && (
              <>
                {pendingQuestion !== null && (
                  <div className="flex flex-col gap-1.5">
                    <span className="text-[11px] font-semibold text-muted-foreground">
                      You asked{pendingAskedAt ? ` · ${pendingAskedAt}` : ""}
                    </span>
                    <p
                      className={cn(
                        "font-serif leading-[1.5] text-foreground",
                        pane ? "text-[17px]" : "text-[14.5px]",
                      )}
                    >
                      {pendingName ?? pendingQuestion}
                    </p>
                  </div>
                )}
                <div
                  className={cn(
                    "flex items-center gap-2.5 rounded-[10px] border border-foreground/[0.14] bg-foreground/[0.03] dark:bg-foreground/[0.05]",
                    pane ? "px-4 py-3.5" : "px-3.5 py-3",
                  )}
                >
                  <span aria-hidden="true" className="working-spinner size-3 shrink-0 text-muted-foreground" />
                  <span className="flex-1 text-[11px] tabular-nums text-muted-foreground">
                    {scopePath === null ? "reading your notes" : "reading this call"}
                  </span>
                  <button
                    type="button"
                    aria-label="Stop answering"
                    title="Stop"
                    onClick={() => void chatCancel(scopePath)}
                    className="inline-flex size-6 items-center justify-center rounded-md text-muted-foreground hover:bg-foreground/[0.07] hover:text-foreground"
                  >
                    <Square className="size-3 fill-current" />
                  </button>
                </div>
              </>
            )}
            {error && (
              <p role="alert" className="text-[12px] leading-[1.5] text-destructive [overflow-wrap:anywhere]">
                {error}
              </p>
            )}
          </div>
        )}
      </div>

      <div
        className={cn(
          "relative flex shrink-0 items-center border-t border-foreground/[0.1]",
          pane ? "gap-3 bg-card px-14 py-4" : "px-[18px] py-3",
        )}
      >
        {menuOpen && (
          <div
            role="listbox"
            aria-label="Quick questions"
            className={cn(
              "absolute bottom-full z-10 mb-1 max-h-64 overflow-y-auto rounded-[10px] border border-border bg-popover p-1 shadow-[0_4px_16px_rgba(0,0,0,0.12)]",
              pane ? "left-14 right-14" : "left-[18px] right-[18px]",
            )}
          >
            {menuItems.length === 0 ? (
              <p className="px-2.5 py-2 text-[12px] text-muted-foreground">No quick question matches</p>
            ) : (
              menuItems.map((question, index) => (
                <button
                  key={question.id}
                  type="button"
                  role="option"
                  aria-selected={index === menuIndex}
                  onMouseMove={() => setMenuIndex(index)}
                  // mousedown, not click: a click would blur the input first
                  // and the menu (which lives off the draft) would vanish.
                  onMouseDown={(event) => {
                    event.preventDefault();
                    sendQuick(question);
                  }}
                  className={cn(
                    "flex w-full flex-col items-start gap-px rounded-[7px] px-2.5 py-1.5 text-left",
                    index === menuIndex ? "bg-foreground/[0.08]" : "hover:bg-foreground/[0.05]",
                  )}
                >
                  <span className="text-[12.5px] font-medium text-foreground">{question.name}</span>
                  <span className="line-clamp-1 text-[11px] text-muted-foreground">{question.prompt}</span>
                </button>
              ))
            )}
          </div>
        )}
        <div className="flex h-9 min-w-0 flex-1 items-center gap-2 rounded-full border border-input bg-background px-3.5 focus-within:border-foreground focus-within:ring-[3px] focus-within:ring-ring/35">
          {quickQuestions.length > 0 && (
            <button
              type="button"
              aria-label="Quick questions"
              title="Quick questions (type /)"
              onClick={openQuickMenu}
              className="inline-flex size-5 shrink-0 items-center justify-center rounded-full text-muted-foreground hover:text-foreground"
            >
              <Zap className="size-[13px]" />
            </button>
          )}
          <input
            ref={inputRef}
            type="text"
            aria-label={placeholder}
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
            onKeyDown={handleKeyDown}
            placeholder={placeholder}
            className={cn(
              "min-w-0 flex-1 bg-transparent outline-none placeholder:text-muted-foreground",
              pane ? "text-[13px]" : "text-[12.5px]",
            )}
          />
          <button
            type="button"
            aria-label="Send question"
            disabled={running || draft.trim() === ""}
            onClick={() => send(draft)}
            className="inline-flex h-[17px] shrink-0 items-center gap-0.5 rounded-[4px] bg-foreground/[0.07] px-[5px] text-[11px] text-quiet-foreground disabled:opacity-60"
          >
            <CornerDownLeft className="size-2.5" />
          </button>
        </div>
      </div>
    </div>
  );
}
