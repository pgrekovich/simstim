import {
  forwardRef,
  useEffect,
  useImperativeHandle,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { ChevronDown, ChevronUp, Sparkles, X } from "lucide-react";
import { cn } from "@/lib/utils";
import { createPortal } from "react-dom";
import { convertFileSrc } from "@tauri-apps/api/core";
import { AudioPlayer, type AudioPlayerHandle } from "@/components/AudioPlayer";
import { Button } from "@/components/ui/button";
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
import { isTranscriptUpdatedButAudioDeleteFailed, type RetranscribeJob } from "@/lib/retranscribeStatus";

/** Global Constraints (Task 6): notes save 500ms after the last keystroke. */
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

interface SessionViewProps {
  path: string;
  /** Whether `path` is the session currently being recorded. */
  isActive: boolean;
  initialData: SessionData;
  /** Live in-memory finalized lines for the active session (ignored otherwise). */
  liveLines: string[];
  /** Live interim drafts for the active session (ignored otherwise). */
  interimMe: string;
  interimThem: string;
  /**
   * Which session (if any) currently has a re-transcribe job running, owned
   * by App instead of local state (security-review fix, Task 4): a job can
   * outlive this component's mounted lifetime (switching sessions away and
   * back remounts it under a fresh `key={path}`, per the class doc comment
   * below), so "is a job running for MY path" has to be read from something
   * that survives that remount rather than reset to `false` by it. See
   * `RetranscribeJob`'s own doc comment for the full rationale.
   */
  retranscribingJob: RetranscribeJob;
  /**
   * Whether an enhance is currently running for THIS path -- owned by App
   * (its `enhance-progress` listener), for exactly the reason
   * `retranscribingJob` is: the backend job survives this component's
   * remount, so a local boolean would silently re-enable the control
   * mid-job. Drives the toolbar "Summarizing..." chip and disables Enhance
   * and re-run. Optional so tests exercising unrelated behavior can omit it.
   */
  enhanceRunning?: boolean;
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
   * Fires the instant `handleRetranscribe` starts a MANUAL job for this
   * `path`, before awaiting `retranscribeSession` (stage 0.9 Task 2 review
   * fix). App tracks this so its `retranscribe-progress` listener can tell a
   * manual click's failure (already shown inline here via
   * `retranscribeError`, right below) apart from an auto-triggered job's
   * failure, which has no inline display anywhere and needs the header
   * instead -- see App's `manualRetranscribePathsRef` doc comment for the
   * full rationale. Never called for the active session (the Re-transcribe
   * button only ever renders when `initialData.audio_path` is set, which is
   * never true while `isActive`).
   */
  onRetranscribeStart?: (path: string) => void;
  /**
   * Fires when a MANUAL job for this `path` ends WITHOUT ever producing a
   * terminal `retranscribe-progress` event (stage 0.9 Task 2, second review
   * round): `retranscribe_session` rejects up front, before emitting
   * anything, on several validation paths (not-active/finalizing check,
   * missing audio, retired/missing model, already-running/already-queued --
   * see `retranscribe.rs`'s doc comments), so `handleRetranscribe`'s catch
   * is the only signal App ever gets that this attempt is over. Without
   * this, `path` would stay marked "manual in flight" forever, permanently
   * suppressing the header for a LATER genuine auto-job failure on the same
   * session -- exactly the silence this whole fix exists to remove, just
   * deferred. Harmless to also fire when a terminal event DID already
   * arrive first (App's cleanup there is a plain `Set.delete`, a no-op on
   * an already-removed path), so this only needs to cover the no-event
   * case, not detect it.
   */
  onRetranscribeEnd?: (path: string) => void;
  transcriptFindRequest?: TranscriptFindRequest | null;
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
 * Renders one session: editable title, a debounced notes editor and the
 * transcript.
 *
 * App keys this component by path (`<SessionView key={path} .../>`), so
 * switching sessions fully unmounts the old instance and mounts a fresh one.
 * Title/notes state is seeded from `initialData` exactly once, on mount
 * (`useState(initialData.title)`), and is never re-seeded from later prop
 * changes -- the remount-on-key-change is what gives "reset when the session
 * changes, keep local edits otherwise" for free, without an effect that
 * would have to distinguish "the path changed" from "the same session's data
 * was refetched out from under an in-progress edit".
 *
 * For the active session specifically, `initialData` on any given mount is
 * NOT a fixed placeholder: App lifts the active session's title/notes into
 * its own state (updated live via `onActiveEdit`, called on every keystroke
 * and on the empty-title normalization) and passes that back in as
 * `initialData` on the next mount. Without this, switching away from the
 * active session and back would remount this component seeded with a stale
 * "Untitled"/"" placeholder instead of what was actually last typed, and the
 * very next keystroke would then debounce-save that near-empty state over
 * the real notes.
 *
 * Routing: while `isActive`, edits go through `update_active_session` --
 * the worker owns that file under the single-writer rule (see
 * `src-tauri/src/session.rs`). Once recording stops, `isActive` flips to
 * `false` on this same mounted instance in the ordinary case (App keeps
 * `selectedPath` steady across stop while this session stays selected), and
 * subsequent edits route through `save_session` instead. `isActiveRef` (not
 * the `isActive` prop) is read inside the notes-debounce timeout callback
 * and the unmount cleanup, kept in sync via an effect below.
 *
 * That said, the stop transition specifically does NOT go through an
 * isActive=false re-render of this same instance in App's actual JSX: App
 * renders the active session and an inactive session as different ternary
 * branches, and on stop the branch flips straight to a "loading" placeholder
 * (sessionData is reset to null first) before the real data streams back in.
 * React therefore unmounts this instance outright rather than re-rendering
 * it with isActive=false, so this component never observes the flip and
 * isActiveRef.current is still `true` in the unmount cleanup -- but by then
 * `stop_recording` may have already resolved and torn down the worker, so a
 * flush routed through `update_active_session` at that point can reject
 * with "not recording", silently losing whatever was still unsaved. That's
 * exactly what `flushPendingEdits` exists to close: App calls it explicitly,
 * awaited, BEFORE calling `stop_recording`, while this instance is still
 * mounted and the backend is still genuinely recording -- see App's
 * `handleStop`.
 *
 * Audio playback and re-transcribe (Task 4, stage-0.7): when
 * `initialData.audio_path` is set (never true while `isActive`, since App
 * only fills it in from a `read_session` disk read), an `<audio>` player and
 * a "Re-transcribe" button render above the transcript. Clicking a
 * transcript line seeks that player (`handleSeek`, wired through
 * `TranscriptView`'s `onSeek`); the button runs `retranscribeSession` and,
 * once it resolves, re-reads the file so the rewritten transcript appears
 * without a full remount (`transcript` state, seeded once like title/notes
 * above).
 *
 * "Is a job running, and its progress text" is deliberately NOT local state
 * here (security-review fix, Task 4): it's the `retranscribingJob` prop,
 * owned by App. A local boolean would reset to `false` on exactly the kind
 * of remount described above (switching away from this session and back
 * mid-job), silently re-enabling the button while the backend job (global,
 * one at a time, see `retranscribe.rs`) was still genuinely running. See
 * `RetranscribeJob`'s doc comment for the full rationale.
 */
export const SessionView = forwardRef<SessionViewHandle, SessionViewProps>(function SessionView(
  {
    path,
    isActive,
    initialData,
    liveLines,
    interimMe,
    interimThem,
    retranscribingJob,
    enhanceRunning = false,
    onTitleChange,
    onSaveError,
    onActiveEdit,
    onRetranscribeStart,
    onRetranscribeEnd,
    transcriptFindRequest,
  },
  ref
) {
  const [title, setTitle] = useState(initialData.title);
  const [notes, setNotes] = useState(initialData.notes);
  // Seeded once from initialData (same "reset on remount, live otherwise"
  // pattern as title/notes above), then updated in place after a successful
  // re-transcribe -- see handleRetranscribe. `initialData.audio_path` is
  // read directly from the prop rather than mirrored into its own state --
  // NOT because it's immutable across a re-transcribe (fix-review
  // correction, stage 0.9: `delete_audio_after_retranscribe` can remove the
  // audio file entirely once a pass succeeds), but because this component
  // never needs to react to that change live: a delete-after failure/success
  // doesn't change what this mounted instance renders for `audioPath` below
  // until the next remount (switching sessions away and back, or the
  // App-driven `reloadTick` remount after a background job) re-seeds it fresh
  // from a real `read_session` disk read.
  const [transcript, setTranscript] = useState(initialData.transcript);
  // Read straight from the mount's initialData: when an enhance finishes,
  // App refetches the session and remounts this component under a bumped
  // `reloadTick` key (its `enhance-progress` listener), so there is no
  // in-place refresh to track here anymore.
  const summary = initialData.summary;
  const summaryModel = initialData.summary_model;
  // Local-only optimistic flag for Enhance, same shape as `clicking` below:
  // covers the instants before the backend's `enhance-progress` "running"
  // event lands and flips the `enhanceRunning` prop.
  const [enhanceClicking, setEnhanceClicking] = useState(false);
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
  const notesInputRef = useRef<HTMLTextAreaElement>(null);
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

  // Saves the title if it changed since the last save, routed by whether
  // this session is currently the active recording. A no-op save (title
  // unchanged) is skipped so blurring without editing never fires an IPC
  // call. An emptied-out title is normalized to "Untitled" on commit (both
  // in the saved value and the displayed input) rather than left blank --
  // otherwise the sidebar would show an empty row for it. `save_session`
  // takes the full title+notes pair (it rewrites the whole file), so the
  // inactive branch sends the current notes alongside.
  //
  // `savedTitleRef` only advances once the IPC call actually resolves
  // (Critical fix-review finding): an earlier version set it eagerly,
  // before awaiting the result, so a rejected save (e.g. `save_session`
  // rejecting because this path just became the active recording, right as
  // App's `handleStart` flips `AppState` synchronously) was marked "saved"
  // anyway -- the edit was shown in the UI but silently never written, and
  // nothing would ever retry it since the next blur/debounce would see the
  // value already matching `savedTitleRef` and skip. Not advancing on
  // failure is itself the "revert": a later commit with the same value
  // will retry instead of no-op-ing.
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

  // Click-to-seek (Task 4): seeks the player to the clicked transcript
  // line's offset and starts playback. Only ever wired up by the JSX below
  // when `initialData.audio_path` is set, so `playerRef.current` being null
  // here would mean the player was unmounted out from under an in-flight
  // click -- defensive, not expected in practice.
  function handleSeek(offsetSecs: number) {
    playerRef.current?.seek(offsetSecs);
  }

  // Starts an offline re-transcribe of this session's audio and, once it
  // resolves, re-reads the file from disk so the new transcript (and the
  // updated `asr:`) show up without a full remount -- `retranscribeSession`
  // only rewrites the `## Transcript` section and frontmatter, it doesn't
  // push the new text back through an event, so a re-read is the only way
  // to pick it up. Progress text/"is running" in the meantime come from the
  // `retranscribingJob` prop (App's listener), not from local state -- see
  // the class doc comment above for why. `clicking` only covers the brief
  // window before that prop updates. Calls `onRetranscribeStart` right
  // before the await so App can record this as a MANUAL job -- see that
  // prop's own doc comment for why (its `retranscribe-progress` listener
  // needs to tell this apart from an auto-triggered job on failure).
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
      // Minor fix-review finding, stage 0.9 final review: the "transcript
      // updated, but the audio could not be deleted" rejection is the one
      // case where the promise rejects even though the transcript itself WAS
      // rewritten -- `maybe_delete_audio`'s own failure surfaces through the
      // same "error" phase/rejection as a genuine pipeline failure, but only
      // the deletion step afterward actually failed. Without refetching
      // here too, this on-screen transcript stays stale (the success path
      // above never runs, since the promise rejected).
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

  // Enhance (stage 0.11): the user's rough notes lead, the transcript backs
  // them, and the configured provider writes the result into `## Summary`.
  // Flushes pending edits FIRST so what the model reads from disk is what's
  // on screen. Progress, the refreshed summary and any failure all flow
  // through the backend's `enhance-progress` events (App's listener), which
  // this command also emits for its own run -- so the rejection here is
  // deliberately not shown a second time.
  async function handleEnhance() {
    if (isActive || enhanceBusy) return;
    setEnhanceClicking(true);
    try {
      await commitNotes();
      await commitTitle();
      await enhanceSession(path);
    } catch (err) {
      console.error("failed to enhance session", err);
    } finally {
      setEnhanceClicking(false);
    }
  }

  function handleTitleChange(e: React.ChangeEvent<HTMLInputElement>) {
    const value = e.target.value;
    setTitle(value);
    titleRef.current = value;
    onActiveEdit?.(titleRef.current, notesRef.current);
  }

  // Enter blurs the input, which triggers the save via onBlur below.
  function handleTitleKeyDown(e: React.KeyboardEvent<HTMLInputElement>) {
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

  const titleInput = (
    <input
      value={title}
      onChange={handleTitleChange}
      onKeyDown={handleTitleKeyDown}
      onBlur={commitTitle}
      placeholder="Untitled"
      className="w-full min-w-0 bg-transparent font-serif text-[26px] font-semibold leading-[1.25] tracking-[-0.01em] outline-none placeholder:text-muted-foreground"
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
      <span className="shrink-0 font-mono text-[10px] text-muted-foreground" aria-live="polite">
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
  // -- the same slot shows the indeterminate work chip, so processing is
  // visible from every tab. Once a summary exists the pill retires; re-runs
  // live in the Summary tab's provenance line.
  const enhanceControl = enhanceBusy ? (
    <span
      data-testid="enhance-working"
      className="ml-3 inline-flex h-[30px] shrink-0 items-center gap-2 rounded-full border border-input bg-foreground/[0.04] px-[15px] font-mono text-[11px] font-medium tracking-[0.04em] text-muted-foreground"
    >
      <span aria-hidden="true" className="working-spinner size-3.5" />
      Summarizing...
    </span>
  ) : canEnhance && !summary ? (
    <Button
      type="button"
      variant="outline"
      size="sm"
      className="ml-3 h-[30px] shrink-0 gap-[7px] rounded-full px-[14px] font-mono text-[11px] font-medium tracking-[0.04em]"
      onClick={handleEnhance}
    >
      <Sparkles className="size-3.5 text-[var(--voice-remote)]" />
      Enhance
    </Button>
  ) : null;

  const tabList = (
    <TabsList ref={tabListRef} className="h-[34px] shrink-0 gap-[3px] rounded-full bg-foreground/[0.06] p-[3px] dark:bg-foreground/[0.07]">
      <TabsTrigger
        className="h-7 gap-[7px] rounded-full px-[13px] font-mono text-[11px] font-normal hover:bg-foreground/[0.05] data-[state=active]:bg-background data-[state=active]:font-semibold data-[state=active]:tracking-[0.04em] data-[state=active]:shadow-[0_1px_2px_rgb(30_27_22/0.14)] dark:data-[state=active]:bg-foreground/[0.14]"
        value="notes"
      >
        Notes
      </TabsTrigger>
      {summary && (
        <TabsTrigger
          className="h-7 gap-[7px] rounded-full px-[13px] font-mono text-[11px] font-normal hover:bg-foreground/[0.05] data-[state=active]:bg-background data-[state=active]:font-semibold data-[state=active]:tracking-[0.04em] data-[state=active]:shadow-[0_1px_2px_rgb(30_27_22/0.14)] dark:data-[state=active]:bg-foreground/[0.14]"
          value="summary"
        >
          Summary
        </TabsTrigger>
      )}
      <TabsTrigger
        className="h-7 gap-[7px] rounded-full px-[13px] font-mono text-[11px] font-normal hover:bg-foreground/[0.05] data-[state=active]:bg-background data-[state=active]:font-semibold data-[state=active]:tracking-[0.04em] data-[state=active]:shadow-[0_1px_2px_rgb(30_27_22/0.14)] dark:data-[state=active]:bg-foreground/[0.14]"
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
        <div className="shrink-0 px-6 pt-0.5">{titleInput}</div>

        <TabsContent
          value="notes"
          forceMount
          className="flex min-h-0 flex-1 flex-col gap-3 data-[state=inactive]:hidden"
        >
          <textarea
            ref={notesInputRef}
            value={notes}
            onChange={handleNotesChange}
            onBlur={commitNotes}
            placeholder="your notes..."
            className={cn(
              "min-h-0 w-full flex-1 resize-none border-0 bg-transparent px-6 py-4 font-serif text-[15px] leading-[1.65] text-card-foreground outline-none placeholder:font-serif placeholder:text-muted-foreground focus-visible:ring-0"
            )}
          />

        </TabsContent>

        {summary && (
          <TabsContent
            value="summary"
            forceMount
            className="min-h-0 flex-1 overflow-y-auto px-6 py-4 data-[state=inactive]:hidden"
          >
            <div className="border-l-2 border-[var(--voice-remote)] pl-5">
              {summaryModel && (
                <p className="mb-4 font-mono text-[10px] uppercase tracking-[0.08em] text-muted-foreground">
                  by {summaryModel}
                  {canEnhance && (
                    <>
                      {" · "}
                      {/* The regenerate affordance is plain text in the
                          provenance line (design 3b): quiet, next to what it
                          replaces. The toolbar chip shows the running state. */}
                      <button
                        type="button"
                        disabled={enhanceBusy}
                        onClick={handleEnhance}
                        className="font-mono uppercase tracking-[0.08em] underline-offset-2 hover:text-foreground hover:underline disabled:pointer-events-none disabled:opacity-50"
                      >
                        {enhanceBusy ? "summarizing..." : "re-run"}
                      </button>
                    </>
                  )}
                </p>
              )}
              <div className="whitespace-pre-wrap font-serif text-[14.5px] leading-[1.65] text-card-foreground">
                {summary}
              </div>
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
          <TranscriptView
            lines={transcriptLines}
            interimMe={isActive ? interimMe : undefined}
            interimThem={isActive ? interimThem : undefined}
            onSeek={audioPath ? handleSeek : undefined}
            matches={transcriptMatches}
            activeMatchIndex={findOpen ? activeFindIndex : -1}
          />

          {retranscribeStatusText && (
            <p className="shrink-0 px-6 font-mono text-[10.5px] text-muted-foreground">{retranscribeStatusText}</p>
          )}
          {retranscribeError && (
            <p className="shrink-0 text-sm text-destructive">{retranscribeError}</p>
          )}

          {/* Pinned under the transcript as one compact, stackable row. */}
          {audioPath && (
            <AudioPlayer
              ref={playerRef}
              src={convertFileSrc(audioPath)}
              trailing={
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  className="shrink-0"
                  disabled={isActive || retranscribeBusy}
                  onClick={handleRetranscribe}
                >
                  {retranscribeBusy ? "Re-transcribing..." : "Re-transcribe"}
                </Button>
              }
            />
          )}
        </TabsContent>
      </Tabs>
    </div>
  );
});
