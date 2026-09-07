import { describe, expect, it } from "vitest";
import { describePeople, formatPerson } from "@/lib/people";

describe("formatPerson", () => {
  it("turns an address into a readable name and keeps real names as they are", () => {
    expect(formatPerson("m.suwald@demoboost.com")).toBe("M. Suwald");
    expect(formatPerson("anna.berg@x.io")).toBe("Anna Berg");
    expect(formatPerson("mark-ivanov+cal@x.io")).toBe("Mark Ivanov");
    expect(formatPerson("pavel@x.io")).toBe("Pavel");
    expect(formatPerson("Anna Berg")).toBe("Anna Berg");
  });
});

describe("describePeople", () => {
  it("spells out a small roster and counts the rest of a big one", () => {
    expect(describePeople([])).toBeNull();
    expect(describePeople(["al@x.io", "Bob"])).toEqual({ summary: "Al, Bob", full: "Al, Bob" });
    const big = describePeople(["al@x", "bo@x", "cy@x", "di@x", "ed@x", "fy@x"]);
    expect(big?.summary).toBe("Al, Bo, Cy, Di and 2 others");
    expect(big?.full).toBe("Al, Bo, Cy, Di, Ed, Fy");
    expect(describePeople(["al@x", "bo@x", "cy@x", "di@x", "ed@x"])?.summary).toBe("Al, Bo, Cy, Di and 1 other");
  });
});
