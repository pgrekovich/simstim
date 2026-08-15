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

    expect(rowA.querySelector('[data-testid="recording-dot"]')).not.toBeNull();
    expect(rowB.querySelector('[data-testid="recording-dot"]')).toBeNull();
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
