import { describe, expect, it } from "vitest";
import { describeFinishing, describeRetranscribeProgress } from "@/lib/retranscribeStatus";

describe("describeFinishing", () => {
  it("names the whole-pass percent when the backend has one, rounded", () => {
    expect(describeFinishing(43.4)).toBe("finishing 43%");
    expect(describeFinishing(99.6)).toBe("finishing 100%");
  });

  it("falls back to the bare word before the first percent lands", () => {
    expect(describeFinishing(null)).toBe("finishing");
  });
});

describe("describeRetranscribeProgress", () => {
  it("renders a skipped pass from its message", () => {
    expect(
      describeRetranscribeProgress({
        path: "/tmp/a.md",
        phase: "skipped",
        channel: null,
        message: "this note is recording again; the accurate pass runs after it stops",
        percent: null,
      }),
    ).toBe("this note is recording again; the accurate pass runs after it stops");
  });

  it("keeps the per-channel detail for a transcribing phase", () => {
    expect(
      describeRetranscribeProgress({ path: "/tmp/a.md", phase: "transcribing", channel: "me", message: "12%", percent: 12 }),
    ).toBe("transcribing (me) 12%...");
  });
});
