import { describe, expect, it } from "vitest";
import { formatDuration } from "@/lib/utils";

describe("formatDuration", () => {
  it("speaks in minutes and hours, never as a clock time", () => {
    expect(formatDuration(null)).toBeNull();
    expect(formatDuration(20)).toBe("< 1 min");
    expect(formatDuration(611)).toBe("10 min");
    expect(formatDuration(1874)).toBe("31 min");
    expect(formatDuration(3600)).toBe("1 h");
    expect(formatDuration(6900)).toBe("1 h 55 min");
  });
});
