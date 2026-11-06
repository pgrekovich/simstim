import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { OpenLoopsView, callTarget, splitBody } from "@/components/OpenLoopsView";
import * as ipc from "@/lib/ipc";
import type { OpenLoops } from "@/lib/ipc";

vi.mock("@/lib/ipc", () => ({
  openLoopsToggle: vi.fn(),
  openLoopsSave: vi.fn(),
}));

// Lines 1-4 are frontmatter; the body starts on file line 5.
const BODY = [
  "",
  "## Mine",
  "",
  "- [ ] Send Alex the checklist · Sep 9 · [Beta planning](2026-09-09%2010-00-00%20call.md#t=00:00:16)",
  "",
  "## Waiting for",
  "",
  "- [ ] Sam: record a sample call · Sep 9 · [Beta planning](2026-09-09%2010-00-00%20call.md#t=00:00:09)",
  "",
  "## Open questions",
  "",
  "## Done",
  "",
  "- [x] Pick five testers · Sep 9 · done Sep 9",
].join("\n");

function loops(overrides: Partial<OpenLoops> = {}): OpenLoops {
  return {
    exists: true,
    body: BODY,
    body_line: 5,
    updated: "2026-09-10T09:00:00+02:00",
    open_count: 2,
    running: false,
    unread: 0,
    ...overrides,
  };
}

function view(props: Partial<React.ComponentProps<typeof OpenLoopsView>> = {}) {
  const handlers = {
    onChange: vi.fn(),
    onUpdate: vi.fn(),
    onOpenCitation: vi.fn(),
    onOpenFile: vi.fn(),
  };
  render(<OpenLoopsView loops={loops()} running={false} error={null} {...handlers} {...props} />);
  return handlers;
}

beforeEach(() => {
  vi.mocked(ipc.openLoopsToggle).mockReset();
  vi.mocked(ipc.openLoopsSave).mockReset();
  try {
    window.localStorage.clear();
  } catch {
    // Storage is optional in the component too.
  }
});

describe("splitBody and callTarget", () => {
  it("keeps each section's first file line", () => {
    const chunks = splitBody(BODY, 5);
    expect(chunks.map((c) => [c.heading, c.section, c.startLine])).toEqual([
      ["Mine", "mine", 7],
      ["Waiting for", "waiting", 11],
      ["Open questions", "questions", 15],
      ["Done", "done", 17],
    ]);
  });

  it("recognizes links to call files and their second", () => {
    expect(callTarget("2026-09-09%2010-00-00%20call.md#t=00:00:16")).toEqual({
      file: "2026-09-09 10-00-00 call.md",
      timestamp: "00:00:16",
    });
    expect(callTarget("call.md")).toEqual({ file: "call.md", timestamp: null });
    expect(callTarget("https://example.com/a.md")).toBeNull();
    expect(callTarget("notes/a.md")).toBeNull();
    expect(callTarget("image.png")).toBeNull();
  });
});

