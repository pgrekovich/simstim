import {
  forwardRef,
  useEffect,
  useImperativeHandle,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { Check, ChevronDown, ChevronUp, Sparkles, X } from "lucide-react";
import { cn } from "@/lib/utils";
import { createPortal } from "react-dom";
import { convertFileSrc } from "@tauri-apps/api/core";
import { AudioPlayer, type AudioPlayerHandle } from "@/components/AudioPlayer";
import {
  MarkdownContent,
  MarkdownEditor,
  type MarkdownEditorHandle,
} from "@/components/Markdown";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { findTranscriptMatches, TranscriptView } from "@/components/TranscriptView";
import {
  enhanceSession,
  readSession,
  retranscribeSession,
  saveSession,
  updateActiveSession,
  type SessionData,
} from "@/lib/ipc";
import { formatDuration } from "@/lib/utils";
import { describePeople } from "@/lib/people";
import { STANDARD_STYLE, styleName, usePrompts } from "@/lib/prompts";
import { isTranscriptUpdatedButAudioDeleteFailed, type RetranscribeJob } from "@/lib/retranscribeStatus";

/**
 * Debounce note saves for 500 ms after the last keystroke.
 *
 */
const NOTES_DEBOUNCE_MS = 500;
const TOPBAR_GAP_PX = 12;
const MIN_FIND_GAP_PX = 16;

export function shouldStackTranscriptFind(
  tabsRight: number,
  actionsLeft: number,
  findWidth: number,
): boolean {
  const candidateFindLeft = actionsLeft - TOPBAR_GAP_PX - findWidth;
  return candidateFindLeft - tabsRight < MIN_FIND_GAP_PX;
}

/**
 * The quiet meta line under a past note's title: "Sep 2, 14:03 · 10:11",
 * with the year spelled out once the note is from a different year than
 * `now`. The sidebar carries the same facts, but it groups by relative day
 * and can be collapsed entirely -- an opened note has to say when its call
 * happened on its own. Returns `null` for a blank/invalid date (the active
 * session's placeholder `initialData.date` is `""`).
 */
export function formatSessionMeta(
  dateStr: string,
  durationSecs: number | null,
  now = new Date(),
): string | null {
  const d = new Date(dateStr);
  if (dateStr === "" || Number.isNaN(d.getTime())) return null;
  const date = d.toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
    ...(d.getFullYear() !== now.getFullYear() ? { year: "numeric" } : {}),
  });
  const time = `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
  const duration = formatDuration(durationSecs);
  return duration ? `${date}, ${time} · ${duration}` : `${date}, ${time}`;
}

interface SessionViewProps {
  path: string;
  /** Whether `path` is the session currently being recorded. */
  isActive: boolean;
  initialData: SessionData;
  /**
   * The session's recorded length in seconds, from its sidebar `SessionMeta`
   * (`SessionData` itself doesn't carry one). Shown in the quiet meta line
   * under the title next to the date; `null`/omitted hides the duration
   * half. Ignored while `isActive` (the live clock is in the Stop button).
   */
  durationSecs?: number | null;
  /** Live in-memory finalized lines for the active session (ignored otherwise). */
  liveLines: string[];
  /** Live interim drafts for the active session (ignored otherwise). */
  interimMe: string;
  interimThem: string;
  /**
 * Job state lives in App so it survives switching notes and remounting this
 * view.
 *
 */
  retranscribingJob: RetranscribeJob;
  /**
   * Whether an enhance is currently running for THIS path -- owned by App
   * (its `enhance-progress` listener), for exactly the reason
   * `retranscribingJob` is: the backend job survives this component's
   * remount, so a local boolean would silently re-enable the control
   * mid-job. Drives the Summary tab's spinner and disables Enhance and
   * re-run. Optional so tests exercising unrelated behavior can omit it.
   */
  enhanceRunning?: boolean;
  /**
 * Progress text from App's enhance-progress listener, shown in the Summary
 * tab tooltip.
 */
  enhanceProgress?: string | null;
  /** Notifies the caller after a title save resolves, so it can refresh the sidebar. */
  onTitleChange?: () => void;
  /**
   * Fires synchronously on every title/notes change (including the
   * empty-title-\>-"Untitled" normalization on commit), for the active
   * session only. Undebounced and independent of save success: App uses
   * this to cache the latest typed title/notes across a remount (see
   * `ACTIVE_SESSION_PLACEHOLDER` / `activeTitle`/`activeNotes` in App.tsx),
   * so switching away from the active session and back doesn't reseed this
   * component with stale/blank content.
   */
  onActiveEdit?: (title: string, notes: string) => void;
  /**
   * A title/notes save rejected (rare: a real backend failure, not the
   * ordinary no-op-when-unchanged skip). Surfaced through the same status
   * line App already uses for `session-error` events, since a save failure
   * here is just as actionable to the user.
   */
  onSaveError?: (message: string) => void;
  /**
 * Mark a manual request before awaiting it so App can distinguish inline
 * failures from automatic background errors.
 *
 */
  onRetranscribeStart?: (path: string) => void;
  /**
 * Clear manual-job tracking when validation rejects before a terminal
 * progress event. Otherwise later automatic failures on this path could be
 * suppressed.
 *
 */
  onRetranscribeEnd?: (path: string) => void;
  transcriptFindRequest?: TranscriptFindRequest | null;
  /**
 * Scroll-to-citation: a chat source chip for THIS session was clicked.
 * Switches to the transcript tab and centers the line nearest the cited
 * timestamp (passed through to `TranscriptView`'s `jumpRequest`). Same
 * monotonic-id recipe as `transcriptFindRequest`.
 *
 */
  transcriptJumpRequest?: { id: number; timestamp: string } | null;
}

export interface TranscriptFindRequest {
  id: number;
  /** `null` closes find; an empty string opens a fresh local find. */
  query: string | null;
  timestamp?: string;
}

/**
 * Imperative escape hatch for the one case a keyed remount's unmount
 * cleanup can't handle correctly on its own: see the `flushPendingEdits`
 * doc comment below.
 */
export interface SessionViewHandle {
  flushPendingEdits: () => Promise<void>;
  focusNotes: () => void;
}

/**
 * Render one session with editable notes and transcript. App keys the
 * component by path, so local state is seeded on mount and preserved during
 * edits. Flush debounced edits before recording ownership changes; use the
 * current active writer when saving. Background processing can outlive this
 * view and is tracked by App.
 *
 */
export const SessionView = forwardRef<SessionViewHandle, SessionViewProps>(function SessionView(
  {
    path,
    isActive,
    initialData,
    durationSecs = null,
    liveLines,
    interimMe,
    interimThem,
    retranscribingJob,
    enhanceRunning = false,
    enhanceProgress = null,
    onTitleChange,
    onSaveError,
    onActiveEdit,
    onRetranscribeStart,
    onRetranscribeEnd,
    transcriptFindRequest,
    transcriptJumpRequest = null,
  },
  ref
) {
  const [title, setTitle] = useState(initialData.title);
  const [notes, setNotes] = useState(initialData.notes);
  // Seed transcript state on mount and refresh it after transcription. Audio
  // availability comes from the parent's refreshed data because successful cleanup can
  // remove the file.
  const [transcript, setTranscript] = useState(initialData.transcript);
  // Read straight from the mount's initialData: when an enhance finishes,
  // App refetches the session and remounts this component under a bumped
  // `reloadTick` key (its `enhance-progress` listener), so there is no
  // in-place refresh to track here anymore.
  const summary = initialData.summary;
  const summaryModel = initialData.summary_model;
  // Which of the user's summary styles wrote this summary (`summary_style:`
  // frontmatter), `null` for the built-in prompt. The style menu in the
  // provenance line lists the current prompts.toml styles.
  const summaryStyle = initialData.summary_style;
  const { prompts } = usePrompts();
  // Local-only optimistic flag for Enhance, same shape as `clicking` below:
  // covers the instants before the backend's `enhance-progress` "running"
  // event lands and flips the `enhanceRunning` prop.
  const [enhanceClicking, setEnhanceClicking] = useState(false);
  // "copied" flashes in the provenance line for a beat after Copy summary.
  const [summaryCopied, setSummaryCopied] = useState(false);
  const [transcriptCopied, setTranscriptCopied] = useState(false);
  const copiedTimerRef = useRef<number | null>(null);
  useEffect(
    () => () => {
      if (copiedTimerRef.current !== null) window.clearTimeout(copiedTimerRef.current);
    },
    [],
  );
  // Local-only optimistic flag: disables the button the instant it's
  // clicked, before the backend's first `retranscribe-progress` event has
  // had a chance to land and update `retranscribingJob` (a window of at
  // most a few milliseconds -- the command validates and claims the job
  // slot synchronously before any await). Safe to let this reset on a
  // remount unlike the job state itself: `retranscribingJob` is what
  // actually guarantees the button stays disabled across one, this is only
  // ever read while the SAME click's promise is still in flight.
  const [clicking, setClicking] = useState(false);
  const [retranscribeError, setRetranscribeError] = useState<string | null>(null);
  const [activeTab, setActiveTab] = useState(() =>
    !isActive && initialData.notes.trim().length === 0 && initialData.summary
      ? "summary"
      : "notes"
  );
  const [findOpen, setFindOpen] = useState(false);
  const [findQuery, setFindQuery] = useState("");
  const [activeFindIndex, setActiveFindIndex] = useState(0);
  const [findStacked, setFindStacked] = useState(false);
  const findInputRef = useRef<HTMLInputElement>(null);
  const findControlsRef = useRef<HTMLDivElement>(null);
  const tabListRef = useRef<HTMLDivElement>(null);
  const handledFindRequestRef = useRef<number | null>(null);
  const pendingFindTargetRef = useRef<{ query: string; timestamp?: string } | null>(null);
  const playerRef = useRef<AudioPlayerHandle>(null);
  const jobRunningHere = retranscribingJob?.path === path;
  // App's toolbar renders an empty `#topbar-tabs-slot`; the tab switcher is
  // portaled into it to match the design while remaining inside the Radix
  // Tabs React tree. Looked up in an effect because the slot is a sibling DOM
  // subtree. Component tests have no slot and use the inline fallback.
  const [toolbarSlot, setToolbarSlot] = useState<HTMLElement | null>(null);
  const [toolbarFindSlot, setToolbarFindSlot] = useState<HTMLElement | null>(null);
  useLayoutEffect(() => {
    setToolbarSlot(document.getElementById("topbar-tabs-slot"));
    setToolbarFindSlot(document.getElementById("topbar-transcript-find-slot"));
  }, []);

  useLayoutEffect(() => {
    if (!findOpen || !toolbarFindSlot) {
      setFindStacked(false);
      return;
    }
    const header = toolbarFindSlot.parentElement;
    if (!header) return;

    let frame: number | null = null;
    const measure = () => {
      frame = null;
      const tabs = tabListRef.current;
      const find = findControlsRef.current;
      if (!tabs || !find) return;

      const children = Array.from(header.children);
      const slotIndex = children.indexOf(toolbarFindSlot);
      const nextVisible = children.slice(slotIndex + 1).find((element) => {
        const rect = element.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0;
      });
      if (!nextVisible) return;

      setFindStacked(shouldStackTranscriptFind(
        tabs.getBoundingClientRect().right,
        nextVisible.getBoundingClientRect().left,
        find.getBoundingClientRect().width,
      ));
    };
    const scheduleMeasure = () => {
      if (frame !== null) window.cancelAnimationFrame(frame);
      frame = window.requestAnimationFrame(measure);
    };

    const observer = new ResizeObserver(scheduleMeasure);
    const mutationObserver = new MutationObserver(scheduleMeasure);
    observer.observe(header);
    observer.observe(tabListRef.current ?? toolbarFindSlot);
    observer.observe(findControlsRef.current ?? toolbarFindSlot);
    mutationObserver.observe(header, {
      attributes: true,
      characterData: true,
      childList: true,
      subtree: true,
    });
    window.addEventListener("resize", scheduleMeasure);
    measure();

    return () => {
      if (frame !== null) window.cancelAnimationFrame(frame);
      observer.disconnect();
      mutationObserver.disconnect();
      window.removeEventListener("resize", scheduleMeasure);
    };
  }, [findOpen, toolbarFindSlot]);

  // "Latest value" mirrors of the controlled state above, read from
  // callbacks (blur handlers, the debounce timeout, the unmount cleanup)
  // that must never close over a stale render's value.
  const titleRef = useRef(title);
  const notesRef = useRef(notes);
  const titleFieldRef = useRef<HTMLTextAreaElement | null>(null);

  // The title field is a textarea so long titles wrap instead of clipping,
  // but it must never scroll internally: grow it to fit its content, and
  // re-fit when the available width changes (window resize, sidebar toggle).
  const fitTitleField = () => {
    const el = titleFieldRef.current;
    if (!el) return;
    el.style.height = "0";
    el.style.height = `${el.scrollHeight}px`;
  };

  useLayoutEffect(fitTitleField, [title]);

  useEffect(() => {
    const el = titleFieldRef.current;
    if (!el) return;
    const observer = new ResizeObserver(fitTitleField);
    observer.observe(el);
    return () => observer.disconnect();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  const notesInputRef = useRef<MarkdownEditorHandle>(null);
  const savedTitleRef = useRef(title);
  const savedNotesRef = useRef(notes);
  const isActiveRef = useRef(isActive);
  const notesTimeoutRef = useRef<number | null>(null);

  // Keeps isActiveRef in sync with the isActive prop for the ordinary
  // (non-stop) routing decisions: a debounce timeout firing, or a blur, a
  // little while after isActive last changed. This does NOT fix the stop
  // race described above -- that race happens because no re-render (and so
  // no effect run) occurs at all before the unmount; `flushPendingEdits` is
  // the actual fix for that. This effect just keeps the ref honest for
  // every other case, instead of mutating a ref during render.
  useEffect(() => {
    isActiveRef.current = isActive;
  }, [isActive]);

  // Save changed titles through the current writer, normalizing empty titles to
  // Untitled. Include current notes in inactive-file saves. Advance savedTitleRef only
  // after successful IPC so a failed save can be retried.
  function commitTitle(): Promise<void> {
    const rawValue = titleRef.current;
    const value = rawValue.trim().length > 0 ? rawValue : "Untitled";
    if (value !== rawValue) {
      setTitle(value);
      titleRef.current = value;
      onActiveEdit?.(titleRef.current, notesRef.current);
    }
    if (value === savedTitleRef.current) return Promise.resolve();
    const result = isActiveRef.current
      ? updateActiveSession(value, undefined)
      : saveSession(path, value, notesRef.current);
    return result
      .then(() => {
        savedTitleRef.current = value;
        onTitleChange?.();
      })
      .catch((err) => {
        // A real failure (not the no-op skip above): rare (e.g. the
        // "not recording" rejection if this races past a stop, or a disk
        // error), but not silently swallowed -- the worker's own
        // `session-error` event covers worker-internal failures, not a
        // rejection of this direct IPC call, so it's surfaced here too.
        console.error("failed to save session title", err);
        onSaveError?.(String(err));
      });
  }

  // Same shape as `commitTitle` for notes (see its doc comment for why
  // `savedNotesRef` only advances after a successful save). Also clears any
  // pending debounce timer, so it's safe to call this directly (blur,
  // unmount, explicit flush) without a duplicate save firing later.
  function commitNotes(): Promise<void> {
    if (notesTimeoutRef.current !== null) {
      window.clearTimeout(notesTimeoutRef.current);
      notesTimeoutRef.current = null;
    }
    const value = notesRef.current;
    if (value === savedNotesRef.current) return Promise.resolve();
    const result = isActiveRef.current
      ? updateActiveSession(undefined, value)
      : saveSession(path, titleRef.current, value);
    return result
      .then(() => {
        savedNotesRef.current = value;
      })
      .catch((err) => {
        // See commitTitle.
        console.error("failed to save session notes", err);
        onSaveError?.(String(err));
      });
  }

  // The explicit flush App calls before stop_recording (see the class doc
  // comment above). Notes before title only matters for the inactive-path
  // save shape (each sends the other field's latest ref value along), not
  // for the active path, where the two routes are independent partial
  // updates.
  useImperativeHandle(ref, () => ({
    flushPendingEdits: async () => {
      await commitNotes();
      await commitTitle();
    },
    focusNotes: () => {
      setActiveTab("notes");
      window.requestAnimationFrame(() => notesInputRef.current?.focus());
    },
  }));

  // Flushes both pending edits when this exact instance goes away: the app
  // closing, or (the common case) the user switching to a different
  // session, which remounts this component under a new `key={path}` and
  // runs this cleanup for the outgoing instance first. Deliberately an
  // empty dep array: `commitTitle`/`commitNotes` only ever read from refs
  // and the `path` prop, which is fixed for the lifetime of this
  // keyed instance, so the closures captured at mount stay correct. (The
  // stop transition is the one case this cleanup can't handle correctly on
  // its own -- see `flushPendingEdits`.)
  useEffect(() => {
    return () => {
      commitNotes();
      commitTitle();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Click-to-seek: seeks the player to the clicked transcript line's offset and starts
  // playback. Only ever wired up by the JSX below when `initialData.audio_path` is set,
  // so `playerRef.current` being null here would mean the player was unmounted out from
  // under an in-flight click -- defensive, not expected in practice.
  function handleSeek(offsetSecs: number) {
    playerRef.current?.seek(offsetSecs);
  }

  // Start a manual transcription request and reread the note after completion. Job
  // progress lives in App because it can outlive this view. Notify App when starting and
  // when early validation rejects without a progress event.
  async function handleRetranscribe() {
    if (!initialData.audio_path || isActive || clicking || jobRunningHere) return;
    setClicking(true);
    setRetranscribeError(null);
    onRetranscribeStart?.(path);
    try {
      await retranscribeSession(path);
      const data = await readSession(path);
      setTranscript(data.transcript);
    } catch (err) {
      console.error("failed to re-transcribe session", err);
      setRetranscribeError(String(err));
      // the "transcript updated, but the audio could not be deleted" rejection is the
      // one case where the promise rejects even though the transcript itself WAS
      // rewritten -- `maybe_delete_audio`'s own failure surfaces through the same
      // "error" phase/rejection as a genuine pipeline failure, but only the deletion
      // step afterward actually failed. Without refetching here too, this on-screen
      // transcript stays stale (the success path above never runs, since the promise
      // rejected).
      if (isTranscriptUpdatedButAudioDeleteFailed(String(err))) {
        readSession(path)
          .then((data) => setTranscript(data.transcript))
          .catch((refetchErr) => console.error("failed to refetch after re-transcribe", refetchErr));
      }
      // Covers the rejections that never produce a `retranscribe-progress`
      // event at all (the up-front validation checks -- see
      // `onRetranscribeEnd`'s doc comment) as well as the rarer case where
      // the pass itself succeeded but the follow-up `readSession` failed
      // (that one DID already get a "done" event, so this is a harmless
      // no-op there).
      onRetranscribeEnd?.(path);
    } finally {
      setClicking(false);
    }
  }

  // Enhance: the user's rough notes lead, the transcript backs them, and the configured
  // provider writes the result into `## Summary`. Flushes pending edits FIRST so what
  // the model reads from disk is what's on screen. Progress, the refreshed summary and
  // any failure all flow through the backend's `enhance-progress` events (App's
  // listener), which this command also emits for its own run -- so the rejection here is
  // deliberately not shown a second time.
  /**
 * Copies the summary as the Markdown it is (shareable in one paste).
 * The clipboard API is user-gesture gated, and this only ever runs from a
 * click; a rejection just leaves the label as "copy".
 *
 */
  function handleCopySummary() {
    const text = summary.trim();
    if (!text) return;
    void navigator.clipboard
      ?.writeText(text)
      .then(() => {
        setSummaryCopied(true);
        if (copiedTimerRef.current !== null) window.clearTimeout(copiedTimerRef.current);
        copiedTimerRef.current = window.setTimeout(() => setSummaryCopied(false), 1500);
      })
      .catch(() => {});
  }

  /** The whole transcript as the file has it, one timestamped line per chunk. */
  function handleCopyTranscript() {
    const text = transcriptLines.join("\n").trim();
    if (!text) return;
    void navigator.clipboard
      ?.writeText(text)
      .then(() => {
        setTranscriptCopied(true);
        if (copiedTimerRef.current !== null) window.clearTimeout(copiedTimerRef.current);
        copiedTimerRef.current = window.setTimeout(() => setTranscriptCopied(false), 1500);
      })
      .catch(() => {});
  }

  /**
   * `style` is a summary style id (or `"standard"`) to write the summary
   * in; omitted, the backend repeats the style the note remembers.
   */
  async function handleEnhance(style?: string) {
    if (isActive || enhanceBusy) return;
    setEnhanceClicking(true);
    try {
      await commitNotes();
      await commitTitle();
      await enhanceSession(path, style);
    } catch (err) {
      console.error("failed to enhance session", err);
    } finally {
      setEnhanceClicking(false);
    }
  }

  function handleTitleChange(e: React.ChangeEvent<HTMLTextAreaElement>) {
    // The field is a textarea only so long titles can wrap; the title
    // itself stays one logical line (it is the file's H1). Enter never
    // types (see handleTitleKeyDown), but a multiline paste would slip
    // newlines in, so collapse them here.
    const value = e.target.value.replace(/\s*\n\s*/g, " ");
    setTitle(value);
    titleRef.current = value;
    onActiveEdit?.(titleRef.current, notesRef.current);
  }

  // Enter blurs the field, which triggers the save via onBlur below.
  function handleTitleKeyDown(e: React.KeyboardEvent<HTMLTextAreaElement>) {
    if (e.key === "Enter") {
      e.preventDefault();
      e.currentTarget.blur();
    }
  }

  function handleNotesChange(e: React.ChangeEvent<HTMLTextAreaElement>) {
    const value = e.target.value;
    setNotes(value);
    notesRef.current = value;
    onActiveEdit?.(titleRef.current, notesRef.current);
    if (notesTimeoutRef.current !== null) {
      window.clearTimeout(notesTimeoutRef.current);
    }
    notesTimeoutRef.current = window.setTimeout(() => {
      notesTimeoutRef.current = null;
      commitNotes();
    }, NOTES_DEBOUNCE_MS);
  }

  const transcriptLines = useMemo(
    () => isActive ? liveLines : transcript.split("\n").filter((line) => line.length > 0),
    [isActive, liveLines, transcript],
  );
  const transcriptMatches = useMemo(
    () => findTranscriptMatches(transcriptLines, findQuery),
    [transcriptLines, findQuery],
  );

  useEffect(() => {
    if (!transcriptFindRequest) return;
    if (handledFindRequestRef.current === transcriptFindRequest.id) return;
    handledFindRequestRef.current = transcriptFindRequest.id;
    if (transcriptFindRequest.query === null) {
      setFindOpen(false);
      setFindQuery("");
      setActiveFindIndex(0);
      return;
    }
    setActiveTab("transcript");
    setFindOpen(true);
    setFindQuery(transcriptFindRequest.query);
    pendingFindTargetRef.current = {
      query: transcriptFindRequest.query,
      timestamp: transcriptFindRequest.timestamp,
    };
    setActiveFindIndex(0);
    window.requestAnimationFrame(() => findInputRef.current?.focus());
  }, [transcriptFindRequest]);

  useEffect(() => {
    const pending = pendingFindTargetRef.current;
    if (pending && pending.query === findQuery) {
      const targetLine = pending.timestamp
        ? transcriptLines.findIndex((line) => line.startsWith(`[${pending.timestamp}]`))
        : -1;
      const targetMatch = targetLine === -1
        ? 0
        : transcriptMatches.findIndex((match) => match.lineIndex === targetLine);
      setActiveFindIndex(Math.max(0, targetMatch));
      pendingFindTargetRef.current = null;
      return;
    }
    setActiveFindIndex((current) =>
      transcriptMatches.length === 0 ? 0 : Math.min(current, transcriptMatches.length - 1),
    );
  }, [findQuery, transcriptLines, transcriptMatches]);

  // A citation jump must also surface the transcript tab: the chip lives in
  // the chat drawer, but its target is a transcript line. The scroll itself
  // happens inside TranscriptView (it owns the line refs).
  const handledJumpTabRef = useRef(0);
  useEffect(() => {
    if (!transcriptJumpRequest || handledJumpTabRef.current === transcriptJumpRequest.id) return;
    handledJumpTabRef.current = transcriptJumpRequest.id;
    setActiveTab("transcript");
  }, [transcriptJumpRequest]);

  function moveFind(direction: 1 | -1) {
    if (transcriptMatches.length === 0) return;
    setActiveFindIndex((current) =>
      (current + direction + transcriptMatches.length) % transcriptMatches.length,
    );
  }

  function closeFind() {
    setFindOpen(false);
    setFindQuery("");
    setActiveFindIndex(0);
  }
  const audioPath = initialData.audio_path;
  const retranscribeBusy = clicking || jobRunningHere;
  const enhanceBusy = enhanceClicking || enhanceRunning;
  // Enhance needs a finished transcript to read: never during recording,
  // never on a transcript-less note (the backend rejects both anyway).
  const canEnhance = !isActive && transcriptLines.length > 0;
  const retranscribeStatusText = jobRunningHere
    ? retranscribingJob!.text
    : clicking
      ? "starting..."
      : null;

  // The date/duration meta plus who was on the call (`people:` frontmatter,
  // saved by the enhance calendar lookup). One quiet line; a long roster
  // truncates rather than wraps under the title.
  const sessionMeta = formatSessionMeta(initialData.date, durationSecs);
  // Who was on the call, on its own line so a big meeting never truncates
  // the date: readable names (emails become "M. Rivera"), the first few
  // spelled out and the rest counted, the full roster on hover.
  const people = describePeople(initialData.people);

  const titleInput = (
    <textarea
      ref={titleFieldRef}
      value={title}
      onChange={handleTitleChange}
      onKeyDown={handleTitleKeyDown}
      onBlur={commitTitle}
      placeholder="Untitled"
      rows={1}
      className="block w-full min-w-0 resize-none overflow-hidden bg-transparent font-serif text-[26px] font-semibold leading-[1.25] tracking-[-0.01em] outline-none placeholder:text-muted-foreground"
    />
  );

  const findControls = findOpen && activeTab === "transcript" ? (
    <div
      ref={findControlsRef}
      className={cn(
        "flex h-7 shrink-0 items-center gap-2 rounded-full border border-input bg-background py-0 pl-[11px] pr-[5px] shadow-none focus-within:border-foreground focus-within:ring-[3px] focus-within:ring-ring/35",
        findStacked && "fixed right-8 top-[60px] z-20",
      )}
    >
      <input
        ref={findInputRef}
        autoFocus
        type="search"
        aria-label="Find in transcript"
        value={findQuery}
        onChange={(event) => {
          setFindQuery(event.target.value);
          setActiveFindIndex(0);
        }}
        onKeyDown={(event) => {
          if (event.key === "Enter") {
            event.preventDefault();
            moveFind(event.shiftKey ? -1 : 1);
          } else if (event.key === "Escape") {
            event.preventDefault();
            closeFind();
          }
        }}
        placeholder="Find"
        className="w-[clamp(64px,8vw,120px)] min-w-0 bg-transparent text-[11.5px] outline-none placeholder:text-muted-foreground"
      />
      <span className="shrink-0 text-[11px] tabular-nums text-muted-foreground" aria-live="polite">
        {transcriptMatches.length === 0 ? 0 : activeFindIndex + 1} / {transcriptMatches.length}
      </span>
      <span className="flex shrink-0 gap-0.5">
        <Button
          type="button"
          variant="ghost"
          size="icon-xs"
          className="size-[18px] rounded-full bg-foreground/[0.07]"
          aria-label="Previous transcript match"
          disabled={transcriptMatches.length === 0}
          onClick={() => moveFind(-1)}
        >
          <ChevronUp className="size-[11px]" />
        </Button>
        <Button
          type="button"
          variant="ghost"
          size="icon-xs"
          className="size-[18px] rounded-full bg-foreground/[0.07]"
          aria-label="Next transcript match"
          disabled={transcriptMatches.length === 0}
          onClick={() => moveFind(1)}
        >
          <ChevronDown className="size-[11px]" />
        </Button>
        <Button
          type="button"
          variant="ghost"
          size="icon-xs"
          className="size-[18px] rounded-full text-muted-foreground"
          aria-label="Close transcript find"
          onClick={closeFind}
        >
          <X className="size-[11px]" />
        </Button>
      </span>
    </div>
  ) : null;

  // The Enhance affordance lives next to the tab switcher in the toolbar
  // (the note's one command strip), not at the bottom of the notes pane: a
  // compact bordered pill in the design language (mono label, ochre AI
  // mark). While a job runs -- started here, from re-run, or automatically
  // -- the pill yields to a spinner ON the Summary tab itself (the old
  // separate work chip clipped against the toolbar's find slot and read as
  // a second control). Once a summary exists the pill retires; re-runs
  // live in the Summary tab's provenance line.
  const enhanceControl = enhanceBusy ? null : canEnhance && !summary ? (
    <Button
      type="button"
      variant="outline"
      size="sm"
      className="ml-3 h-[28px] shrink-0 gap-[6px] rounded-[7px] px-[12px] text-[12.5px] font-medium"
      onClick={() => void handleEnhance()}
    >
      <Sparkles className="size-3.5 text-[var(--voice-remote)]" />
      Summarize
    </Button>
  ) : null;

  const tabList = (
    <TabsList ref={tabListRef} className="h-[32px] shrink-0 gap-0 rounded-full bg-foreground/[0.06] p-[3px] dark:bg-foreground/[0.08]">
      <TabsTrigger
        className="h-[26px] gap-[6px] rounded-full px-[14px] text-[13.5px] font-normal text-foreground hover:bg-foreground/[0.05] data-[state=active]:bg-foreground/[0.1] data-[state=active]:font-medium data-[state=active]:text-foreground data-[state=active]:shadow-none dark:data-[state=active]:bg-foreground/[0.16]"
        value="notes"
      >
        Notes
      </TabsTrigger>
      {/* The tab appears as soon as a summary exists OR one is being
          written: during a first run it sits disabled with a spinner (the
          material is on its way), during a re-run it stays clickable. The
          step detail ("Summarizing part 2 of 7...") lives in the tooltip. */}
      {(summary || enhanceBusy) && (
        <TabsTrigger
          className="h-[26px] gap-[6px] rounded-full px-[14px] text-[13.5px] font-normal text-foreground hover:bg-foreground/[0.05] data-[state=active]:bg-foreground/[0.1] data-[state=active]:font-medium data-[state=active]:text-foreground data-[state=active]:shadow-none dark:data-[state=active]:bg-foreground/[0.16]"
          value="summary"
          disabled={!summary}
          title={enhanceBusy ? enhanceProgress ?? "Summarizing..." : undefined}
        >
          {enhanceBusy && (
            <span data-testid="summary-tab-spinner" aria-hidden="true" className="working-spinner size-3" />
          )}
          Summary
        </TabsTrigger>
      )}
      <TabsTrigger
        className="h-[26px] gap-[6px] rounded-full px-[14px] text-[13.5px] font-normal text-foreground hover:bg-foreground/[0.05] data-[state=active]:bg-foreground/[0.1] data-[state=active]:font-medium data-[state=active]:text-foreground data-[state=active]:shadow-none dark:data-[state=active]:bg-foreground/[0.16]"
        value="transcript"
      >
        {isActive && <span aria-hidden="true" className="recording-tally size-[5px] rounded-full bg-destructive" />}
        Transcript
      </TabsTrigger>
    </TabsList>
  );

  return (
    <div className="mx-auto flex h-full min-h-0 w-full max-w-[920px] flex-col">
      {/* Notes stay primary. Generated Summary is a separate material and
          only appears after Enhance has produced it. Tab contents are
          force-mounted and CSS-hidden so a switch does not kill playback or
          reset TranscriptView's scroll. */}
      <Tabs
        value={activeTab}
        onValueChange={(value) => {
          setActiveTab(value);
          if (value !== "transcript") closeFind();
        }}
        className="min-h-0 flex-1 gap-0"
      >
        {toolbarSlot ? createPortal(<>{tabList}{enhanceControl}</>, toolbarSlot) : (
          <div className="flex h-11 shrink-0 items-center justify-between gap-3">
            <div className="flex min-w-0 items-center">
              {tabList}
              {enhanceControl}
            </div>
            {findControls}
          </div>
        )}
        {toolbarFindSlot && findControls ? createPortal(findControls, toolbarFindSlot) : null}

        {/* The title belongs to the session, not to a single material tab. */}
        <div className="shrink-0 px-6 pt-0.5">
          {/* The date sits centered above the title, the way Notes stamps a
              note. Hidden while recording: the live clock is in the Stop
              button, and the placeholder date is blank anyway. */}
          {!isActive && sessionMeta && (
            <p className="mb-2 text-center text-[11px] tabular-nums text-muted-foreground">{sessionMeta}</p>
          )}
          {titleInput}
          {!isActive && people && (
            <p className="mt-0.5 text-[11px] text-muted-foreground" title={people.full}>
              with {people.summary}
            </p>
          )}
        </div>

        <TabsContent
          value="notes"
          forceMount
          className="flex min-h-0 flex-1 flex-col gap-3 data-[state=inactive]:hidden"
        >
          <MarkdownEditor
            ref={notesInputRef}
            value={notes}
            onChange={handleNotesChange}
            onBlur={commitNotes}
            placeholder="your notes..."
          />
        </TabsContent>

        {summary && (
          <TabsContent
            value="summary"
            forceMount
            className="min-h-0 flex-1 overflow-y-auto px-6 py-4 data-[state=inactive]:hidden"
          >
            <div>
              {summaryModel && (
                <p className="mb-4 text-[11px] text-muted-foreground">
                  Written by {summaryModel}
                  {/* The style that wrote this summary, and the menu to
                      write it another way (prompts.toml). Picking a style
                      re-summarizes at once; the note remembers it. Shown
                      only when the user has styles, or when this summary
                      came from one (even a since-deleted one). */}
                  {(prompts.summary_styles.length > 0 || summaryStyle) && (
                    <>
                      {" · "}
                      <DropdownMenu>
                        <DropdownMenuTrigger asChild>
                          <button
                            type="button"
                            disabled={enhanceBusy || !canEnhance}
                            aria-label="Summary style"
                            className="inline-flex items-center gap-px underline-offset-2 hover:text-foreground hover:underline disabled:pointer-events-none disabled:opacity-50"
                          >
                            {styleName(prompts.summary_styles, summaryStyle)}
                            <ChevronDown className="size-3" />
                          </button>
                        </DropdownMenuTrigger>
                        <DropdownMenuContent align="start">
                          {[
                            { id: STANDARD_STYLE, name: styleName([], null) },
                            ...prompts.summary_styles,
                          ].map((style) => {
                            const current = (summaryStyle ?? STANDARD_STYLE) === style.id;
                            return (
                              <DropdownMenuItem
                                key={style.id}
                                onSelect={() => void handleEnhance(style.id)}
                                aria-checked={current}
                                role="menuitemradio"
                              >
                                <span className="flex-1">{style.name}</span>
                                {current && <Check aria-hidden="true" />}
                              </DropdownMenuItem>
                            );
                          })}
                        </DropdownMenuContent>
                      </DropdownMenu>
                    </>
                  )}
                  {" · "}
                  {/* Sharing is the point of a summary: one word away from
                      the clipboard as Markdown, ready for the person you
                      just talked to. */}
                  <button
                    type="button"
                    onClick={handleCopySummary}
                    aria-label="Copy summary"
                    className="underline-offset-2 hover:text-foreground hover:underline"
                  >
                    {summaryCopied ? "Copied" : "Copy"}
                  </button>
                  {canEnhance && (
                    <>
                      {" · "}
                      {/* The regenerate affordance is plain text in the
                          provenance line (design 3b): quiet, next to what it
                          replaces. The Summary tab's spinner shows the
                          running state. */}
                      <button
                        type="button"
                        disabled={enhanceBusy}
                        onClick={() => void handleEnhance()}
                        className="underline-offset-2 hover:text-foreground hover:underline disabled:pointer-events-none disabled:opacity-50"
                      >
                        {enhanceBusy ? "Summarizing..." : "Again"}
                      </button>
                    </>
                  )}
                </p>
              )}
              <MarkdownContent className="text-[14.5px]">{summary}</MarkdownContent>
            </div>
          </TabsContent>
        )}

        <TabsContent
          value="transcript"
          forceMount
          className={cn(
            "flex min-h-0 flex-1 flex-col gap-3 data-[state=inactive]:hidden",
            findStacked && "pt-10",
          )}
        >
          {/* The transcript's own quiet line, the twin of the summary's
              provenance line: copy, and the accurate pass by hand. */}
          {(transcriptLines.length > 0 || audioPath) && (
            <p className="shrink-0 px-6 text-[11px] text-muted-foreground">
              <button
                type="button"
                onClick={handleCopyTranscript}
                aria-label="Copy transcript"
                disabled={transcriptLines.length === 0}
                className="underline-offset-2 hover:text-foreground hover:underline disabled:pointer-events-none disabled:opacity-50"
              >
                {transcriptCopied ? "Copied" : "Copy"}
              </button>
              {audioPath && (
                <>
                  {" · "}
                  <button
                    type="button"
                    disabled={isActive || retranscribeBusy}
                    onClick={handleRetranscribe}
                    className="underline-offset-2 hover:text-foreground hover:underline disabled:pointer-events-none disabled:opacity-50"
                  >
                    {retranscribeBusy ? "Improving..." : "Improve transcript"}
                  </button>
                </>
              )}
            </p>
          )}
          <TranscriptView
            lines={transcriptLines}
            interimMe={isActive ? interimMe : undefined}
            interimThem={isActive ? interimThem : undefined}
            onSeek={audioPath ? handleSeek : undefined}
            matches={transcriptMatches}
            activeMatchIndex={findOpen ? activeFindIndex : -1}
            // Wait until the transcript tab is visible before scrolling. scrollIntoView
            // is ineffective inside a display:none panel.
            jumpRequest={activeTab === "transcript" ? transcriptJumpRequest : null}
          />

          {retranscribeStatusText && (
            <p className="shrink-0 px-6 text-[11px] tabular-nums text-muted-foreground">{retranscribeStatusText}</p>
          )}
          {retranscribeError && (
            <p className="shrink-0 text-sm text-destructive">{retranscribeError}</p>
          )}

          {/* Pinned under the transcript as one compact, stackable row. */}
          {audioPath && (
            <AudioPlayer
              ref={playerRef}
              src={convertFileSrc(audioPath)}
            />
          )}
        </TabsContent>
      </Tabs>
    </div>
  );
});
