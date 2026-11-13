import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";

/** What the one-off prompt is called wherever styles are listed. */
export const CUSTOM_PROMPT_NAME = "Custom prompt";

interface SummaryPromptDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** The one-off prompt that wrote the note's current summary, to tweak and rerun. */
  initialPrompt: string | null;
  /** Called with the trimmed, non-empty prompt; the dialog closes itself. */
  onSubmit: (prompt: string) => void;
}

/**
 * A prompt for this note's summary alone: typed here, used once, never
 * saved as a style. The note remembers it (`summary_prompt:` frontmatter),
 * so reopening the dialog starts from the prompt that wrote the summary.
 */
export function SummaryPromptDialog({ open, onOpenChange, initialPrompt, onSubmit }: SummaryPromptDialogProps) {
  const [prompt, setPrompt] = useState(initialPrompt ?? "");
  const fieldRef = useRef<HTMLTextAreaElement>(null);
  const ready = prompt.trim().length > 0;

  // Every opening starts from the note's own prompt, not from a draft
  // abandoned on another note.
  useEffect(() => {
    if (open) setPrompt(initialPrompt ?? "");
  }, [open, initialPrompt]);

  function submit() {
    if (!ready) return;
    onOpenChange(false);
    onSubmit(prompt.trim());
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        showCloseButton={false}
        overlayClassName="!bg-foreground/30"
        className="w-[480px] gap-3 rounded-xl border-border bg-popover p-5 sm:max-w-[480px]"
        onOpenAutoFocus={(event) => {
          event.preventDefault();
          const field = fieldRef.current;
          if (!field) return;
          field.focus();
          field.setSelectionRange(field.value.length, field.value.length);
        }}
      >
        <DialogHeader className="gap-1">
          <DialogTitle className="font-serif text-[17px] font-semibold tracking-[-0.005em]">
            Summarize with a custom prompt
          </DialogTitle>
          <DialogDescription className="text-[12px]">
            Used for this note only. Prompts you reuse belong in Settings as styles.
          </DialogDescription>
        </DialogHeader>
        <textarea
          ref={fieldRef}
          aria-label="Custom prompt"
          value={prompt}
          rows={5}
          placeholder="How should this summary be written? What to extract, in what shape."
          onChange={(event) => setPrompt(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
              event.preventDefault();
              submit();
            }
          }}
          className="w-full resize-y rounded-[7px] border border-input bg-background px-2.5 py-1.5 text-[13px] leading-[1.5] outline-none placeholder:text-muted-foreground focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/40"
        />
        <DialogFooter>
          <Button variant="ghost" size="sm" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button size="sm" disabled={!ready} onClick={submit}>
            Summarize
            <kbd className="font-sans text-[11px] opacity-70">⌘↵</kbd>
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
