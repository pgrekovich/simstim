import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { GlobalSearch, highlightSegments, parseTranscriptSnippet } from "@/components/GlobalSearch";
import { searchSessions, type SearchHit, type SessionMeta } from "@/lib/ipc";

vi.mock("@/lib/ipc", () => ({
  getPrompts: vi.fn().mockResolvedValue({ summary_instructions: "", summary_styles: [], quick_questions: [] }),
  savePrompts: vi.fn().mockResolvedValue({ summary_instructions: "", summary_styles: [], quick_questions: [] }),
  restoreDefaultPrompts: vi.fn().mockResolvedValue({ summary_instructions: "", summary_styles: [], quick_questions: [] }),
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
    expect(screen.getByRole("option", { name: /weekly sync/i })).toHaveTextContent("1 min");
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

describe("GlobalSearch: ask across all notes (stage 4.2)", () => {
  it("offers the ask row only with a query, and hands the question over", async () => {
    const onAskAllNotes = vi.fn();
    const onOpenChange = vi.fn();
    render(
      <GlobalSearch
        open
        onOpenChange={onOpenChange}
        sessions={sessions}
        onOpenSession={vi.fn()}
        onAskAllNotes={onAskAllNotes}
      />,
    );

    // Blank query: recent notes only, no ask row.
    expect(screen.queryByRole("option", { name: /ask across all notes/i })).toBeNull();

    fireEvent.change(screen.getByRole("searchbox", { name: /search all notes/i }), {
      target: { value: "berlin rollout" },
    });
    fireEvent.click(await screen.findByRole("option", { name: /ask across all notes/i }));
    expect(onAskAllNotes).toHaveBeenCalledExactlyOnceWith("berlin rollout");
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("asks with ⌘Enter regardless of the active row", async () => {
    const onAskAllNotes = vi.fn();
    render(
      <GlobalSearch
        open
        onOpenChange={vi.fn()}
        sessions={sessions}
        onOpenSession={vi.fn()}
        onAskAllNotes={onAskAllNotes}
      />,
    );

    const input = screen.getByRole("searchbox", { name: /search all notes/i });
    fireEvent.change(input, { target: { value: "berlin" } });
    await screen.findByRole("option", { name: /ask across all notes/i });
    fireEvent.keyDown(input, { key: "Enter", metaKey: true });
    expect(onAskAllNotes).toHaveBeenCalledExactlyOnceWith("berlin");
  });

  it("keeps plain Enter on results and reaches the ask row by arrows", async () => {
    const onAskAllNotes = vi.fn();
    const onOpenSession = vi.fn();
    render(
      <GlobalSearch
        open
        onOpenChange={vi.fn()}
        sessions={sessions}
        onOpenSession={onOpenSession}
        onAskAllNotes={onAskAllNotes}
      />,
    );

    const input = screen.getByRole("searchbox", { name: /search all notes/i });
    fireEvent.change(input, { target: { value: "budget" } });
    // The ask row renders instantly; the RESULT rows land only once the
    // debounced search resolves, so wait for one before pressing Enter.
    await screen.findByRole("option", { name: /about the budget/i });

    // Arrow past the two result rows (one note, one spoken) to the ask row.
    fireEvent.keyDown(input, { key: "ArrowDown" });
    fireEvent.keyDown(input, { key: "ArrowDown" });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onAskAllNotes).toHaveBeenCalledExactlyOnceWith("budget");
    // Plain Enter on a result row stays navigation (the test above); it
    // never fell through to ask here.
    expect(onOpenSession).not.toHaveBeenCalled();
  });
});

describe("GlobalSearch: ask this call (one field, 2026-09-04)", () => {
  it("offers 'Ask this call' above 'Ask across all notes' when a note is open, and hands the question over", async () => {
    const onAskThisCall = vi.fn();
    const onAskAllNotes = vi.fn();
    const onOpenChange = vi.fn();
    render(
      <GlobalSearch
        open
        onOpenChange={onOpenChange}
        sessions={sessions}
        onOpenSession={vi.fn()}
        onAskAllNotes={onAskAllNotes}
        onAskThisCall={onAskThisCall}
      />,
    );

    expect(screen.queryByRole("option", { name: /ask this call/i })).toBeNull();
    fireEvent.change(screen.getByRole("searchbox", { name: /search all notes/i }), {
      target: { value: "what did I promise" },
    });
    const thisCall = await screen.findByRole("option", { name: /ask this call/i });
    const allNotes = screen.getByRole("option", { name: /ask across all notes/i });
    expect(thisCall.compareDocumentPosition(allNotes) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();

    fireEvent.click(thisCall);
    expect(onAskThisCall).toHaveBeenCalledExactlyOnceWith("what did I promise");
    expect(onAskAllNotes).not.toHaveBeenCalled();
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("renders no 'Ask this call' row without an open note", async () => {
    render(
      <GlobalSearch
        open
        onOpenChange={vi.fn()}
        sessions={sessions}
        onOpenSession={vi.fn()}
        onAskAllNotes={vi.fn()}
      />,
    );
    fireEvent.change(screen.getByRole("searchbox", { name: /search all notes/i }), {
      target: { value: "berlin" },
    });
    await screen.findByRole("option", { name: /ask across all notes/i });
    expect(screen.queryByRole("option", { name: /ask this call/i })).toBeNull();
  });
});
