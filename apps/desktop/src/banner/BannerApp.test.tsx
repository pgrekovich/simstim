import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { emitTo, listen } from "@tauri-apps/api/event";
import BannerApp, { AUTO_DISMISS_MS } from "@/banner/BannerApp";
import * as ipc from "@/lib/ipc";

/**
 * BannerApp talks to two Tauri surfaces: the `@/lib/ipc` wrappers (seed
 * state, dismiss, open-main-window) and `@tauri-apps/api/event` directly
 * (`listen` for banner-state pushes, `emitTo` toward the main window).
 */
vi.mock("@/lib/ipc", () => ({
  getPrompts: vi.fn().mockResolvedValue({ summary_instructions: "", summary_styles: [], quick_questions: [] }),
  savePrompts: vi.fn().mockResolvedValue({ summary_instructions: "", summary_styles: [], quick_questions: [] }),
  restoreDefaultPrompts: vi.fn().mockResolvedValue({ summary_instructions: "", summary_styles: [], quick_questions: [] }),
  getConfig: vi.fn().mockResolvedValue({ theme: "system" }),
  bannerCurrentState: vi.fn().mockResolvedValue({ kind: "hidden" }),
  bannerDismiss: vi.fn().mockResolvedValue(undefined),
  showMainWindow: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
  emitTo: vi.fn(() => Promise.resolve()),
}));

/** Fires the handler BannerApp registered for `name` (last registration). */
function emit(name: string, payload: unknown) {
  const calls = vi.mocked(listen).mock.calls.filter(([eventName]) => eventName === name);
  const call = calls[calls.length - 1];
  if (!call) throw new Error(`no listener registered for "${name}"`);
  const handler = call[1] as (event: { payload: unknown }) => void;
  act(() => {
    handler({ payload });
  });
}

const DETECTED = { kind: "detected", bundle_id: "us.zoom.xos", app_name: "Zoom" } as const;

async function renderDetected() {
  render(<BannerApp />);
  // BannerApp registers `listen` synchronously in its mount effect; one
  // microtask flush settles the seed promise and works in BOTH timer modes
  // (waitFor is unreliable under vitest fake timers).
  await act(async () => {});
  emit("banner-state", DETECTED);
  screen.getByText("Zoom");
}

