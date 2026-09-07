import { describe, expect, it } from "vitest";
import type { Prompts } from "@/lib/ipc";
import { quickQuestionsFor, slugFor, styleName } from "@/lib/prompts";

const prompts: Prompts = {
  summary_instructions: "",
  summary_styles: [{ id: "action-items", name: "Action items", prompt: "..." }],
  quick_questions: [
    { id: "miss", name: "What did I miss", prompt: "...", scope: "call", connectors: false },
    { id: "loops", name: "Open loops", prompt: "...", scope: "notes", connectors: false },
    { id: "smart", name: "Make me smart", prompt: "...", scope: "both", connectors: false },
  ],
};

describe("prompts helpers", () => {
  it("offers a scope its own questions plus the ones marked both, in file order", () => {
    expect(quickQuestionsFor(prompts, "call").map((q) => q.id)).toEqual(["miss", "smart"]);
    expect(quickQuestionsFor(prompts, "notes").map((q) => q.id)).toEqual(["loops", "smart"]);
  });

  it("names styles, with Standard for the built-in prompt and the raw id for a deleted style", () => {
    expect(styleName(prompts.summary_styles, null)).toBe("Standard");
    expect(styleName(prompts.summary_styles, "standard")).toBe("Standard");
    expect(styleName(prompts.summary_styles, "action-items")).toBe("Action items");
    expect(styleName(prompts.summary_styles, "gone")).toBe("gone");
  });

  it("derives stable, unique ids from names and never takes the reserved one", () => {
    expect(slugFor("Make me smart", [])).toBe("make-me-smart");
    expect(slugFor("  Follow-up e-mail!  ", [])).toBe("follow-up-e-mail");
    expect(slugFor("Make me smart", ["make-me-smart"])).toBe("make-me-smart-2");
    expect(slugFor("Make me smart", ["make-me-smart", "make-me-smart-2"])).toBe("make-me-smart-3");
    expect(slugFor("Standard", [])).toBe("standard-2");
    expect(slugFor("Что я пропустил", [])).toBe("prompt");
  });
});
