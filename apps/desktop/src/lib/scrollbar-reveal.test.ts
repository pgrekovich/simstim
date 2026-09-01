import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { installScrollbarReveal } from "@/lib/scrollbar-reveal";

describe("installScrollbarReveal", () => {
  let teardown: () => void;
  let container: HTMLDivElement;

  beforeEach(() => {
    vi.useFakeTimers();
    container = document.createElement("div");
    document.body.appendChild(container);
    teardown = installScrollbarReveal();
  });

  afterEach(() => {
    teardown();
    container.remove();
    vi.useRealTimers();
  });

  it("tags the scrolled element while scrolling and untags it after the linger delay", () => {
    container.dispatchEvent(new Event("scroll"));
    expect(container.classList.contains("is-scrolling")).toBe(true);

    vi.advanceTimersByTime(699);
    expect(container.classList.contains("is-scrolling")).toBe(true);
    vi.advanceTimersByTime(1);
    expect(container.classList.contains("is-scrolling")).toBe(false);
  });

  it("keeps the tag alive across a continuous scroll (each event resets the delay)", () => {
    container.dispatchEvent(new Event("scroll"));
    vi.advanceTimersByTime(500);
    container.dispatchEvent(new Event("scroll"));

    // Past the first event's deadline, but within the second's.
    vi.advanceTimersByTime(500);
    expect(container.classList.contains("is-scrolling")).toBe(true);
    vi.advanceTimersByTime(200);
    expect(container.classList.contains("is-scrolling")).toBe(false);
  });

  it("tracks concurrently scrolling containers independently", () => {
    const other = document.createElement("div");
    document.body.appendChild(other);

    container.dispatchEvent(new Event("scroll"));
    vi.advanceTimersByTime(400);
    other.dispatchEvent(new Event("scroll"));
    vi.advanceTimersByTime(300);

    expect(container.classList.contains("is-scrolling")).toBe(false);
    expect(other.classList.contains("is-scrolling")).toBe(true);
    other.remove();
  });

  it("teardown removes the listener and clears any lingering tags", () => {
    container.dispatchEvent(new Event("scroll"));
    expect(container.classList.contains("is-scrolling")).toBe(true);

    teardown();
    expect(container.classList.contains("is-scrolling")).toBe(false);
    container.dispatchEvent(new Event("scroll"));
    expect(container.classList.contains("is-scrolling")).toBe(false);
    // afterEach calls teardown again; it must be idempotent.
  });
});
