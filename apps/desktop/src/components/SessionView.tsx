import { forwardRef, useEffect, useImperativeHandle, useRef, useState } from "react";
import { TranscriptView } from "@/components/TranscriptView";
import { saveSession, updateActiveSession, type SessionData } from "@/lib/ipc";

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
  function commitTitle(): Promise<void> {
    const rawValue = titleRef.current;
    const value = rawValue.trim().length > 0 ? rawValue : "Untitled";
    if (value !== rawValue) {
      setTitle(value);
      titleRef.current = value;
      onActiveEdit?.(titleRef.current, notesRef.current);
    }
    if (value === savedTitleRef.current) return Promise.resolve();
    savedTitleRef.current = value;
    const result = isActiveRef.current
      ? updateActiveSession(value, undefined)
      : saveSession(path, value, notesRef.current);
    return result
      .then(() => {
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

  // Same shape as `commitTitle` for notes. Also clears any pending debounce
  // timer, so it's safe to call this directly (blur, unmount, explicit
  // flush) without a duplicate save firing later.
  function commitNotes(): Promise<void> {
    if (notesTimeoutRef.current !== null) {
      window.clearTimeout(notesTimeoutRef.current);
      notesTimeoutRef.current = null;
    }
    const value = notesRef.current;
    if (value === savedNotesRef.current) return Promise.resolve();
    savedNotesRef.current = value;
    const result = isActiveRef.current
      ? updateActiveSession(undefined, value)
      : saveSession(path, titleRef.current, value);
    return result.catch((err) => {
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
    : initialData.transcript.split("\n").filter((line) => line.length > 0);

  return (
    <div className="flex h-full min-h-0 flex-col gap-4">
      <input
        value={title}
        onChange={handleTitleChange}
        onKeyDown={handleTitleKeyDown}
        onBlur={commitTitle}
        placeholder="Untitled"
        className="w-full shrink-0 bg-transparent text-xl font-semibold outline-none placeholder:text-muted-foreground"
      />

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
      />
    </div>
  );
});
