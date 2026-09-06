import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { Sidebar } from "@/components/Sidebar";
import type { SessionMeta } from "@/lib/ipc";

function makeSessions(): SessionMeta[] {
  const now = new Date().toISOString();
  return [
    { path: "/tmp/a.md", title: "Session A", date: now, duration_secs: null },
    { path: "/tmp/b.md", title: "Session B", date: now, duration_secs: null },
  ];
}

describe("Sidebar: recording indicator", () => {
  it("shows the pulsing recording dot only next to the row matching activePath", () => {
    const sessions = makeSessions();
    render(
      <Sidebar
        sessions={sessions}
        selectedPath={null}
        activePath="/tmp/a.md"
        onSelect={vi.fn()}
        onNewSession={vi.fn()}
        onDelete={vi.fn()}
      />
    );

    const rowA = screen.getByRole("button", { name: /session a/i });
    const rowB = screen.getByRole("button", { name: /session b/i });

    const dot = rowA.querySelector('[data-testid="recording-dot"]');
    expect(dot).not.toBeNull();
    // Theme-tuned token, not a hardcoded color: index.css defines separate
    // light/dark values for `--destructive`.
    expect(dot?.className).toContain("bg-destructive");
    expect(rowB.querySelector('[data-testid="recording-dot"]')).toBeNull();
  });

  it("exposes a screen-reader-only 'recording' label without becoming the row's primary accessible name", () => {
    render(
      <Sidebar
        sessions={makeSessions()}
        selectedPath={null}
        activePath="/tmp/a.md"
        onSelect={vi.fn()}
        onNewSession={vi.fn()}
        onDelete={vi.fn()}
      />
    );

    // Matches how App.test.tsx finds the active row: the name must still
    // START with the title, i.e. the sr-only text is appended after it, not
    // prepended before it.
    const rowA = screen.getByRole("button", { name: /^session a/i });
    expect(rowA.textContent).toContain("recording");
  });

  it("shows no recording dot on any row when activePath is null", () => {
    const { container } = render(
      <Sidebar
        sessions={makeSessions()}
        selectedPath={null}
        activePath={null}
        onSelect={vi.fn()}
        onNewSession={vi.fn()}
        onDelete={vi.fn()}
      />
    );

    expect(container.querySelector('[data-testid="recording-dot"]')).toBeNull();
  });

  it("uses the design's two-line recording row instead of showing a timestamp", () => {
    render(
      <Sidebar
        sessions={makeSessions()}
        selectedPath="/tmp/a.md"
        activePath="/tmp/a.md"
        onSelect={vi.fn()}
        onNewSession={vi.fn()}
        onDelete={vi.fn()}
      />
    );

    const row = screen.getByRole("button", { name: /^session a/i });
    expect(row.classList.contains("items-center")).toBe(true);
    expect(row.textContent).toBe("Session Arecording");
  });
});

describe("Sidebar: session metadata", () => {
  it("places time and duration on the second line below the title", () => {
    const sessions = makeSessions();
    sessions[0].duration_secs = 65;
    render(
      <Sidebar
        sessions={sessions}
        selectedPath={null}
        activePath={null}
        onSelect={vi.fn()}
        onNewSession={vi.fn()}
        onDelete={vi.fn()}
      />
    );

    const row = screen.getByRole("button", { name: /^session a/i });
    expect(row.classList.contains("flex-col")).toBe(true);
    expect(row.textContent).toContain(" · 1:05");
    expect(row.querySelector(".group-hover\\:opacity-0")).toBeNull();
  });
});

describe("Sidebar: selected styling", () => {
  it("applies the selected row's distinct fill and weight only to selectedPath", () => {
    const sessions = makeSessions();
    render(
      <Sidebar
        sessions={sessions}
        selectedPath="/tmp/b.md"
        activePath={null}
        onSelect={vi.fn()}
        onNewSession={vi.fn()}
        onDelete={vi.fn()}
      />
    );

    const rowA = screen.getByRole("button", { name: /session a/i });
    const rowB = screen.getByRole("button", { name: /session b/i });

    // Exact classList tokens, not substring matching: the hover variants
    // carry the same base names at different opacities.
    expect(rowB.classList.contains("bg-background")).toBe(true);
    expect(rowB.querySelector(".font-medium")).not.toBeNull();
    expect(rowA.classList.contains("bg-background")).toBe(false);
    expect(rowA.querySelector(".font-medium")).toBeNull();
    // No left rule on any row: a border on a rounded row reads as a bracket.
    expect(rowA.className).not.toContain("border-l");
    expect(rowB.className).not.toContain("border-l");
  });

  it("gives the selected row a deeper hover than the unselected rows", () => {
    const sessions = makeSessions();
    render(
      <Sidebar
        sessions={sessions}
        selectedPath="/tmp/b.md"
        activePath={null}
        onSelect={vi.fn()}
        onNewSession={vi.fn()}
        onDelete={vi.fn()}
      />
    );

    const rowA = screen.getByRole("button", { name: /session a/i });
    const rowB = screen.getByRole("button", { name: /session b/i });

    // Regression test: a lower-opacity hover shared with the selected row
    // would win the cascade on hover and visibly dim it instead of
    // reinforcing it, so each state carries its own hover step.
    expect(rowB.classList.contains("hover:bg-background/80")).toBe(true);
    expect(rowB.classList.contains("hover:bg-foreground/[0.05]")).toBe(false);
    expect(rowA.classList.contains("hover:bg-foreground/[0.05]")).toBe(true);
    expect(rowA.classList.contains("hover:bg-background/80")).toBe(false);
  });
});

