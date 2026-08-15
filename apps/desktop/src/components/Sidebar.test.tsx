import { render, screen } from "@testing-library/react";
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
      />
    );

    expect(container.querySelector('[data-testid="recording-dot"]')).toBeNull();
  });
});

describe("Sidebar: selected styling", () => {
  it("applies the selected row's distinct background/border only to selectedPath", () => {
    const sessions = makeSessions();
    render(
      <Sidebar
        sessions={sessions}
        selectedPath="/tmp/b.md"
        activePath={null}
        onSelect={vi.fn()}
        onNewSession={vi.fn()}
      />
    );

    const rowA = screen.getByRole("button", { name: /session a/i });
    const rowB = screen.getByRole("button", { name: /session b/i });

    // Use exact classList tokens rather than substring matching on
    // className: "border-transparent" (the unselected default) and
    // "border-sidebar-accent"-style opacity variants like
    // "hover:bg-sidebar-accent/50" would otherwise false-positive a
    // substring check against "bg-sidebar-accent".
    expect(rowB.classList.contains("bg-sidebar-accent")).toBe(true);
    expect(rowB.classList.contains("border-primary")).toBe(true);
    expect(rowA.classList.contains("bg-sidebar-accent")).toBe(false);
    expect(rowA.classList.contains("border-primary")).toBe(false);
    expect(rowA.classList.contains("border-transparent")).toBe(true);
  });

  it("gives the selected row its own full-opacity hover instead of the unselected 50%-tint hover", () => {
    const sessions = makeSessions();
    render(
      <Sidebar
        sessions={sessions}
        selectedPath="/tmp/b.md"
        activePath={null}
        onSelect={vi.fn()}
        onNewSession={vi.fn()}
      />
    );

    const rowA = screen.getByRole("button", { name: /session a/i });
    const rowB = screen.getByRole("button", { name: /session b/i });

    // Regression test: a bare `hover:bg-sidebar-accent/50` alongside the
    // selected row's full-opacity `bg-sidebar-accent` would still win on
    // hover, visibly dimming the selected row instead of reinforcing it.
    // The selected row must get a same-color, full-opacity hover instead.
    expect(rowB.classList.contains("hover:bg-sidebar-accent")).toBe(true);
    expect(rowB.classList.contains("hover:bg-sidebar-accent/50")).toBe(false);
    expect(rowA.classList.contains("hover:bg-sidebar-accent/50")).toBe(true);
    expect(rowA.classList.contains("hover:bg-sidebar-accent")).toBe(false);
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
      />
    );

    expect(screen.queryByText(/^simstim$/i)).toBeNull();
    screen.getByRole("button", { name: /new note/i }).click();
    expect(onNewSession).toHaveBeenCalledTimes(1);
  });
});
