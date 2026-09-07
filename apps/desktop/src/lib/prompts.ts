import { useCallback, useEffect, useState } from "react";
import {
  getPrompts,
  type Prompts,
  type QuickQuestion,
  type QuickScope,
  type SummaryStyle,
} from "@/lib/ipc";

/** The reserved id of the built-in summary prompt (mirrors `prompts::STANDARD_STYLE`). */
export const STANDARD_STYLE = "standard";

/** What the built-in prompt is called wherever styles are listed. */
export const STANDARD_STYLE_NAME = "Standard";

/** An empty document: what surfaces render until the file has been read. */
export const EMPTY_PROMPTS: Prompts = {
  summary_instructions: "",
  summary_styles: [],
  quick_questions: [],
};

/**
 * Fired on `window` after Settings saves the prompts, so every mounted
 * surface (the Summary tab's style menu, the chat's quick questions)
 * refetches without a prop chain from App. One process, one file, one
 * event: the file stays the source of truth.
 */
export const PROMPTS_CHANGED_EVENT = "simstim:prompts-changed";

export function notifyPromptsChanged(): void {
  window.dispatchEvent(new Event(PROMPTS_CHANGED_EVENT));
}

/**
 * The user's prompts, read on mount and again whenever Settings saves them.
 * `error` carries a broken hand edit's parse error (the surfaces then show
 * no styles or questions, and Settings shows the message).
 */
export function usePrompts(): { prompts: Prompts; error: string | null; reload: () => void } {
  const [prompts, setPrompts] = useState<Prompts>(EMPTY_PROMPTS);
  const [error, setError] = useState<string | null>(null);
  const reload = useCallback(() => {
    getPrompts()
      .then((fresh) => {
        setPrompts(fresh);
        setError(null);
      })
      .catch((err: unknown) => {
        setPrompts(EMPTY_PROMPTS);
        setError(err instanceof Error ? err.message : String(err));
      });
  }, []);
  useEffect(() => {
    reload();
    window.addEventListener(PROMPTS_CHANGED_EVENT, reload);
    return () => window.removeEventListener(PROMPTS_CHANGED_EVENT, reload);
  }, [reload]);
  return { prompts, error, reload };
}

/** The quick questions offered in a chat scope: its own plus the ones marked "both". */
export function quickQuestionsFor(prompts: Prompts, scope: "call" | "notes"): QuickQuestion[] {
  return prompts.quick_questions.filter((q) => q.scope === scope || q.scope === "both");
}

/** The user-facing name of a summary style id, "Standard" for the built-in prompt or `null`. */
export function styleName(styles: SummaryStyle[], id: string | null | undefined): string {
  if (!id || id === STANDARD_STYLE) return STANDARD_STYLE_NAME;
  return styles.find((style) => style.id === id)?.name ?? id;
}

/**
 * An id for a new prompt from its name ("Make me smart" -> "make-me-smart"),
 * made unique against `taken` with a numeric suffix. Ids are the file's
 * stable handles (a note remembers its style by id), so they never change
 * once created, even when the name is edited.
 */
export function slugFor(name: string, taken: Iterable<string>): string {
  const base =
    name
      .toLowerCase()
      .normalize("NFKD")
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .slice(0, 40) || "prompt";
  const used = new Set(taken);
  used.add(STANDARD_STYLE);
  if (!used.has(base)) return base;
  let n = 2;
  while (used.has(`${base}-${n}`)) n += 1;
  return `${base}-${n}`;
}

/** The scope labels shown in Settings, in the order they are offered. */
export const QUICK_SCOPE_OPTIONS: { value: QuickScope; label: string }[] = [
  { value: "call", label: "This call" },
  { value: "notes", label: "All notes" },
  { value: "both", label: "Both" },
];
