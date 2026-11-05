import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { HEARD_WITHIN_MS, HearingIndicator } from "@/components/HearingIndicator";

describe("HearingIndicator", () => {
  it("lights a channel heard within the window and dims one that has gone quiet, saying for how long", () => {
    render(
      <HearingIndicator status={{ me_silent_ms: 1200, them_silent_ms: 95_000, mic: "WH-1000XM5" }} />,
    );

    expect(screen.getByTestId("hearing-you")).toHaveAttribute("data-heard", "true");
    expect(screen.getByTestId("hearing-them")).toHaveAttribute("data-heard", "false");
    expect(screen.getByTitle("Hearing you via WH-1000XM5")).toBeInTheDocument();
    expect(screen.getByTitle("Nothing from them for 2 min")).toBeInTheDocument();
  });

  it("treats the window edge as still heard and reports short gaps in seconds", () => {
    render(
      <HearingIndicator
        status={{ me_silent_ms: HEARD_WITHIN_MS, them_silent_ms: 14_000, mic: "MacBook Pro Microphone" }}
      />,
    );

    expect(screen.getByTestId("hearing-you")).toHaveAttribute("data-heard", "true");
    expect(screen.getByTitle("Hearing you via MacBook Pro Microphone")).toBeInTheDocument();
    expect(screen.getByTitle("Nothing from them for 14s")).toBeInTheDocument();
  });

  it("names the microphone on a quiet you-channel so a dark dot can be diagnosed", () => {
    render(
      <HearingIndicator status={{ me_silent_ms: 20_000, them_silent_ms: 500, mic: "Brio 500" }} />,
    );

    expect(screen.getByTitle("Nothing from you for 20s via Brio 500")).toBeInTheDocument();
  });

  it("shows both dots dim before the first reading lands", () => {
    render(<HearingIndicator status={null} />);

    expect(screen.getByTestId("hearing-you")).toHaveAttribute("data-heard", "false");
    expect(screen.getByTestId("hearing-them")).toHaveAttribute("data-heard", "false");
    expect(screen.getAllByTitle("Listening...")).toHaveLength(2);
  });
});
