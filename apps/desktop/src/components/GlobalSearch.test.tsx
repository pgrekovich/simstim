import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { GlobalSearch, highlightSegments, parseTranscriptSnippet } from "@/components/GlobalSearch";
import { searchSessions, type SearchHit, type SessionMeta } from "@/lib/ipc";

vi.mock("@/lib/ipc", () => ({
  searchSessions: vi.fn(),
}));

const sessions: SessionMeta[] = [
  {
    path: "/tmp/sync.md",
    title: "Weekly sync",
    date: "2026-08-20T14:00:00+02:00",
    duration_secs: 65,
  },
  {
    path: "/tmp/standup.md",
    title: "Standup",
    date: "2026-08-19T09:00:00+02:00",
    duration_secs: null,
  },
];

const hit: SearchHit = {
  ...sessions[0],
  snippets: [
    { section: "notes", text: "Budget decisions" },
    { section: "transcript", text: "[00:00:04] Me: about the budget" },
  ],
  total_matches: 2,
};

beforeEach(() => {
  vi.mocked(searchSessions).mockReset().mockResolvedValue([hit]);
});

describe("GlobalSearch helpers", () => {
  it("parses a transcript prefix without changing the spoken text", () => {
    expect(parseTranscriptSnippet("[01:02:03] Them: hello there")).toEqual({
      timestamp: "01:02:03",
      speaker: "Them",
      text: "hello there",
    });
    expect(parseTranscriptSnippet("unstructured line")).toEqual({ text: "unstructured line" });
  });

  it("highlights every case-insensitive match", () => {
    expect(highlightSegments("Budget then budget", "budget")).toEqual([
      { text: "Budget", match: true },
      { text: " then ", match: false },
      { text: "budget", match: true },
    ]);
  });
});

describe("GlobalSearch", () => {
  it("shows recent notes before a query without scanning files again", () => {
    render(
      <GlobalSearch
        open
        onOpenChange={vi.fn()}
        sessions={sessions}
        onOpenSession={vi.fn()}
      />,
    );

    expect(screen.getByRole("dialog", { name: /search all notes/i })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: /weekly sync/i })).toHaveTextContent("1:05");
    expect(screen.getByRole("option", { name: /standup/i })).toBeInTheDocument();
    expect(searchSessions).not.toHaveBeenCalled();
  });

  it("groups note and spoken matches, including speaker, timestamp, and highlight", async () => {
    render(
      <GlobalSearch
        open
        onOpenChange={vi.fn()}
        sessions={sessions}
        onOpenSession={vi.fn()}
      />,
    );

    fireEvent.change(screen.getByRole("searchbox", { name: /search all notes/i }), {
      target: { value: "budget" },
    });
    await waitFor(() => expect(searchSessions).toHaveBeenCalledExactlyOnceWith("budget"));

    expect(await screen.findByRole("region", { name: "Notes" })).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Spoken" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: /about the budget/i })).toHaveTextContent("00:00:04");
    expect(screen.getByText("Me")).toBeInTheDocument();
    expect(document.querySelectorAll("mark")).toHaveLength(1);
  });

  it("moves with arrow keys and opens the active result with Enter", () => {
    const onOpenSession = vi.fn();
    const onOpenChange = vi.fn();
    render(
      <GlobalSearch
        open
        onOpenChange={onOpenChange}
        sessions={sessions}
        onOpenSession={onOpenSession}
      />,
    );

    const input = screen.getByRole("searchbox", { name: /search all notes/i });
    fireEvent.keyDown(input, { key: "ArrowDown" });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onOpenSession).toHaveBeenCalledExactlyOnceWith("/tmp/standup.md");
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("hands the global query and timestamp to spoken-result navigation", async () => {
    const onOpenSession = vi.fn();
    render(
      <GlobalSearch
        open
        onOpenChange={vi.fn()}
        sessions={sessions}
        onOpenSession={onOpenSession}
      />,
    );

    fireEvent.change(screen.getByRole("searchbox", { name: /search all notes/i }), {
      target: { value: "budget" },
    });
    fireEvent.click(await screen.findByRole("option", { name: /about the budget/i }));

    expect(onOpenSession).toHaveBeenCalledExactlyOnceWith("/tmp/sync.md", {
      query: "budget",
      timestamp: "00:00:04",
    });
  });

  it("keeps failures inside the dialog", async () => {
    vi.mocked(searchSessions).mockRejectedValue("folder unavailable");
    render(
      <GlobalSearch
        open
        onOpenChange={vi.fn()}
        sessions={sessions}
        onOpenSession={vi.fn()}
      />,
    );

    fireEvent.change(screen.getByRole("searchbox", { name: /search all notes/i }), {
      target: { value: "budget" },
    });
    expect(await screen.findByRole("alert")).toHaveTextContent("folder unavailable");
  });
});
