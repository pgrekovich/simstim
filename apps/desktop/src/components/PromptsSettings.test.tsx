import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { PromptsSettings } from "@/components/PromptsSettings";
import * as ipc from "@/lib/ipc";
import type { Prompts } from "@/lib/ipc";
import { PROMPTS_CHANGED_EVENT } from "@/lib/prompts";

vi.mock("@/lib/ipc", () => ({
  getPrompts: vi.fn(),
  savePrompts: vi.fn(),
  restoreDefaultPrompts: vi.fn(),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({
  ask: vi.fn(),
}));
import { ask } from "@tauri-apps/plugin-dialog";

const seeded: Prompts = {
  summary_instructions: "British spelling.",
  summary_styles: [
    { id: "action-items", name: "Action items", prompt: "List only the action items.\nOwner first." },
    { id: "brief", name: "Brief", prompt: "A two-minute brief." },
  ],
  quick_questions: [
    { id: "what-did-i-miss", name: "What did I miss", prompt: "I stepped away.", scope: "call", connectors: false },
    { id: "open-loops", name: "Open loops", prompt: "Promises without follow-up.", scope: "notes", connectors: false },
  ],
};

beforeEach(() => {
  vi.mocked(ipc.getPrompts).mockReset().mockResolvedValue(structuredClone(seeded));
  vi.mocked(ipc.savePrompts).mockReset().mockImplementation(async (p) => p);
  vi.mocked(ipc.restoreDefaultPrompts).mockReset();
  vi.mocked(ask).mockReset();
});

describe("PromptsSettings", () => {
  it("lists styles and questions as rows with their first prompt line and scope", async () => {
    render(<PromptsSettings open />);
    const actions = await screen.findByRole("button", { name: /action items/i });
    expect(actions).toHaveTextContent("List only the action items.");
    expect(actions).not.toHaveTextContent("Owner first.");
    expect(screen.getByRole("button", { name: /open loops/i })).toHaveTextContent("All notes");
    expect(screen.getByRole("button", { name: /what did I miss/i })).toHaveTextContent("This call");
    expect(screen.getByLabelText("Summary instructions")).toHaveValue("British spelling.");
  });

  it("saves an edited prompt after the debounce, as the whole document, and announces the change", async () => {
    const changed = vi.fn();
    window.addEventListener(PROMPTS_CHANGED_EVENT, changed);
    render(<PromptsSettings open />);
    fireEvent.click(await screen.findByRole("button", { name: /^brief/i }));
    const prompt = screen.getByLabelText("Prompt");
    fireEvent.change(prompt, { target: { value: "A one-minute brief." } });
    await waitFor(() => expect(ipc.savePrompts).toHaveBeenCalledTimes(1));
    const saved = vi.mocked(ipc.savePrompts).mock.calls[0][0];
    expect(saved.summary_styles[1]).toEqual({ id: "brief", name: "Brief", prompt: "A one-minute brief." });
    // Untouched parts travel along unchanged.
    expect(saved.summary_instructions).toBe("British spelling.");
    expect(saved.quick_questions).toEqual(seeded.quick_questions);
    await waitFor(() => expect(changed).toHaveBeenCalled());
    window.removeEventListener(PROMPTS_CHANGED_EVENT, changed);
  });

  it("adds a question, gives it an id from its name on the first save, and stays unsaved while incomplete", async () => {
    render(<PromptsSettings open />);
    fireEvent.click(await screen.findByRole("button", { name: /add question/i }));
    // A new row opens its editor with the name focused.
    const name = screen.getByLabelText("Name");
    expect(name).toHaveFocus();
    fireEvent.change(name, { target: { value: "Make me smart" } });
    // Name alone is not a question yet: the document saves without it.
    await waitFor(() => expect(ipc.savePrompts).toHaveBeenCalledTimes(1));
    expect(vi.mocked(ipc.savePrompts).mock.calls[0][0].quick_questions).toHaveLength(2);

    fireEvent.click(within(screen.getByRole("radiogroup", { name: /where it is offered/i })).getByRole("radio", { name: "Both" }));
    fireEvent.change(screen.getByLabelText("Prompt"), { target: { value: "Explain the terms." } });
    await waitFor(() => expect(ipc.savePrompts).toHaveBeenCalledTimes(2));
    const saved = vi.mocked(ipc.savePrompts).mock.calls[1][0];
    expect(saved.quick_questions[2]).toEqual({
      id: "make-me-smart",
      name: "Make me smart",
      prompt: "Explain the terms.",
      scope: "both",
      connectors: false,
    });
  });

  it("lets a question use the connectors and marks the row", async () => {
    render(<PromptsSettings open />);
    fireEvent.click(await screen.findByRole("button", { name: /what did I miss/i }));
    fireEvent.click(screen.getByRole("checkbox", { name: /use my connectors/i }));
    await waitFor(() => expect(ipc.savePrompts).toHaveBeenCalledTimes(1));
    expect(vi.mocked(ipc.savePrompts).mock.calls[0][0].quick_questions[0].connectors).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Done" }));
    expect(screen.getByRole("button", { name: /what did I miss/i })).toHaveTextContent("This call · connectors");
  });

  it("deletes a style at once and restores the defaults only after the native confirm", async () => {
    render(<PromptsSettings open />);
    fireEvent.click(await screen.findByRole("button", { name: /action items/i }));
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(ipc.savePrompts).toHaveBeenCalledTimes(1));
    expect(vi.mocked(ipc.savePrompts).mock.calls[0][0].summary_styles.map((s) => s.id)).toEqual(["brief"]);
    expect(screen.queryByRole("button", { name: /action items/i })).toBeNull();

    vi.mocked(ask).mockResolvedValue(false);
    fireEvent.click(screen.getByRole("button", { name: /restore defaults/i }));
    await waitFor(() => expect(ask).toHaveBeenCalledTimes(1));
    expect(ipc.restoreDefaultPrompts).not.toHaveBeenCalled();

    vi.mocked(ask).mockResolvedValue(true);
    vi.mocked(ipc.restoreDefaultPrompts).mockResolvedValue(structuredClone(seeded));
    fireEvent.click(screen.getByRole("button", { name: /restore defaults/i }));
    await waitFor(() => expect(ipc.restoreDefaultPrompts).toHaveBeenCalledTimes(1));
    expect(await screen.findByRole("button", { name: /action items/i })).toBeInTheDocument();
  });

  it("shows a broken hand edit instead of clobbering it", async () => {
    vi.mocked(ipc.getPrompts).mockRejectedValue(new Error("parsing prompts.toml as TOML: expected ]"));
    render(<PromptsSettings open />);
    expect(await screen.findByRole("alert")).toHaveTextContent("expected ]");
    expect(screen.queryByRole("button", { name: /add style/i })).toBeNull();
    expect(ipc.savePrompts).not.toHaveBeenCalled();
  });
});
