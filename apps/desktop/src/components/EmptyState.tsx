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
          className="flex h-5 min-w-5 items-center justify-center rounded-[4px] border border-border bg-foreground/[0.06] px-1.5 font-mono text-[10.5px] text-muted-foreground"
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
 * The main pane when nothing is selected. One thing on the screen: the
 * action the app exists for (2026-09-04 ruling: the old shortcut card hid
 * Record behind a menu row). The other two actions stay as quiet rows
 * underneath, doubling as the shortcut cheat sheet; the real key handling
 * lives in App's keydown listener.
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
          className="h-12 rounded-full bg-foreground px-8 font-mono text-sm font-semibold tracking-[0.02em] text-background shadow-none hover:bg-foreground/90"
        >
          <span aria-hidden="true" className="size-2 rounded-full bg-destructive" />
          Start Recording
        </Button>
        <p className="mt-3 font-mono text-[10.5px] text-muted-foreground">
          {recordingHint ?? "⌘⇧N · your call becomes a note"}
        </p>
        <div className="mt-8 w-full rounded-xl border border-border bg-card/45 p-2 shadow-[0_8px_28px_rgb(30_27_22/0.05)]">
          <Row label="New Note" keys={["⌘", "N"]} onClick={onNewNote} />
          <Row label="Settings" keys={["⌘", ","]} onClick={onOpenSettings} />
        </div>
      </div>
    </div>
  );
}
