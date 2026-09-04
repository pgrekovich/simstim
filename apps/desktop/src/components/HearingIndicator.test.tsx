import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { HEARD_WITHIN_MS, HearingIndicator } from "@/components/HearingIndicator";

describe("HearingIndicator", () => {
  it("lights a channel heard within the window and dims one that has gone quiet, saying for how long", () => {
    render(<HearingIndicator status={{ me_silent_ms: 1200, them_silent_ms: 95_000 }} />);

    expect(screen.getByTestId("hearing-you")).toHaveAttribute("data-heard", "true");
    expect(screen.getByTestId("hearing-them")).toHaveAttribute("data-heard", "false");
    expect(screen.getByTitle("Hearing you")).toBeInTheDocument();
    expect(screen.getByTitle("Nothing from them for 2 min")).toBeInTheDocument();
  });

  it("treats the window edge as still heard and reports short gaps in seconds", () => {
    render(<HearingIndicator status={{ me_silent_ms: HEARD_WITHIN_MS, them_silent_ms: 14_000 }} />);

    expect(screen.getByTestId("hearing-you")).toHaveAttribute("data-heard", "true");
    expect(screen.getByTitle("Nothing from them for 14s")).toBeInTheDocument();
  });

  it("shows both dots dim before the first reading lands", () => {
    render(<HearingIndicator status={null} />);

    expect(screen.getByTestId("hearing-you")).toHaveAttribute("data-heard", "false");
    expect(screen.getByTestId("hearing-them")).toHaveAttribute("data-heard", "false");
    expect(screen.getAllByTitle("Listening...")).toHaveLength(2);
  });
});