describe("Sidebar: new note button", () => {
  it("calls onNewSession when clicked, without a wordmark in the sidebar", () => {
    const onNewSession = vi.fn();
    render(
      <Sidebar
        sessions={[]}
        selectedPath={null}
        activePath={null}
        onSelect={vi.fn()}
        onNewSession={onNewSession}
        onDelete={vi.fn()}
      />
    );

    expect(screen.queryByText(/^simstim$/i)).toBeNull();
    screen.getByRole("button", { name: /new note/i }).click();
    expect(onNewSession).toHaveBeenCalledTimes(1);
  });
});

describe("Sidebar: collapse button", () => {
  it("reports a collapse request without changing the selected row", () => {
    const onCollapse = vi.fn();
    render(
      <Sidebar
        sessions={makeSessions()}
        selectedPath="/tmp/a.md"
        activePath={null}
        onSelect={vi.fn()}
        onNewSession={vi.fn()}
        onCollapse={onCollapse}
        onDelete={vi.fn()}
      />
    );

    const collapse = screen.getByRole("button", { name: /collapse sidebar/i });
    expect(collapse.parentElement?.className).toContain("pl-[88px]");
    collapse.click();
    expect(onCollapse).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: /session a/i })).toBeInTheDocument();
  });
});

describe("Sidebar: delete button", () => {
  it("calls onDelete with the row's path, and never onSelect, when its trash button is clicked", () => {
    const onSelect = vi.fn();
    const onDelete = vi.fn();
    render(
      <Sidebar
        sessions={makeSessions()}
        selectedPath={null}
        activePath={null}
        onSelect={onSelect}
        onNewSession={vi.fn()}
        onDelete={onDelete}
      />
    );

    const trashButtons = screen.getAllByRole("button", { name: /move to trash/i });
    expect(trashButtons).toHaveLength(2);
    trashButtons[1].click();
    expect(onDelete).toHaveBeenCalledExactlyOnceWith("/tmp/b.md");
    expect(onSelect).not.toHaveBeenCalled();
  });

  it("offers no trash button for the recording row (the backend refuses deleting it anyway)", () => {
    render(
      <Sidebar
        sessions={makeSessions()}
        selectedPath={null}
        activePath="/tmp/a.md"
        onSelect={vi.fn()}
        onNewSession={vi.fn()}
        onDelete={vi.fn()}
      />
    );

    // Only Session B's row (not recording) gets one.
    expect(screen.getAllByRole("button", { name: /move to trash/i })).toHaveLength(1);
  });
});

describe("Sidebar: global search", () => {
  it("opens the global palette instead of filtering the sidebar inline", () => {
    const onOpenSearch = vi.fn();
    render(
      <Sidebar
        sessions={makeSessions()}
        selectedPath={null}
        activePath={null}
        onSelect={vi.fn()}
        onNewSession={vi.fn()}
        onOpenSearch={onOpenSearch}
        onDelete={vi.fn()}
      />
    );

    expect(screen.queryByRole("searchbox")).toBeNull();
    screen.getByRole("button", { name: /search all notes/i }).click();
    expect(onOpenSearch).toHaveBeenCalledTimes(1);
  });
});

describe("Sidebar: row context menu (macOS right-click reflex)", () => {
  it("opens on right click and routes each action to its callback with the row's path", () => {
    const onOpenFile = vi.fn();
    const onRevealInFinder = vi.fn();
    const onDelete = vi.fn();
    const onSelect = vi.fn();
    render(
      <Sidebar
        sessions={makeSessions()}
        selectedPath={null}
        activePath={null}
        onSelect={onSelect}
        onNewSession={vi.fn()}
        onDelete={onDelete}
        onOpenFile={onOpenFile}
        onRevealInFinder={onRevealInFinder}
      />
    );

    fireEvent.contextMenu(screen.getByRole("button", { name: /session a/i }));
    fireEvent.click(screen.getByRole("menuitem", { name: /show in finder/i }));
    expect(onRevealInFinder).toHaveBeenCalledExactlyOnceWith("/tmp/a.md");

    fireEvent.contextMenu(screen.getByRole("button", { name: /session b/i }));
    fireEvent.click(screen.getByRole("menuitem", { name: /open file/i }));
    expect(onOpenFile).toHaveBeenCalledExactlyOnceWith("/tmp/b.md");

    fireEvent.contextMenu(screen.getByRole("button", { name: /session b/i }));
    fireEvent.click(screen.getByRole("menuitem", { name: /move to trash/i }));
    expect(onDelete).toHaveBeenCalledExactlyOnceWith("/tmp/b.md");

    // The right click itself must not select the row.
    expect(onSelect).not.toHaveBeenCalled();
  });

  it("disables Move to Trash for the recording row (same gate as the hover trash button)", () => {
    render(
      <Sidebar
        sessions={makeSessions()}
        selectedPath={null}
        activePath="/tmp/a.md"
        onSelect={vi.fn()}
        onNewSession={vi.fn()}
        onDelete={vi.fn()}
      />
    );

    fireEvent.contextMenu(screen.getByRole("button", { name: /^session a/i }));
    expect(screen.getByRole("menuitem", { name: /move to trash/i })).toHaveAttribute(
      "data-disabled"
    );
  });
});
