import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { findTranscriptMatches, nearestLineIndex, TranscriptView } from "@/components/TranscriptView";

describe("TranscriptView: transcript find (stage 2.1)", () => {
  it("finds every case-insensitive occurrence in finalized spoken text", () => {
    expect(findTranscriptMatches([
      "[00:00:01] Me: Berlin then berlin",
      "[00:00:02] Them: no match",
      "hand-added Berlin annotation",
    ], "berlin")).toEqual([
      { lineIndex: 0, start: 0, end: 6 },
      { lineIndex: 0, start: 12, end: 18 },
      { lineIndex: 2, start: 11, end: 17 },
    ]);
  });

  it("treats regular-expression punctuation as plain text", () => {
    expect(findTranscriptMatches(["[00:00:01] Me: ship v2.1, not v201"], "v2.1"))
      .toEqual([{ lineIndex: 0, start: 5, end: 9 }]);
  });

  it("preserves text while distinguishing the active match", () => {
    const lines = ["[00:00:01] Me: Berlin then berlin"];
    const matches = findTranscriptMatches(lines, "berlin");
    render(<TranscriptView lines={lines} matches={matches} activeMatchIndex={1} />);

    const marks = screen.getAllByText(/berlin/i);
    expect(marks).toHaveLength(2);
    expect(marks[0]).not.toHaveAttribute("data-current-match");
    expect(marks[1]).toHaveAttribute("data-current-match", "true");
    expect(screen.getByText(/then/, { selector: "span" })).toHaveTextContent("Berlin then berlin");
  });
});

describe("TranscriptView: consecutive same-speaker lines group into one turn", () => {
  it("shows one speaker label per turn, with the turn's first timestamp", () => {
    render(
      <TranscriptView
        lines={[
          "[00:00:02] Me: No, it's not a background.",
          "[00:00:06] Me: Yeah.",
          "[00:00:10] Me: Yeah.",
          "[00:00:15] Them: Okay.",
        ]}
      />,
    );

    expect(screen.getAllByText("Me")).toHaveLength(1);
    expect(screen.getAllByText("Them")).toHaveLength(1);
    expect(screen.getByText("00:00:02")).toBeInTheDocument();
    // The grouped lines' own timestamps are not printed as headers.
    expect(screen.queryByText("00:00:06")).toBeNull();
    expect(screen.queryByText("00:00:10")).toBeNull();
    expect(screen.getByText("00:00:15")).toBeInTheDocument();
  });

  it("keeps every grouped chunk individually seekable at its own offset", () => {
    const onSeek = vi.fn();
    render(
      <TranscriptView
        lines={["[00:00:02] Me: first chunk", "[00:01:10] Me: second chunk"]}
        onSeek={onSeek}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: /second chunk/i }));
    expect(onSeek).toHaveBeenCalledWith(70);
    fireEvent.click(screen.getByRole("button", { name: /first chunk/i }));
    expect(onSeek).toHaveBeenCalledWith(2);
  });

  it("breaks the grouping around a line the pattern cannot parse", () => {
    render(
      <TranscriptView
        lines={[
          "[00:00:02] Me: before",
          "-- hand-added annotation --",
          "[00:00:09] Me: after",
        ]}
      />,
    );

    // The annotation splits one speaker into two labeled turns.
    expect(screen.getAllByText("Me")).toHaveLength(2);
    expect(screen.getByText(/hand-added annotation/)).toBeInTheDocument();
    expect(screen.getByText("00:00:02")).toBeInTheDocument();
    expect(screen.getByText("00:00:09")).toBeInTheDocument();
  });

  it("still finds and highlights matches inside a grouped turn", () => {
    const lines = ["[00:00:02] Me: Berlin first", "[00:00:06] Me: then berlin again"];
    const matches = findTranscriptMatches(lines, "berlin");
    render(<TranscriptView lines={lines} matches={matches} activeMatchIndex={1} />);

    const marks = screen.getAllByText(/berlin/i);
    expect(marks).toHaveLength(2);
    expect(marks[1]).toHaveAttribute("data-current-match", "true");
  });
});