describe("OpenLoopsView", () => {
  it("renders sections with the action in ink and hides empty ones", async () => {
    view();
    expect(await screen.findByRole("checkbox", { name: "Send Alex the checklist" })).not.toBeChecked();
    expect(screen.getByRole("heading", { name: "Mine" })).toBeVisible();
    expect(screen.queryByRole("heading", { name: "Open questions" })).toBeNull();
    expect(screen.getByText("2 open")).toBeVisible();
    expect(screen.getByText(/Updated/)).toBeVisible();
    // Done is a divider below the open sections, open by default so a
    // checked item stays in sight.
    expect(screen.getByRole("button", { name: /done/i })).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("checkbox", { name: "Pick five testers" })).toBeChecked();
  });

  it("puts the date and source call on their own quiet line", async () => {
    view();
    const link = (await screen.findAllByRole("link", { name: "Beta planning" }))[0];
    const meta = link.closest(".open-loop-meta");
    expect(meta).not.toBeNull();
    expect(meta?.textContent).toBe("Sep 9 · Beta planning");
  });

  it("toggles the exact file line it rendered", async () => {
    const next = loops({ open_count: 1 });
    vi.mocked(ipc.openLoopsToggle).mockResolvedValue(next);
    const { onChange } = view();
    fireEvent.click(await screen.findByRole("checkbox", { name: "Sam: record a sample call" }));
    await waitFor(() =>
      expect(ipc.openLoopsToggle).toHaveBeenCalledWith(
        12,
        "- [ ] Sam: record a sample call · Sep 9 · [Beta planning](2026-09-09%2010-00-00%20call.md#t=00:00:09)",
        true,
      ),
    );
    expect(onChange).toHaveBeenCalledWith(next);
  });

  it("shows a refused toggle instead of pretending it worked", async () => {
    vi.mocked(ipc.openLoopsToggle).mockRejectedValue("Open loops changed on disk; showing the current file");
    view();
    fireEvent.click(await screen.findByRole("checkbox", { name: "Send Alex the checklist" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(/changed on disk/);
  });

  it("opens the cited call instead of following the link", async () => {
    const { onOpenCitation } = view();
    const links = await screen.findAllByRole("link", { name: "Beta planning" });
    fireEvent.click(links[0]);
    expect(onOpenCitation).toHaveBeenCalledWith({
      file: "2026-09-09 10-00-00 call.md",
      timestamp: "00:00:16",
    });
  });

  it("folds Done and remembers it", async () => {
    view();
    fireEvent.click(await screen.findByRole("button", { name: /done/i }));
    expect(screen.queryByRole("checkbox", { name: "Pick five testers" })).toBeNull();
    expect(window.localStorage.getItem("simstim.openLoops.doneOpen")).toBe("false");
  });

  it("edits the raw Markdown and saves against the body it started from", async () => {
    vi.mocked(ipc.openLoopsSave).mockResolvedValue(loops());
    const { onChange } = view();
    fireEvent.click(await screen.findByRole("heading", { name: "Mine" }));
    const editor = screen.getByRole("textbox", { name: "Open loops Markdown" });
    expect(editor).toHaveValue(BODY);
    fireEvent.change(editor, { target: { value: BODY.replace("record", "send") } });
    fireEvent.blur(editor);
    await waitFor(() => expect(ipc.openLoopsSave).toHaveBeenCalledWith(BODY.replace("record", "send"), BODY));
    expect(onChange).toHaveBeenCalled();
    expect(screen.queryByRole("textbox", { name: "Open loops Markdown" })).toBeNull();
  });

  it("offers a choice when the file changed during an edit", async () => {
    vi.mocked(ipc.openLoopsSave)
      .mockRejectedValueOnce("Open loops changed while you were editing")
      .mockResolvedValueOnce(loops());
    view();
    fireEvent.click(await screen.findByRole("heading", { name: "Mine" }));
    const editor = screen.getByRole("textbox", { name: "Open loops Markdown" });
    fireEvent.change(editor, { target: { value: "mine" } });
    fireEvent.blur(editor);
    fireEvent.click(await screen.findByRole("button", { name: "Keep mine" }));
    await waitFor(() => expect(ipc.openLoopsSave).toHaveBeenLastCalledWith("mine", null));
  });

  it("does not save an unchanged edit", async () => {
    view();
    fireEvent.click(await screen.findByRole("heading", { name: "Mine" }));
    fireEvent.blur(screen.getByRole("textbox", { name: "Open loops Markdown" }));
    await act(async () => {});
    expect(ipc.openLoopsSave).not.toHaveBeenCalled();
  });

  it("offers the first build when there is no file", () => {
    const { onUpdate } = view({ loops: loops({ exists: false, body: "", open_count: 0, updated: null }) });
    fireEvent.click(screen.getByRole("button", { name: "Build from the last four weeks" }));
    expect(onUpdate).toHaveBeenCalled();
  });

  it("shows a running update and a failed one in the provenance line", async () => {
    const { rerender } = render(
      <OpenLoopsView
        loops={loops()}
        running
        error={null}
        onChange={vi.fn()}
        onUpdate={vi.fn()}
        onOpenCitation={vi.fn()}
        onOpenFile={vi.fn()}
      />,
    );
    expect(screen.getByText("Updating...")).toBeVisible();
    expect(await screen.findByRole("checkbox", { name: "Send Alex the checklist" })).toBeDisabled();
    const onUpdate = vi.fn();
    rerender(
      <OpenLoopsView
        loops={loops()}
        running={false}
        error="Claude Code isn't installed"
        onChange={vi.fn()}
        onUpdate={onUpdate}
        onOpenCitation={vi.fn()}
        onOpenFile={vi.fn()}
      />,
    );
    expect(screen.getByText(/Couldn't update: Claude Code isn't installed/)).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(onUpdate).toHaveBeenCalled();
  });

  it("offers to read older calls and shows the step while it runs", async () => {
    const onUpdate = vi.fn();
    const { rerender } = render(
      <OpenLoopsView
        loops={loops({ unread: 12 })}
        running={false}
        error={null}
        onChange={vi.fn()}
        onUpdate={onUpdate}
        onOpenCitation={vi.fn()}
        onOpenFile={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Read 12 older calls" }));
    expect(onUpdate).toHaveBeenCalled();
    rerender(
      <OpenLoopsView
        loops={loops({ unread: 12 })}
        running
        progress="Reading call 3 of 12"
        error={null}
        onChange={vi.fn()}
        onUpdate={onUpdate}
        onOpenCitation={vi.fn()}
        onOpenFile={vi.fn()}
      />,
    );
    expect(screen.getByText("Reading call 3 of 12")).toBeVisible();
  });

  it("says so when nothing is open", async () => {
    view({ loops: loops({ body: "\n## Mine\n\n## Waiting for\n\n## Open questions\n\n## Done\n", open_count: 0 }) });
    expect(await screen.findByText(/Nothing open/)).toBeVisible();
  });
});