beforeEach(() => {
  vi.mocked(ipc.bannerCurrentState).mockClear().mockResolvedValue({ kind: "hidden" });
  vi.mocked(ipc.bannerDismiss).mockClear();
  vi.mocked(ipc.showMainWindow).mockClear();
  vi.mocked(listen).mockClear();
  vi.mocked(emitTo).mockClear();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("BannerApp", () => {
  it("renders nothing while hidden", async () => {
    const { container } = render(<BannerApp />);
    await waitFor(() => expect(ipc.bannerCurrentState).toHaveBeenCalled());
    expect(container).toBeEmptyDOMElement();
  });

  it("seeds its state from banner_current_state on mount", async () => {
    vi.mocked(ipc.bannerCurrentState).mockResolvedValue(DETECTED);
    render(<BannerApp />);
    expect(await screen.findByText("Call detected")).toBeVisible();
    expect(screen.getByText("Zoom")).toBeVisible();
  });

  it("renders the detected state from a banner-state event", async () => {
    await renderDetected();
    expect(screen.getByText("Call detected")).toBeVisible();
    expect(screen.getByRole("button", { name: /start recording/i })).toBeVisible();
  });

  it("renders the ended state from a banner-state event", async () => {
    await renderDetected();
    emit("banner-state", { kind: "ended" });
    expect(await screen.findByText("Call seems over")).toBeVisible();
    expect(screen.getByRole("button", { name: /^stop$/i })).toBeVisible();
  });

  it("start forwards to the main window with the detection payload", async () => {
    await renderDetected();
    fireEvent.click(screen.getByRole("button", { name: /start recording/i }));
    expect(emitTo).toHaveBeenCalledWith("main", "banner-start", {
      bundle_id: "us.zoom.xos",
      app_name: "Zoom",
    });
  });

  it("stop forwards to the main window", async () => {
    await renderDetected();
    emit("banner-state", { kind: "ended" });
    fireEvent.click(await screen.findByRole("button", { name: /^stop$/i }));
    expect(emitTo).toHaveBeenCalledWith("main", "banner-stop");
  });

  it("reveals the close button on hover only, and it dismisses", async () => {
    await renderDetected();
    expect(screen.queryByRole("button", { name: /dismiss/i })).not.toBeInTheDocument();

    fireEvent.mouseEnter(screen.getByRole("status"));
    fireEvent.click(screen.getByRole("button", { name: /dismiss/i }));
    expect(ipc.bannerDismiss).toHaveBeenCalledTimes(1);
  });

  it("offers the close button in the ended state too", async () => {
    await renderDetected();
    emit("banner-state", { kind: "ended" });
    fireEvent.mouseEnter(screen.getByRole("status"));
    fireEvent.click(screen.getByRole("button", { name: /dismiss/i }));
    expect(ipc.bannerDismiss).toHaveBeenCalledTimes(1);
  });

  it("open simstim lives in the dropdown, focuses the main window, keeps the banner", async () => {
    await renderDetected();
    fireEvent.click(screen.getByRole("button", { name: /more options/i }));
    fireEvent.click(screen.getByRole("button", { name: "Open Simstim" }));
    expect(ipc.showMainWindow).toHaveBeenCalledTimes(1);
    expect(screen.getByText("Call detected")).toBeVisible();
    // The menu closed on selection.
    expect(screen.queryByRole("button", { name: "Open Simstim" })).not.toBeInTheDocument();
  });

  it("closes the dropdown when a new state arrives", async () => {
    await renderDetected();
    fireEvent.click(screen.getByRole("button", { name: /more options/i }));
    expect(screen.getByRole("button", { name: "Open Simstim" })).toBeVisible();

    emit("banner-state", { kind: "ended" });
    expect(screen.queryByRole("button", { name: "Open Simstim" })).not.toBeInTheDocument();
  });

  it("auto-dismisses the detected state exactly once when the timer drains", async () => {
    vi.useFakeTimers();
    await renderDetected();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(AUTO_DISMISS_MS + 1_000);
    });
    expect(ipc.bannerDismiss).toHaveBeenCalledTimes(1);
  });

  it("hovering pauses the auto-dismiss timer", async () => {
    vi.useFakeTimers();
    await renderDetected();
    fireEvent.mouseEnter(screen.getByRole("status"));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(AUTO_DISMISS_MS * 2);
    });
    expect(ipc.bannerDismiss).not.toHaveBeenCalled();

    fireEvent.mouseLeave(screen.getByRole("status"));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(AUTO_DISMISS_MS + 1_000);
    });
    expect(ipc.bannerDismiss).toHaveBeenCalledTimes(1);
  });

  it("a hide while hovered does not pause the next banner's timer", async () => {
    vi.useFakeTimers();
    await renderDetected();
    // Hover, then let the backend hide the banner (Start, auto-stop or a
    // cleared detection): the card unmounts with the pointer inside it, so
    // no mouseleave ever fires.
    fireEvent.mouseEnter(screen.getByRole("status"));
    emit("banner-state", { kind: "hidden" });
    vi.mocked(ipc.bannerDismiss).mockClear();

    emit("banner-state", { ...DETECTED, bundle_id: "com.hnc.Discord", app_name: "Discord" });
    screen.getByText("Discord");
    await act(async () => {
      await vi.advanceTimersByTimeAsync(AUTO_DISMISS_MS + 1_000);
    });
    expect(ipc.bannerDismiss).toHaveBeenCalledTimes(1);
  });

  it("the ended state has no auto-dismiss", async () => {
    vi.useFakeTimers();
    await renderDetected();
    emit("banner-state", { kind: "ended" });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(AUTO_DISMISS_MS * 2);
    });
    expect(ipc.bannerDismiss).not.toHaveBeenCalled();
    expect(screen.getByText("Call seems over")).toBeVisible();
  });
});