describe("TranscriptView: click-to-seek (Task 4, stage-0.7)", () => {
  it("calls onSeek with the offset in seconds parsed from a clicked line's timestamp", () => {
    const onSeek = vi.fn();
    render(<TranscriptView lines={["[00:01:05] Me: hello there"]} onSeek={onSeek} />);

    fireEvent.click(screen.getByRole("button", { name: /hello there/i }));

    expect(onSeek).toHaveBeenCalledTimes(1);
    expect(onSeek).toHaveBeenCalledWith(65);
  });

  it("renders finalized lines as plain, non-interactive text when onSeek is not provided", () => {
    render(<TranscriptView lines={["[00:00:03] Me: hi"]} />);

    expect(screen.getByText(/hi/)).toBeInTheDocument();
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("never makes interim (draft) lines seekable, even when onSeek is provided", () => {
    const onSeek = vi.fn();
    render(
      <TranscriptView
        lines={["[00:00:01] Me: hi"]}
        interimMe="still talking"
        interimThem="also still talking"
        onSeek={onSeek}
      />
    );

    // The finalized line above is seekable...
    expect(screen.getByRole("button", { name: /hi/i })).toBeInTheDocument();
    // ...but neither interim draft is.
    expect(screen.getByText(/^still talking$/)).toBeInTheDocument();
    expect(screen.getByText(/^also still talking$/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /still talking/i })).toBeNull();
  });

  it("keeps a line with no recognizable [hh:mm:ss] timestamp non-interactive even when onSeek is provided", () => {
    const onSeek = vi.fn();
    render(<TranscriptView lines={["-- a hand-added annotation, not a transcript line --"]} onSeek={onSeek} />);

    expect(screen.queryByRole("button")).toBeNull();
    expect(onSeek).not.toHaveBeenCalled();
  });

  it("parses hours and minutes correctly, not just seconds", () => {
    const onSeek = vi.fn();
    render(<TranscriptView lines={["[01:02:03] Them: long call"]} onSeek={onSeek} />);

    fireEvent.click(screen.getByRole("button", { name: /long call/i }));

    expect(onSeek).toHaveBeenCalledWith(1 * 3600 + 2 * 60 + 3);
  });
});

describe("TranscriptView: citation jump (stage 4.2)", () => {
  const lines = [
    "[00:00:05] Me: opening",
    "[00:10:00] Them: berlin stays on hold",
    "[00:20:00] Me: closing",
  ];

  it("picks the line nearest the cited timestamp", () => {
    expect(nearestLineIndex(lines, "00:10:00")).toBe(1);
    // Not an exact line (a re-transcribe shifted timestamps): nearest wins.
    expect(nearestLineIndex(lines, "00:11:30")).toBe(1);
    expect(nearestLineIndex(lines, "00:19:00")).toBe(2);
    expect(nearestLineIndex(["no timestamps here"], "00:00:01")).toBe(-1);
  });

  it("centers the cited line when a jump request arrives", () => {
    const scrollIntoView = vi.fn();
    Element.prototype.scrollIntoView = scrollIntoView;
    const { rerender } = render(<TranscriptView lines={lines} jumpRequest={null} />);
    expect(scrollIntoView).not.toHaveBeenCalled();

    rerender(<TranscriptView lines={lines} jumpRequest={{ id: 1, timestamp: "00:10:00" }} />);
    expect(scrollIntoView).toHaveBeenCalledWith({ block: "center" });

    // The same request id never re-fires on unrelated re-renders.
    scrollIntoView.mockClear();
    rerender(<TranscriptView lines={lines} jumpRequest={{ id: 1, timestamp: "00:10:00" }} />);
    expect(scrollIntoView).not.toHaveBeenCalled();
  });
});

describe("TranscriptView: selecting text never seeks", () => {
  it("ignores the click that ends a text selection", () => {
    const onSeek = vi.fn();
    render(<TranscriptView lines={["[00:00:04] Me: copy this passage"]} onSeek={onSeek} />);
    const original = window.getSelection;
    window.getSelection = () => ({ toString: () => "copy this" }) as unknown as Selection;
    try {
      fireEvent.click(screen.getByRole("button", { name: /copy this passage/i }));
      expect(onSeek).not.toHaveBeenCalled();
    } finally {
      window.getSelection = original;
    }
    fireEvent.click(screen.getByRole("button", { name: /copy this passage/i }));
    expect(onSeek).toHaveBeenCalledWith(4);
  });
});
