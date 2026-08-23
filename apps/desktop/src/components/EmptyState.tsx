interface EmptyStateProps {
  onNewNote: () => void;
  onStartRecording: () => void;
  onOpenSettings: () => void;
  /** Mirrors the Record button's gating: the row renders but stays inert. */
  recordingDisabled: boolean;
  /**
   * App's status line ("model not downloaded", a delete error, ...): the
   * topbar that normally shows it is hidden on this screen, so it surfaces
   * here instead of disappearing.
   */
  notice?: string;
  noticeIsError?: boolean;
}

function Keys({ chars }: { chars: string[] }) {
  return (
    <span className="flex shrink-0 gap-1">
      {chars.map((c) => (
        <kbd
          key={c}
          className="flex h-6 min-w-6 items-center justify-center rounded-md border bg-muted px-1.5 font-sans text-xs text-muted-foreground"
        >
          {c}
        </kbd>
      ))}
    </span>
  );
}

function Row({
  label,
  keys,
  onClick,
  disabled = false,
}: {
  label: string;
  keys: string[];
  onClick: () => void;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      className="flex w-full cursor-pointer items-center justify-between gap-8 rounded-md px-3 py-2.5 text-left transition-colors duration-150 hover:bg-foreground/[0.04] active:bg-foreground/[0.07] disabled:cursor-not-allowed disabled:opacity-50"
    >
      <span className="text-sm">{label}</span>
      <Keys chars={keys} />
    </button>
  );
}

/**
 * The main pane when nothing is selected (stage 0.10): the three primary
 * actions with their shortcuts, doubling as the app's shortcut cheat sheet.
 * The real key handling lives in App's global keydown listener; these rows
 * click through to the same handlers.
 */
export function EmptyState({
  onNewNote,
  onStartRecording,
  onOpenSettings,
  recordingDisabled,
  notice,
  noticeIsError = false,
}: EmptyStateProps) {
  return (
    <div className="flex h-full items-center justify-center">
      <div className="w-80">
        <Row label="New Note" keys={["⌘", "N"]} onClick={onNewNote} />
        <Row
          label="Start Recording"
          keys={["⌘", "⇧", "N"]}
          onClick={onStartRecording}
          disabled={recordingDisabled}
        />
        <div className="my-2 border-t" />
        <Row label="Settings" keys={["⌘", ","]} onClick={onOpenSettings} />
        {notice && (
          <p
            className={
              noticeIsError
                ? "mt-6 px-3 text-center text-xs text-destructive"
                : "mt-6 px-3 text-center text-xs text-muted-foreground"
            }
          >
            {notice}
          </p>
        )}
      </div>
    </div>
  );
}
