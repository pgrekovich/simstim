import { Button } from "@/components/ui/button";

interface EmptyStateProps {
  onNewNote: () => void;
  onStartRecording: () => void;
  onOpenSettings: () => void;
  /** Mirrors the Record button's gating: the hero renders but stays inert. */
  recordingDisabled: boolean;
  /** Explains why Start Recording is disabled without becoming a global notice. */
  recordingHint?: string;
}

function Keys({ chars }: { chars: string[] }) {
  return (
    <span className="flex shrink-0 gap-1">
      {chars.map((c) => (
        <kbd
          key={c}
          className="flex h-5 min-w-5 items-center justify-center rounded-[4px] border border-border bg-foreground/[0.06] px-1.5 text-[11px] tabular-nums text-muted-foreground"
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
}: {
  label: string;
  keys: string[];
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="flex w-full items-center justify-between gap-8 rounded-[7px] px-3 py-2 text-left text-quiet-foreground transition-colors duration-150 hover:bg-foreground/[0.06] hover:text-foreground active:bg-foreground/[0.10]"
    >
      <span className="text-[12.5px]">{label}</span>
      <Keys chars={keys} />
    </button>
  );
}

/**
 * Empty main pane with recording, new-note and settings actions. Keyboard
 * shortcuts are handled in App.
 *
 */
export function EmptyState({
  onNewNote,
  onStartRecording,
  onOpenSettings,
  recordingDisabled,
  recordingHint,
}: EmptyStateProps) {
  return (
    <div className="flex h-full items-center justify-center">
      <div className="flex w-80 flex-col items-center">
        <Button
          type="button"
          onClick={onStartRecording}
          disabled={recordingDisabled}
          className="h-12 rounded-[12px] bg-primary px-8 text-[14px] font-semibold text-primary-foreground shadow-none hover:bg-primary/90"
        >
          <span aria-hidden="true" className="size-2 rounded-full bg-destructive" />
          Start Recording
        </Button>
        <p className="mt-3 text-[11px] tabular-nums text-muted-foreground">
          {recordingHint ?? "⌘⇧N · your call becomes a note"}
        </p>
        <div className="mt-8 w-full rounded-xl border border-border bg-card/45 p-2 shadow-[0_8px_28px_rgb(0_0_0/0.05)]">
          <Row label="New Note" keys={["⌘", "N"]} onClick={onNewNote} />
          <Row label="Settings" keys={["⌘", ","]} onClick={onOpenSettings} />
        </div>
      </div>
    </div>
  );
}
