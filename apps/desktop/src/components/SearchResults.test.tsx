import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { SearchResults, highlightSegments } from "@/components/SearchResults";
import type { SearchHit } from "@/lib/ipc";

function hit(overrides: Partial<SearchHit> = {}): SearchHit {
  return {
    path: "/tmp/sync.md",
    title: "Weekly sync",
    date: "2026-08-20T14:00:00+02:00",
    duration_secs: null,
    snippets: [
      { section: "transcript", text: "[00:00:04] Me: let's talk about the budget" },
    ],
    total_matches: 1,
    ...overrides,
  };
}

describe("highlightSegments", () => {
  it("splits a line into plain and matched segments, case-insensitively", () => {
    expect(highlightSegments("Budget talk about budget", "budget")).toEqual([
      { text: "Budget", match: true },
      { text: " talk about ", match: false },
      { text: "budget", match: true },
    ]);
  });

  it("windows a long line so the first match stays visible", () => {
    const long = "x".repeat(200) + " budget " + "y".repeat(50);
    const segments = highlightSegments(long, "budget");
    expect(segments[0]).toEqual({ text: "…", match: false });
    expect(segments.some((s) => s.match && s.text === "budget")).toBe(true);
    // The window starts near the match, not at the line start.
    expect(segments[1].text.length).toBeLessThanOrEqual(61);
  });

  it("returns the whole line unmatched when the query isn't in it", () => {
    expect(highlightSegments("no such word here", "budget")).toEqual([
      { text: "no such word here", match: false },
    ]);
  });
});

describe("SearchResults", () => {
  it("renders a snippet with the query marked, and opens the note on click", () => {
    const onOpen = vi.fn();
    render(<SearchResults hits={[hit()]} query="budget" onOpen={onOpen} />);

    expect(screen.getByText(/1 note matches/i)).toBeInTheDocument();
    const marks = document.querySelectorAll("mark");
    expect(marks).toHaveLength(1);
    expect(marks[0].textContent).toBe("budget");

    screen.getByRole("button", { name: /weekly sync/i }).click();
    expect(onOpen).toHaveBeenCalledExactlyOnceWith("/tmp/sync.md");
  });

  it("says how many more matches the note has beyond the shown snippets", () => {
    render(
      <SearchResults hits={[hit({ total_matches: 7 })]} query="budget" onOpen={vi.fn()} />
    );
    expect(screen.getByText("7 matches in this note")).toBeInTheDocument();
  });

  it("shows an empty state naming the query", () => {
    render(<SearchResults hits={[]} query="quarterly" onOpen={vi.fn()} />);
    expect(screen.getByText('no matches for "quarterly"')).toBeInTheDocument();
  });
});
