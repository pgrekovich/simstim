import { useEffect, useRef, useState } from "react";
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
 * Routing: while `isActive`, edits go through `update_active_session` --
 * the worker owns that file under the single-writer rule (see
 * `src-tauri/src/session.rs`). Once recording stops, `isActive` flips to
 * `false` on this same mounted instance (App keeps `selectedPath` steady
 * across stop), and subsequent edits route through `save_session` instead.
 * `isActiveRef` (not the `isActive` prop) is read inside the notes-debounce
 * timeout callback, so an edit typed a moment before stop still routes
 * correctly by the time the 500ms timer actually fires.
 */
export function SessionView({
  path,
  isActive,
  initialData,
  liveLines,
  interimMe,
  interimThem,
  onTitleChange,
}: SessionViewProps) {
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
  isActiveRef.current = isActive;
  const notesTimeoutRef = useRef<number | null>(null);

  // Saves the title if it changed since the last save, routed by whether
  // this session is currently the active recording. A no-op save (title
  // unchanged) is skipped so blurring without editing never fires an IPC
  // call. `save_session` takes the full title+notes pair (it rewrites the
  // whole file), so the inactive branch sends the current notes alongside.
  function commitTitle() {
    const value = titleRef.current;
    if (value === savedTitleRef.current) return;
    savedTitleRef.current = value;
    const result = isActiveRef.current
      ? updateActiveSession(value, undefined)
      : saveSession(path, value, notesRef.current);
    result
      .then(() => onTitleChange?.())
      .catch(() => {
        // Active-session failures already surface via the worker's
        // `session-error` event (App listens for it); an inactive-session
        // save failure here is rare (e.g. disk full) and not otherwise
        // actionable from this component.
      });
  }

  // Same shape as `commitTitle` for notes. Also clears any pending debounce
  // timer, so it's safe to call this directly (blur, unmount) without a
  // duplicate save firing later.
  function commitNotes() {
    if (notesTimeoutRef.current !== null) {
      window.clearTimeout(notesTimeoutRef.current);
      notesTimeoutRef.current = null;
    }
    const value = notesRef.current;
    if (value === savedNotesRef.current) return;
    savedNotesRef.current = value;
    const result = isActiveRef.current
      ? updateActiveSession(undefined, value)
      : saveSession(path, titleRef.current, value);
    result.catch(() => {
      // See commitTitle.
    });
  }

  // Flushes both pending edits when this exact instance goes away: the app
  // closing, or (the common case) the user switching to a different
  // session, which remounts this component under a new `key={path}` and
  // runs this cleanup for the outgoing instance first. Deliberately an
  // empty dep array: `commitTitle`/`commitNotes` only ever read from refs
  // and the `path` prop, which is fixed for the lifetime of this
  // keyed instance, so the closures captured at mount stay correct.
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
}
