import { useEffect, useRef, useState, type ReactNode } from "react";
import { ask } from "@tauri-apps/plugin-dialog";
import { ChevronDown, ChevronRight, Plus } from "lucide-react";
import { cn } from "@/lib/utils";
import {
  getPrompts,
  restoreDefaultPrompts,
  savePrompts,
  type Prompts,
  type QuickScope,
} from "@/lib/ipc";
import { QUICK_SCOPE_OPTIONS, notifyPromptsChanged, slugFor } from "@/lib/prompts";

/** Edits save this long after the last keystroke (same feel as notes). */
const SAVE_DEBOUNCE_MS = 400;

/**
 * One row of either list, tracked by a local `key` so a brand-new row (no
 * id until its first save) keeps its identity while the user types.
 */
interface Row {
  key: number;
  id: string;
  name: string;
  prompt: string;
  /** Quick questions only. */
  scope: QuickScope;
  /** Quick questions only: run with the user's connectors (Jira, calendar, ...). */
  connectors: boolean;
}

interface Doc {
  instructions: string;
  styles: Row[];
  questions: Row[];
}

let nextKey = 1;

function toDoc(prompts: Prompts): Doc {
  return {
    instructions: prompts.summary_instructions,
    styles: prompts.summary_styles.map((s) => ({ key: nextKey++, id: s.id, name: s.name, prompt: s.prompt, scope: "call", connectors: false })),
    questions: prompts.quick_questions.map((q) => ({ key: nextKey++, id: q.id, name: q.name, prompt: q.prompt, scope: q.scope, connectors: q.connectors })),
  };
}

/**
 * What gets written: rows still missing a name or a prompt are skipped (they
 * stay on screen until filled in), and a new row gets its id from its name
 * on this first save. Returns the document plus the ids assigned, so the
 * caller can stamp them onto the local rows.
 */
export function toPrompts(doc: Doc): { prompts: Prompts; assigned: Map<number, string> } {
  const assigned = new Map<number, string>();
  function ready(rows: Row[]): Row[] {
    const taken = rows.map((r) => r.id).filter((id) => id !== "");
    return rows
      .filter((r) => r.name.trim() !== "" && r.prompt.trim() !== "")
      .map((r) => {
        if (r.id !== "") return r;
        const id = slugFor(r.name, taken);
        taken.push(id);
        assigned.set(r.key, id);
        return { ...r, id };
      });
  }
  const styles = ready(doc.styles);
  const questions = ready(doc.questions);
  return {
    prompts: {
      summary_instructions: doc.instructions,
      summary_styles: styles.map((r) => ({ id: r.id, name: r.name.trim(), prompt: r.prompt.trim() })),
      quick_questions: questions.map((r) => ({
        id: r.id,
        name: r.name.trim(),
        prompt: r.prompt.trim(),
        scope: r.scope,
        connectors: r.connectors,
      })),
    },
    assigned,
  };
}

/** The first line of a prompt, for the collapsed row. */
function firstLine(prompt: string): string {
  return prompt.trim().split("\n")[0] ?? "";
}

function GroupHeading({ children, right }: { children: ReactNode; right?: ReactNode }) {
  return (
    <div className="flex items-baseline justify-between gap-2">
      <h4 className="text-[11px] font-semibold text-muted-foreground">{children}</h4>
      {right && <span className="text-[11px] tabular-nums text-muted-foreground">{right}</span>}
    </div>
  );
}

const FIELD_CLASS =
  "w-full rounded-[7px] border border-input bg-background px-2.5 py-1.5 text-[13px] leading-[1.5] outline-none placeholder:text-muted-foreground focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/40";

/**
 * Edit summary instructions, styles and quick questions. Save the complete
 * prompts.toml automatically.
 *
 */
