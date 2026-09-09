import { describe, expect, it } from "vitest";
import { describePeople, formatPerson, isGroupAddress } from "@/lib/people";

describe("formatPerson", () => {
  it("turns an address into a readable name and keeps real names as they are", () => {
    expect(formatPerson("m.rivera@example.com")).toBe("M. Rivera");
    expect(formatPerson("anna.berg@x.io")).toBe("Anna Berg");
    expect(formatPerson("mark-ivanov+cal@x.io")).toBe("Mark Ivanov");
    expect(formatPerson("alex@x.io")).toBe("Alex");
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

describe("isGroupAddress", () => {
  it("drops mailing lists and their bare display names, keeps people", () => {
    expect(isGroupAddress("internal@example.com")).toBe(true);
    expect(isGroupAddress("internal")).toBe(true);
    expect(isGroupAddress("Internal")).toBe(true);
    expect(isGroupAddress("dev-team@x.io")).toBe(true);
    expect(isGroupAddress("all-hands@x.io")).toBe(true);
    expect(isGroupAddress("m.rivera@example.com")).toBe(false);
    expect(isGroupAddress("Anna Berg")).toBe(false);
    expect(isGroupAddress("Alex")).toBe(false);
    // A note whose roster was only a list shows no people line at all.
    expect(describePeople(["internal"])).toBeNull();
    expect(describePeople(["internal@x.io", "Anna Berg"])?.summary).toBe("Anna Berg");
  });
});
