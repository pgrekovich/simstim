import { describe, expect, it } from "vitest";
import { downloadProgressPercent, mergeDownloadProgress } from "./modelProgress";

describe("model download progress", () => {
  it("ignores an out-of-order event that would move the bar backwards", () => {
    const current = { id: "model", phase: "download" as const, received: 60, total: 100 };
    const stale = { id: "model", phase: "download" as const, received: 40, total: 100 };

    expect(mergeDownloadProgress(current, stale)).toBe(current);
  });

  it("does not let a late byte event replace a completed download", () => {
    const done = { id: "model", phase: "done" as const, received: 0, total: 0 };
    const late = { id: "model", phase: "download" as const, received: 99, total: 100 };

    expect(mergeDownloadProgress(done, late)).toBe(done);
  });

  it("clamps unexpected byte counts to a valid percentage", () => {
    expect(downloadProgressPercent({
      id: "model",
      phase: "download",
      received: 110,
      total: 100,
    })).toBe(100);
  });
});
