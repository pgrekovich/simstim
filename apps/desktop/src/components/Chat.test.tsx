import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  ChatConversation,
  distinctSourceCount,
  shortTimestamp,
  sourceChipLabel,
} from "@/components/Chat";
import {
  chatAsk,
  chatCancel,
  chatHistory,
  type ChatMessage,
  type ChatProgress,
  type SessionMeta,
} from "@/lib/ipc";
import { listen } from "@tauri-apps/api/event";

vi.mock("@/lib/ipc", () => ({
  getPrompts: vi.fn().mockResolvedValue({ summary_instructions: "", summary_styles: [], quick_questions: [] }),
  savePrompts: vi.fn().mockResolvedValue({ summary_instructions: "", summary_styles: [], quick_questions: [] }),
  restoreDefaultPrompts: vi.fn().mockResolvedValue({ summary_instructions: "", summary_styles: [], quick_questions: [] }),
  chatAsk: vi.fn().mockResolvedValue(undefined),
  chatCancel: vi.fn().mockResolvedValue(undefined),
  chatClear: vi.fn().mockResolvedValue(undefined),
  chatHistory: vi.fn().mockResolvedValue({ messages: [], running: false }),
}));

// Captures each event listener so tests can push chat-progress events the way
// the backend would.
type Handler = (event: { payload: ChatProgress }) => void;
const handlers: Record<string, Handler[]> = {};
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn((name: string, handler: Handler) => {
    (handlers[name] ??= []).push(handler);
    return Promise.resolve(() => {});
  }),
}));

function emitProgress(payload: ChatProgress) {
  act(() => {
    for (const handler of handlers["chat-progress"] ?? []) {
      handler({ payload });
    }
  });
}

const sessions: SessionMeta[] = [
  {
    path: "/notes/2026-08-21 legal review.md",
    title: "Berlin rollout — legal review",
    date: "2026-08-21T14:00:00+02:00",
    duration_secs: 2892,
  },
];

const answered: ChatMessage[] = [
  {
    role: "user",
    text: "What did we decide on Berlin?",
    sources: [],
    followups: [],
    label: null,
    asked_at: null,
    question_name: null,
  },
  {
    role: "assistant",
    text: "You hold Berlin until legal clears the review.",
    sources: [
      { file: "2026-08-21 legal review.md", timestamp: "00:21:44" },
      { file: null, timestamp: "00:11:22" },
    ],
    followups: ["What changed since June?"],
    label: "claude/opus",
    asked_at: null,
    question_name: null,
  },
];

beforeEach(() => {
  for (const key of Object.keys(handlers)) delete handlers[key];
  vi.mocked(chatAsk).mockClear().mockResolvedValue(undefined);
  vi.mocked(chatCancel).mockClear();
  vi.mocked(chatHistory).mockReset().mockResolvedValue({ messages: [], running: false });
  vi.mocked(listen).mockClear();
});

describe("chip helpers", () => {
  it("labels file citations with the note's title and a short timestamp", () => {
    expect(
      sourceChipLabel({ file: "2026-08-21 legal review.md", timestamp: "00:21:44" }, sessions),
    ).toBe("Berlin rollout — legal review · 21:44");
    // An unknown file keeps its name (minus the extension); an hour-long
    // timestamp stays whole.
    expect(
      sourceChipLabel({ file: "grocery-list.md", timestamp: "01:02:03" }, sessions),
    ).toBe("grocery-list · 01:02:03");
    expect(sourceChipLabel({ file: "grocery-list.md", timestamp: null }, sessions)).toBe(
      "grocery-list",
    );
    // A bare timestamp is the current call: the mockups' return arrow.
    expect(sourceChipLabel({ file: null, timestamp: "00:11:22" }, sessions)).toBe("↩ 00:11:22");
  });

  it("counts distinct cited notes, the current call included", () => {
    expect(distinctSourceCount(answered[1].sources)).toBe(2);
    expect(
      distinctSourceCount([
        { file: "a.md", timestamp: "00:00:01" },
        { file: "a.md", timestamp: "00:05:00" },
      ]),
    ).toBe(1);
    expect(distinctSourceCount([])).toBe(0);
  });

  it("trims only a zero hour from timestamps", () => {
    expect(shortTimestamp("00:04:30")).toBe("04:30");
    expect(shortTimestamp("01:04:30")).toBe("01:04:30");
  });
});