export function PromptsSettings({ open }: { open: boolean }) {
  const [doc, setDoc] = useState<Doc | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<number | null>(null);
  const saveTimer = useRef<number | null>(null);
  const latest = useRef<Doc | null>(null);

  function load() {
    getPrompts()
      .then((prompts) => {
        setDoc(toDoc(prompts));
        setLoadError(null);
      })
      .catch((err: unknown) => {
        setDoc(null);
        setLoadError(err instanceof Error ? err.message : String(err));
      });
  }

  // Re-read on every open: the file may have been hand-edited meanwhile.
  useEffect(() => {
    if (open) load();
  }, [open]);

  useEffect(() => {
    latest.current = doc;
  }, [doc]);

  function persist(next: Doc) {
    const { prompts, assigned } = toPrompts(next);
    savePrompts(prompts)
      .then(() => {
        setSaveError(null);
        if (assigned.size > 0) {
          setDoc((current) =>
            current === null
              ? current
              : {
                  ...current,
                  styles: current.styles.map((r) => (assigned.has(r.key) ? { ...r, id: assigned.get(r.key)! } : r)),
                  questions: current.questions.map((r) =>
                    assigned.has(r.key) ? { ...r, id: assigned.get(r.key)! } : r,
                  ),
                },
          );
        }
        notifyPromptsChanged();
      })
      .catch((err: unknown) => setSaveError(err instanceof Error ? err.message : String(err)));
  }

  /** Applies an edit locally and schedules the save. */
  function update(mutate: (current: Doc) => Doc, immediate = false) {
    setDoc((current) => {
      if (current === null) return current;
      const next = mutate(current);
      latest.current = next;
      if (saveTimer.current !== null) window.clearTimeout(saveTimer.current);
      if (immediate) {
        persist(next);
      } else {
        saveTimer.current = window.setTimeout(() => {
          saveTimer.current = null;
          if (latest.current) persist(latest.current);
        }, SAVE_DEBOUNCE_MS);
      }
      return next;
    });
  }

  // Flush a pending save when the dialog closes or the section unmounts.
  useEffect(() => {
    return () => {
      if (saveTimer.current !== null) {
        window.clearTimeout(saveTimer.current);
        saveTimer.current = null;
        if (latest.current) persist(latest.current);
      }
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  function addRow(list: "styles" | "questions") {
    const row: Row = { key: nextKey++, id: "", name: "", prompt: "", scope: "call", connectors: false };
    setDoc((current) => (current === null ? current : { ...current, [list]: [...current[list], row] }));
    setExpanded(row.key);
  }

  function editRow(list: "styles" | "questions", key: number, patch: Partial<Row>) {
    update((current) => ({
      ...current,
      [list]: current[list].map((r) => (r.key === key ? { ...r, ...patch } : r)),
    }));
  }

  function deleteRow(list: "styles" | "questions", key: number) {
    setExpanded(null);
    update((current) => ({ ...current, [list]: current[list].filter((r) => r.key !== key) }), true);
  }

  async function handleRestore() {
    const yes = await ask("Replace all your prompts with the built-in defaults? Your edits are lost.", {
      title: "Restore default prompts",
      kind: "warning",
      okLabel: "Restore",
      cancelLabel: "Cancel",
    });
    if (!yes) return;
    try {
      const prompts = await restoreDefaultPrompts();
      setDoc(toDoc(prompts));
      setSaveError(null);
      setExpanded(null);
      notifyPromptsChanged();
    } catch (err) {
      setSaveError(err instanceof Error ? err.message : String(err));
    }
  }

  function renderList(list: "styles" | "questions", rows: Row[], kind: string) {
    return (
      <ul className="flex flex-col divide-y divide-border border-y border-border">
        {rows.map((row) => {
          const isOpen = expanded === row.key;
          const nameId = `prompt-${row.key}-name`;
          const promptId = `prompt-${row.key}-prompt`;
          return (
            <li key={row.key} className="flex flex-col">
              <button
                type="button"
                aria-expanded={isOpen}
                onClick={() => setExpanded(isOpen ? null : row.key)}
                className="flex w-full items-center gap-2.5 px-0.5 py-2.5 text-left hover:bg-foreground/[0.03]"
              >
                {isOpen ? (
                  <ChevronDown className="size-3.5 shrink-0 text-muted-foreground" />
                ) : (
                  <ChevronRight className="size-3.5 shrink-0 text-muted-foreground" />
                )}
                <span className="flex min-w-0 flex-1 flex-col gap-px">
                  <span className={cn("truncate text-[13px] font-medium", row.name.trim() === "" && "text-muted-foreground")}>
                    {row.name.trim() === "" ? `New ${kind}` : row.name}
                  </span>
                  {!isOpen && (
                    <span className="truncate text-[11.5px] text-muted-foreground">
                      {firstLine(row.prompt) || "No prompt yet"}
                    </span>
                  )}
                </span>
                {list === "questions" && !isOpen && (
                  <span className="shrink-0 text-[11px] text-muted-foreground">
                    {QUICK_SCOPE_OPTIONS.find((o) => o.value === row.scope)?.label}
                    {row.connectors && " · connectors"}
                  </span>
                )}
              </button>
              {isOpen && (
                <div className="flex flex-col gap-2.5 px-0.5 pb-3.5 pl-6">
                  <div className="flex items-center gap-2.5">
                    <label htmlFor={nameId} className="sr-only">
                      Name
                    </label>
                    <input
                      id={nameId}
                      type="text"
                      value={row.name}
                      placeholder="Name"
                      autoFocus={row.id === ""}
                      onChange={(e) => editRow(list, row.key, { name: e.target.value })}
                      className={cn(FIELD_CLASS, "h-8 py-0 font-medium")}
                    />
                    {list === "questions" && (
                      <div
                        role="radiogroup"
                        aria-label="Where it is offered"
                        className="flex h-8 shrink-0 items-center rounded-full bg-foreground/[0.06] p-[3px]"
                      >
                        {QUICK_SCOPE_OPTIONS.map((option) => (
                          <button
                            key={option.value}
                            type="button"
                            role="radio"
                            aria-checked={row.scope === option.value}
                            onClick={() => editRow(list, row.key, { scope: option.value })}
                            className={cn(
                              "h-[26px] rounded-full px-2.5 text-[12px] text-foreground",
                              row.scope === option.value ? "bg-foreground/[0.1] font-medium" : "hover:bg-foreground/[0.05]",
                            )}
                          >
                            {option.label}
                          </button>
                        ))}
                      </div>
                    )}
                  </div>
                  <label htmlFor={promptId} className="sr-only">
                    Prompt
                  </label>
                  <textarea
                    id={promptId}
                    value={row.prompt}
                    rows={4}
                    placeholder={
                      list === "styles"
                        ? "How should the summary be written? What to extract, in what shape."
                        : "What to ask. The call or the notes are already in front of the assistant."
                    }
                    onChange={(e) => editRow(list, row.key, { prompt: e.target.value })}
                    className={cn(FIELD_CLASS, "resize-y")}
                  />
                  {list === "questions" && (
                    <label className="flex items-start gap-2 text-[12px] text-foreground">
                      <input
                        type="checkbox"
                        checked={row.connectors}
                        onChange={(e) => editRow(list, row.key, { connectors: e.target.checked })}
                        className="mt-[3px] size-3.5 accent-primary"
                      />
                      <span className="flex flex-col gap-px">
                        <span>Use my connectors</span>
                        <span className="text-[11px] leading-[1.45] text-muted-foreground">
                          Jira, calendar, Slack and whatever else Claude Code is connected to, to look things
                          up. Slower to start; leave off for questions about the notes alone.
                        </span>
                      </span>
                    </label>
                  )}
                  <div className="flex items-center justify-between">
                    <button
                      type="button"
                      onClick={() => deleteRow(list, row.key)}
                      className="text-[12px] text-destructive underline-offset-2 hover:underline"
                    >
                      Delete
                    </button>
                    <button
                      type="button"
                      onClick={() => setExpanded(null)}
                      className="text-[12px] text-quiet-foreground underline-offset-2 hover:underline"
                    >
                      Done
                    </button>
                  </div>
                </div>
              )}
            </li>
          );
        })}
        <li>
          <button
            type="button"
            onClick={() => addRow(list)}
            className="flex w-full items-center gap-2.5 px-0.5 py-2.5 text-left text-[13px] text-primary hover:bg-foreground/[0.03]"
          >
            <Plus className="size-3.5 shrink-0" />
            Add {kind}
          </button>
        </li>
      </ul>
    );
  }

  return (
    <section id="settings-prompts" className="flex flex-col gap-7">
      <div className="flex flex-col gap-1.5">
        <GroupHeading>Prompts</GroupHeading>
        <p className="max-w-[62ch] text-[11.5px] leading-[1.55] text-muted-foreground">
          Your words for the assistant. Summary styles decide how a note's summary is written;
          quick questions are shortcuts in Ask this call and Ask your notes. All of it is one
          plain file, prompts.toml, next to the settings.
        </p>
        {loadError && (
          <p role="alert" className="text-[11.5px] leading-[1.5] text-destructive [overflow-wrap:anywhere]">
            prompts.toml could not be read: {loadError}
          </p>
        )}
        {doc && (
          <div className="flex flex-col gap-1.5 border-y border-border py-3">
            <label htmlFor="summary-instructions" className="text-[13px] font-medium">
              Summary instructions
            </label>
            <span className="text-[11.5px] leading-[1.45] text-muted-foreground">
              Added to every summary, whatever the style. They refine the summary; the built-in
              rules on language and accuracy still hold.
            </span>
            <textarea
              id="summary-instructions"
              value={doc.instructions}
              rows={3}
              placeholder="e.g. Always name the customer and the deal size. British spelling."
              onChange={(e) => update((current) => ({ ...current, instructions: e.target.value }))}
              className={cn(FIELD_CLASS, "mt-1 resize-y")}
            />
          </div>
        )}
      </div>

      {doc && (
        <>
          <div className="flex flex-col gap-1.5">
            <GroupHeading right={doc.styles.length > 0 ? doc.styles.length : undefined}>Summary styles</GroupHeading>
            <p className="text-[11.5px] leading-[1.5] text-muted-foreground">
              Pick one in a note's Summary tab, or from the toolbar menu. Standard is built in.
            </p>
            {renderList("styles", doc.styles, "style")}
          </div>
          <div className="flex flex-col gap-1.5">
            <GroupHeading right={doc.questions.length > 0 ? doc.questions.length : undefined}>
              Quick questions
            </GroupHeading>
            <p className="text-[11.5px] leading-[1.5] text-muted-foreground">
              Shown as chips in an empty conversation and behind / in the question field.
            </p>
            {renderList("questions", doc.questions, "question")}
          </div>
          <div className="flex items-center justify-between">
            {saveError ? (
              <span role="alert" className="text-[11.5px] text-destructive [overflow-wrap:anywhere]">
                Not saved: {saveError}
              </span>
            ) : (
              <span />
            )}
            <button
              type="button"
              onClick={() => void handleRestore()}
              className="text-[11.5px] text-quiet-foreground underline-offset-2 hover:underline"
            >
              Restore defaults
            </button>
          </div>
        </>
      )}
    </section>
  );
}
