import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AskCallDrawer, recordingClock } from "@/components/AskCallDrawer";
import { AskNotesView, formatHours } from "@/components/AskNotesView";
import { chatClear, chatHistory, storageStats } from "@/lib/ipc";

vi.mock("@/lib/ipc", () => ({
  getPrompts: vi.fn().mockResolvedValue({ summary_instructions: "", summary_styles: [], quick_questions: [] }),
  savePrompts: vi.fn().mockResolvedValue({ summary_instructions: "", summary_styles: [], quick_questions: [] }),
  restoreDefaultPrompts: vi.fn().mockResolvedValue({ summary_instructions: "", summary_styles: [], quick_questions: [] }),
  chatAsk: vi.fn().mockResolvedValue(undefined),
  chatCancel: vi.fn().mockResolvedValue(undefined),
  chatClear: vi.fn().mockResolvedValue(undefined),
  chatHistory: vi.fn().mockResolvedValue({ messages: [], running: false }),
  storageStats: vi.fn().mockResolvedValue({
    note_count: 184,
    notes_bytes: 1024,
    audio_bytes: 2048,
    audio_secs: 61 * 3600,
  }),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

beforeEach(() => {
  vi.mocked(chatClear).mockClear();
  vi.mocked(chatHistory).mockClear().mockResolvedValue({ messages: [], running: false });
  vi.mocked(storageStats).mockClear();
});

describe("formatHours", () => {
  it("rounds to hours and falls back to minutes for short folders", () => {
    expect(formatHours(61 * 3600)).toBe("61 h");
    expect(formatHours(45 * 60)).toBe("45 min");
    expect(formatHours(10)).toBe("1 min");
  });
});

describe("AskNotesView", () => {
  it("shows the folder badge and asks over the folder scope", async () => {
    render(<AskNotesView sessions={[]} onOpenCitation={vi.fn()} />);

    expect(await screen.findByText("184 notes · 61 h")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Ask your notes" })).toBeInTheDocument();
    // The conversation is folder-scoped: seeded with the null path.
    await waitFor(() => expect(chatHistory).toHaveBeenCalledWith(null));
  });

  it("clears the folder conversation from the header", async () => {
    render(<AskNotesView sessions={[]} onOpenCitation={vi.fn()} />);
    await waitFor(() => expect(chatHistory).toHaveBeenCalled());

    fireEvent.click(screen.getByRole("button", { name: "Clear conversation" }));
    await waitFor(() => expect(chatClear).toHaveBeenCalledExactlyOnceWith(null));
  });
});

describe("recordingClock", () => {
  it("formats elapsed time like the Stop button's clock", () => {
    const startedAt = 1_000_000;
    expect(recordingClock(startedAt, startedAt + 15 * 60_000 + 4_000)).toBe("00:15:04");
    expect(recordingClock(startedAt, startedAt + 3_600_000)).toBe("01:00:00");
    expect(recordingClock(startedAt, startedAt - 5_000)).toBe("00:00:00");
  });
});

describe("AskCallDrawer", () => {
  it("scopes the conversation to the open note and can close", async () => {
    const onClose = vi.fn();
    render(
      <AskCallDrawer
        path="/notes/call.md"
        sessions={[]}
        recordingStartedAt={null}
        onClose={onClose}
        onOpenCitation={vi.fn()}
      />,
    );

    expect(
      screen.getByRole("complementary", { name: "Ask this call" }),
    ).toBeInTheDocument();
    await waitFor(() => expect(chatHistory).toHaveBeenCalledWith("/notes/call.md"));

    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(onClose).toHaveBeenCalledOnce();
  });
});
