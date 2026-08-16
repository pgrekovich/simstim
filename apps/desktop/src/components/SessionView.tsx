import { forwardRef, useEffect, useImperativeHandle, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Button } from "@/components/ui/button";
import { TranscriptView } from "@/components/TranscriptView";
import {
  readSession,
  retranscribeSession,
  saveSession,
  updateActiveSession,
  type RetranscribeProgress,
  type SessionData,
} from "@/lib/ipc";

/**
 * Renders a `retranscribe-progress` event's phase into the short status text
 * shown next to the re-transcribe button (Task 4). `"done"`/`"error"` never
 * actually reach the screen in practice -- `handleRetranscribe` below clears
 * the status text once the `retranscribe_session` promise itself settles,
 * which happens at essentially the same moment those two phases fire -- but
 * both are still handled so an event arriving in an unexpected order (or a
 * future phase reordering) never renders `undefined`.
 */
function describeRetranscribeProgress(payload: RetranscribeProgress): string {
  switch (payload.phase) {
    case "loading-model":
      return "loading model...";
    case "transcribing": {
      const who = payload.channel ? ` (${payload.channel})` : "";
      const detail = payload.message ? ` ${payload.message}` : "";
      return `transcribing${who}${detail}...`;
    }
    case "writing":
      return "writing...";
    case "done":
      return "done";
    case "error":
      return payload.message ?? "re-transcribe failed";
  }
}

/** Global Constraints (Task 6): notes save 500ms after the last keystroke. */
const NOTES_DEBOUNCE_MS = 500;

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
}

/**
 * Imperative escape hatch for the one case a keyed remount's unmount
 * cleanup can't handle correctly on its own: see the `flushPendingEdits`
 * doc comment below.
 */
export interface SessionViewHandle {
  flushPendingEdits: () => Promise<void>;
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
 * above). Progress text comes from `retranscribe-progress` events filtered
 * to this session's `path`.
 */
export const SessionView = forwardRef<SessionViewHandle, SessionViewProps>(function SessionView(
  {
    path,
    isActive,
    initialData,
    liveLines,
    interimMe,
    interimThem,
    onTitleChange,
    onSaveError,
    onActiveEdit,
  },
  ref
) {
  const [title, setTitle] = useState(initialData.title);
  const [notes, setNotes] = useState(initialData.notes);
  // Seeded once from initialData (same "reset on remount, live otherwise"
  // pattern as title/notes above), then updated in place after a successful
  // re-transcribe -- see handleRetranscribe. `initialData.audio_path` itself
  // never changes across a re-transcribe (same audio file, only the text
  // and `asr:` change), so it's read directly from the prop rather than
  // mirrored into its own state.
  const [transcript, setTranscript] = useState(initialData.transcript);
  const [retranscribing, setRetranscribing] = useState(false);
  const [retranscribeText, setRetranscribeText] = useState<string | null>(null);
  const [retranscribeError, setRetranscribeError] = useState<string | null>(null);
  const audioRef = useRef<HTMLAudioElement>(null);

  // "Latest value" mirrors of the controlled state above, read from
  // callbacks (blur handlers, the debounce timeout, the unmount cleanup)
  // that must never close over a stale render's value.
  const titleRef = useRef(title);
  const notesRef = useRef(notes);
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

  // Listens for this session's own `retranscribe-progress` events (filtered
  // by `path`, since the backend's job guard is global -- see
  // `retranscribe.rs`'s module docs -- so a different session's job could in
  // principle emit while this one is mounted). Kept live for the whole
  // mounted lifetime of this instance rather than only while a job started
  // from this instance is running, so progress is still visible if the user
  // switches away and back to a session whose re-transcribe (started from
  // elsewhere, or before a remount) is still in flight.
  useEffect(() => {
    const unlisten = listen<RetranscribeProgress>("retranscribe-progress", (event) => {
      if (event.payload.path !== path) return;
      setRetranscribeText(describeRetranscribeProgress(event.payload));
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, [path]);

  // Click-to-seek (Task 4): seeks the audio element to the clicked
  // transcript line's offset and starts playback. Only ever wired up by the
  // JSX below when `initialData.audio_path` is set, so `audioRef.current`
  // being null here would mean the player was unmounted out from under an
  // in-flight click -- defensive, not expected in practice.
  function handleSeek(offsetSecs: number) {
    const audio = audioRef.current;
    if (!audio) return;
    audio.currentTime = offsetSecs;
    void audio.play();
  }

  // Starts an offline re-transcribe of this session's audio and, once it
  // resolves, re-reads the file from disk so the new transcript (and the
  // updated `asr:`) show up without a full remount -- `retranscribeSession`
  // only rewrites the `## Transcript` section and frontmatter, it doesn't
  // push the new text back through an event, so a re-read is the only way
  // to pick it up. Progress text in the meantime comes from the
  // `retranscribe-progress` listener above, not from this function.
  async function handleRetranscribe() {
    if (!initialData.audio_path || isActive || retranscribing) return;
    setRetranscribing(true);
    setRetranscribeError(null);
    setRetranscribeText("starting...");
    try {
      await retranscribeSession(path);
      const data = await readSession(path);
      setTranscript(data.transcript);
    } catch (err) {
      console.error("failed to re-transcribe session", err);
      setRetranscribeError(String(err));
    } finally {
      setRetranscribing(false);
      setRetranscribeText(null);
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

  const transcriptLines = isActive
    ? liveLines
    : transcript.split("\n").filter((line) => line.length > 0);
  const audioPath = initialData.audio_path;

  return (
    <div className="flex h-full min-h-0 flex-col gap-4">
      <div className="flex shrink-0 items-center gap-2">
        <input
          value={title}
          onChange={handleTitleChange}
          onKeyDown={handleTitleKeyDown}
          onBlur={commitTitle}
          placeholder="Untitled"
          className="min-w-0 flex-1 bg-transparent text-xl font-semibold outline-none placeholder:text-muted-foreground"
        />

        {audioPath && (
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="shrink-0"
            disabled={isActive || retranscribing}
            onClick={handleRetranscribe}
          >
            {retranscribing ? "Re-transcribing..." : "Re-transcribe"}
          </Button>
        )}
      </div>

      {audioPath && (
        <audio
          ref={audioRef}
          controls
          preload="metadata"
          src={convertFileSrc(audioPath)}
          className="h-10 w-full shrink-0"
        />
      )}

      {retranscribeText && (
        <p className="shrink-0 text-xs text-muted-foreground">{retranscribeText}</p>
      )}
      {retranscribeError && (
        <p className="shrink-0 text-sm text-destructive">{retranscribeError}</p>
      )}

      <textarea
        value={notes}
        onChange={handleNotesChange}
        onBlur={commitNotes}
        placeholder="your notes..."
        className="min-h-32 w-full shrink-0 resize-y rounded-md border bg-transparent p-3 text-sm outline-none placeholder:text-muted-foreground focus-visible:ring-[3px] focus-visible:ring-ring/50"
      />

      <TranscriptView
        lines={transcriptLines}
        interimMe={isActive ? interimMe : undefined}
        interimThem={isActive ? interimThem : undefined}
        onSeek={audioPath ? handleSeek : undefined}
      />
    </div>
  );
});