describe("ChatConversation", () => {
  it("seeds from the backend conversation and renders the exchange", async () => {
    vi.mocked(chatHistory).mockResolvedValue({ messages: answered, running: false });
    render(
      <ChatConversation
        scopePath={null}
        variant="pane"
        sessions={sessions}
        placeholder="Ask anything..."
        onOpenCitation={vi.fn()}
      />,
    );

    expect(await screen.findByText("What did we decide on Berlin?")).toBeInTheDocument();
    // The answer body renders through the lazy Markdown renderer: await it.
    expect(await screen.findByText(/hold Berlin until legal clears/)).toBeInTheDocument();
    // Machine material is labeled with its evidence count and provider.
    expect(screen.getByText(/^From 2 notes$/i)).toBeInTheDocument();
    expect(screen.getByText("claude/opus")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Berlin rollout — legal review · 21:44" }),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "↩ 00:11:22" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "What changed since June?" })).toBeInTheDocument();
  });

  it("focuses the input on mount so opening the surface means typing", async () => {
    render(
      <ChatConversation
        scopePath={null}
        variant="pane"
        sessions={sessions}
        placeholder="Ask anything..."
        onOpenCitation={vi.fn()}
      />,
    );
    const input = screen.getByRole("textbox", { name: "Ask anything..." });
    await waitFor(() => expect(input).toHaveFocus());
  });

  it("sends a question with Enter and shows it as pending", async () => {
    render(
      <ChatConversation
        scopePath="/notes/call.md"
        variant="drawer"
        sessions={sessions}
        placeholder="Ask about this call..."
        onOpenCitation={vi.fn()}
        askedAt={() => "00:15:04"}
      />,
    );
    await waitFor(() => expect(chatHistory).toHaveBeenCalled());

    const input = screen.getByRole("textbox", { name: "Ask about this call..." });
    fireEvent.change(input, { target: { value: "what did I miss?" } });
    fireEvent.keyDown(input, { key: "Enter" });

    expect(chatAsk).toHaveBeenCalledExactlyOnceWith("/notes/call.md", "what did I miss?", "00:15:04", undefined, undefined);
    expect(screen.getByText("what did I miss?")).toBeInTheDocument();
    expect(screen.getByText(/You asked · 00:15:04/i)).toBeInTheDocument();
    expect((input as HTMLInputElement).value).toBe("");
  });

  it("refetches the conversation when its scope reports done", async () => {
    render(
      <ChatConversation
        scopePath={null}
        variant="pane"
        sessions={sessions}
        placeholder="Ask anything..."
        onOpenCitation={vi.fn()}
      />,
    );
    await waitFor(() => expect(chatHistory).toHaveBeenCalledTimes(1));

    // Another scope's lifecycle is not ours: no refetch.
    emitProgress({ scope: "/notes/other.md", phase: "done", message: null });
    expect(chatHistory).toHaveBeenCalledTimes(1);

    vi.mocked(chatHistory).mockResolvedValue({ messages: answered, running: false });
    emitProgress({ scope: "all", phase: "done", message: null });
    expect(await screen.findByText(/hold Berlin until legal clears/)).toBeInTheDocument();
  });

  it("surfaces a failure and restores the question for a retry", async () => {
    render(
      <ChatConversation
        scopePath={null}
        variant="pane"
        sessions={sessions}
        placeholder="Ask anything..."
        onOpenCitation={vi.fn()}
      />,
    );
    await waitFor(() => expect(chatHistory).toHaveBeenCalled());

    const input = screen.getByRole("textbox", { name: "Ask anything..." });
    fireEvent.change(input, { target: { value: "who owns the DPA?" } });
    fireEvent.keyDown(input, { key: "Enter" });
    emitProgress({ scope: "all", phase: "error", message: "Claude Code isn't installed" });

    expect(await screen.findByRole("alert")).toHaveTextContent("Claude Code isn't installed");
    expect((input as HTMLInputElement).value).toBe("who owns the DPA?");
  });

  it("treats a cancel as a quiet dismissal, not an error", async () => {
    render(
      <ChatConversation
        scopePath={null}
        variant="pane"
        sessions={sessions}
        placeholder="Ask anything..."
        onOpenCitation={vi.fn()}
      />,
    );
    await waitFor(() => expect(chatHistory).toHaveBeenCalled());

    const input = screen.getByRole("textbox", { name: "Ask anything..." });
    fireEvent.change(input, { target: { value: "never mind" } });
    fireEvent.keyDown(input, { key: "Enter" });
    emitProgress({ scope: "all", phase: "running", message: null });

    fireEvent.click(screen.getByRole("button", { name: "Stop answering" }));
    expect(chatCancel).toHaveBeenCalledExactlyOnceWith(null);

    emitProgress({ scope: "all", phase: "error", message: "cancelled" });
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect((input as HTMLInputElement).value).toBe("never mind");
  });

  it("routes citation clicks and sends a clicked followup", async () => {
    const onOpenCitation = vi.fn();
    vi.mocked(chatHistory).mockResolvedValue({ messages: answered, running: false });
    render(
      <ChatConversation
        scopePath="/notes/call.md"
        variant="drawer"
        sessions={sessions}
        placeholder="Ask about this call..."
        onOpenCitation={onOpenCitation}
      />,
    );

    fireEvent.click(await screen.findByRole("button", { name: "↩ 00:11:22" }));
    expect(onOpenCitation).toHaveBeenCalledExactlyOnceWith({ file: null, timestamp: "00:11:22" });

    fireEvent.click(screen.getByRole("button", { name: "What changed since June?" }));
    expect(chatAsk).toHaveBeenCalledExactlyOnceWith(
      "/notes/call.md",
      "What changed since June?",
      undefined,
      undefined,
      undefined,
    );
  });

  it("sends a question handed in from outside exactly once", async () => {
    render(
      <ChatConversation
        scopePath={null}
        variant="pane"
        sessions={sessions}
        placeholder="Ask anything..."
        onOpenCitation={vi.fn()}
        askRequest={{ id: 1, question: "berlin rollout" }}
      />,
    );

    await waitFor(() =>
      expect(chatAsk).toHaveBeenCalledExactlyOnceWith(null, "berlin rollout", undefined, undefined, undefined),
    );
  });
});

describe("ChatConversation: quick questions", () => {
  const quick = [
    { id: "miss", name: "What did I miss", prompt: "I stepped away. What was said?", scope: "call" as const, connectors: false },
    { id: "smart", name: "Make me smart", prompt: "Explain the terms in this call.", scope: "call" as const, connectors: false },
    { id: "email", name: "Draft a follow-up email", prompt: "Draft a short follow-up email.", scope: "call" as const, connectors: false },
    { id: "jira", name: "Find related Jira issues", prompt: "Search Jira.", scope: "call" as const, connectors: false },
    { id: "time", name: "Find a time to meet", prompt: "Propose three times.", scope: "call" as const, connectors: false },
  ];

  it("offers the first four as chips, sends the prompt under the question's name", async () => {
    render(
      <ChatConversation
        scopePath="/notes/call.md"
        variant="drawer"
        sessions={sessions}
        placeholder="Ask about this call..."
        quickQuestions={quick}
        onOpenCitation={vi.fn()}
      />,
    );
    const chips = await screen.findByLabelText("Quick questions", { selector: "div" });
    expect(within(chips).getAllByRole("button").map((b) => b.textContent)).toEqual([
      "What did I miss",
      "Make me smart",
      "Draft a follow-up email",
      "Find related Jira issues",
      "1 more",
    ]);
    fireEvent.click(within(chips).getByRole("button", { name: "Make me smart" }));
    expect(chatAsk).toHaveBeenCalledExactlyOnceWith(
      "/notes/call.md",
      "Explain the terms in this call.",
      undefined,
      "Make me smart",
      false,
    );
    // The pending question shows its name, not the prompt it sent.
    expect(screen.getByText("Make me smart", { selector: "p" })).toBeInTheDocument();
    expect(screen.queryByText("Explain the terms in this call.")).toBeNull();
  });

  it("opens a filtered list on / and picks with Enter; a stored quick question renders by name", async () => {
    vi.mocked(chatHistory).mockResolvedValue({
      messages: [
        {
          role: "user",
          text: "Search Jira.",
          sources: [],
          followups: [],
          label: null,
          asked_at: null,
          question_name: "Find related Jira issues",
        },
        { ...answered[1], text: "Two issues: SIM-12 and SIM-40." },
      ],
      running: false,
    });
    render(
      <ChatConversation
        scopePath="/notes/call.md"
        variant="drawer"
        sessions={sessions}
        placeholder="Ask about this call..."
        quickQuestions={quick}
        onOpenCitation={vi.fn()}
      />,
    );
    expect(await screen.findByText("Find related Jira issues", { selector: "p" })).toBeInTheDocument();
    expect(screen.queryByText("Search Jira.")).toBeNull();

    const input = screen.getByRole("textbox", { name: "Ask about this call..." });
    fireEvent.change(input, { target: { value: "/time" } });
    const list = screen.getByRole("listbox", { name: "Quick questions" });
    expect(within(list).getAllByRole("option").map((o) => o.textContent)).toEqual([
      "Find a time to meetPropose three times.",
    ]);
    fireEvent.keyDown(input, { key: "Enter" });
    expect(chatAsk).toHaveBeenCalledExactlyOnceWith(
      "/notes/call.md",
      "Propose three times.",
      undefined,
      "Find a time to meet",
      false,
    );
    expect(input).toHaveValue("");
    expect(screen.queryByRole("listbox")).toBeNull();
  });

  it("shows the lightning button only when there are quick questions, and it opens the list", async () => {
    const { rerender } = render(
      <ChatConversation
        scopePath={null}
        variant="pane"
        sessions={sessions}
        placeholder="Ask anything..."
        onOpenCitation={vi.fn()}
      />,
    );
    expect(screen.queryByRole("button", { name: "Quick questions" })).toBeNull();
    rerender(
      <ChatConversation
        scopePath={null}
        variant="pane"
        sessions={sessions}
        placeholder="Ask anything..."
        quickQuestions={quick.slice(0, 2)}
        onOpenCitation={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Quick questions" }));
    expect(screen.getByRole("textbox", { name: "Ask anything..." })).toHaveValue("/");
    expect(within(screen.getByRole("listbox", { name: "Quick questions" })).getAllByRole("option")).toHaveLength(2);
    // Escape closes it and clears the slash.
    fireEvent.keyDown(screen.getByRole("textbox", { name: "Ask anything..." }), { key: "Escape" });
    expect(screen.queryByRole("listbox")).toBeNull();
  });
});

describe("ChatConversation: progress steps", () => {
  it("shows what the assistant is reading and counts the steps while it works", async () => {
    render(
      <ChatConversation
        scopePath={null}
        variant="pane"
        sessions={sessions}
        placeholder="Ask anything..."
        onOpenCitation={vi.fn()}
      />,
    );
    await waitFor(() => expect(chatHistory).toHaveBeenCalled());
    emitProgress({ scope: "all", phase: "running", message: null });
    expect(screen.getByText("reading your notes")).toBeInTheDocument();
    emitProgress({ scope: "all", phase: "step", message: "Searching for \u201csalesforce\u201d" });
    expect(screen.getByText("Searching for \u201csalesforce\u201d")).toBeInTheDocument();
    emitProgress({ scope: "all", phase: "step", message: "Reading 2026-09-03 14-01-09 call.md" });
    expect(screen.getByText("Reading 2026-09-03 14-01-09 call.md · 2 steps")).toBeInTheDocument();
    // Another scope's steps are not ours.
    emitProgress({ scope: "/notes/other.md", phase: "step", message: "Reading other.md" });
    expect(screen.queryByText(/Reading other.md/)).toBeNull();
    emitProgress({ scope: "all", phase: "done", message: null });
    await waitFor(() => expect(screen.queryByText(/steps/)).toBeNull());
  });
});
