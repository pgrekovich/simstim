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
        searchQuery=""
        onSearchChange={vi.fn()}
        onOpenSettings={vi.fn()}
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
        searchQuery=""
        onSearchChange={vi.fn()}
        onOpenSettings={vi.fn()}
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
        searchQuery=""
        onSearchChange={vi.fn()}
        onOpenSettings={vi.fn()}
      />
    );

    expect(container.querySelector('[data-testid="recording-dot"]')).toBeNull();
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
        searchQuery=""
        onSearchChange={vi.fn()}
        onOpenSettings={vi.fn()}
      />
    );

    const rowA = screen.getByRole("button", { name: /session a/i });
    const rowB = screen.getByRole("button", { name: /session b/i });

    // Exact classList tokens, not substring matching: the hover variants
    // carry the same base names at different opacities.
    expect(rowB.classList.contains("bg-foreground/[0.12]")).toBe(true);
    expect(rowB.classList.contains("font-medium")).toBe(true);
    expect(rowA.classList.contains("bg-foreground/[0.12]")).toBe(false);
    expect(rowA.classList.contains("font-medium")).toBe(false);
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
        searchQuery=""
        onSearchChange={vi.fn()}
        onOpenSettings={vi.fn()}
      />
    );

    const rowA = screen.getByRole("button", { name: /session a/i });
    const rowB = screen.getByRole("button", { name: /session b/i });

    // Regression test: a lower-opacity hover shared with the selected row
    // would win the cascade on hover and visibly dim it instead of
    // reinforcing it, so each state carries its own hover step.
    expect(rowB.classList.contains("hover:bg-foreground/[0.16]")).toBe(true);
    expect(rowB.classList.contains("hover:bg-foreground/[0.07]")).toBe(false);
    expect(rowA.classList.contains("hover:bg-foreground/[0.07]")).toBe(true);
    expect(rowA.classList.contains("hover:bg-foreground/[0.16]")).toBe(false);
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
        searchQuery=""
        onSearchChange={vi.fn()}
        onOpenSettings={vi.fn()}
      />
    );

    expect(screen.queryByText(/^simstim$/i)).toBeNull();
    screen.getByRole("button", { name: /new note/i }).click();
    expect(onNewSession).toHaveBeenCalledTimes(1);
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
        searchQuery=""
        onSearchChange={vi.fn()}
        onOpenSettings={vi.fn()}
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
        searchQuery=""
        onSearchChange={vi.fn()}
        onOpenSettings={vi.fn()}
      />
    );

    // Only Session B's row (not recording) gets one.
    expect(screen.getAllByRole("button", { name: /move to trash/i })).toHaveLength(1);
  });
});

describe("Sidebar: search and settings (stage 0.10)", () => {
  it("reports every keystroke through onSearchChange", () => {
    const onSearchChange = vi.fn();
    render(
      <Sidebar
        sessions={makeSessions()}
        selectedPath={null}
        activePath={null}
        onSelect={vi.fn()}
        onNewSession={vi.fn()}
        onDelete={vi.fn()}
        searchQuery=""
        onSearchChange={onSearchChange}
        onOpenSettings={vi.fn()}
      />
    );

    const input = screen.getByRole("searchbox", { name: /search sessions/i });
    fireEvent.change(input, { target: { value: "budget" } });
    expect(onSearchChange).toHaveBeenCalledExactlyOnceWith("budget");
  });

  it("says 'no matches' for an empty result of an active search, not 'no sessions yet'", () => {
    render(
      <Sidebar
        sessions={[]}
        selectedPath={null}
        activePath={null}
        onSelect={vi.fn()}
        onNewSession={vi.fn()}
        onDelete={vi.fn()}
        searchQuery="quarterly"
        onSearchChange={vi.fn()}
        onOpenSettings={vi.fn()}
      />
    );

    expect(screen.getByText("no matches")).toBeInTheDocument();
    expect(screen.queryByText("no sessions yet")).toBeNull();
  });

  it("opens settings from the footer button", () => {
    const onOpenSettings = vi.fn();
    render(
      <Sidebar
        sessions={[]}
        selectedPath={null}
        activePath={null}
        onSelect={vi.fn()}
        onNewSession={vi.fn()}
        onDelete={vi.fn()}
        searchQuery=""
        onSearchChange={vi.fn()}
        onOpenSettings={onOpenSettings}
      />
    );

    screen.getByRole("button", { name: /settings/i }).click();
    expect(onOpenSettings).toHaveBeenCalledTimes(1);
  });
});
