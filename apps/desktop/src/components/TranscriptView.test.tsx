import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { TranscriptView } from "@/components/TranscriptView";

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
