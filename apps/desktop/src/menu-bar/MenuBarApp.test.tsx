import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { emitTo, listen } from "@tauri-apps/api/event";
import MenuBarApp from "@/menu-bar/MenuBarApp";
import * as ipc from "@/lib/ipc";

vi.mock("@/lib/ipc", () => ({
  getConfig: vi.fn().mockResolvedValue({ theme: "system" }),
  menuBarCurrentState: vi.fn().mockResolvedValue({ kind: "idle" }),
  menuBarHide: vi.fn().mockResolvedValue(undefined),
  showMainWindow: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
  emitTo: vi.fn(() => Promise.resolve()),
}));

function emit(name: string, payload: unknown) {
  const calls = vi.mocked(listen).mock.calls.filter(([eventName]) => eventName === name);
  const call = calls[calls.length - 1];
  if (!call) throw new Error(`no listener registered for "${name}"`);
  const handler = call[1] as (event: { payload: unknown }) => void;
  act(() => handler({ payload }));
}

beforeEach(() => {
  vi.mocked(ipc.menuBarCurrentState).mockReset().mockResolvedValue({ kind: "idle" });
  vi.mocked(ipc.menuBarHide).mockClear();
  vi.mocked(ipc.showMainWindow).mockClear();
  vi.mocked(listen).mockClear();
  vi.mocked(emitTo).mockClear();
});

describe("MenuBarApp", () => {
  it("renders the idle state and its global shortcut", async () => {
    render(<MenuBarApp />);
    expect(await screen.findByText("Ready to record")).toBeVisible();
    expect(screen.getByText(/⌘⇧N starts a new recording/)).toBeVisible();
  });

  it("forwards start to the main coordinator then hides", async () => {
    render(<MenuBarApp />);
    fireEvent.click(await screen.findByRole("button", { name: /start recording/i }));
    expect(emitTo).toHaveBeenCalledWith("main", "menu-bar-toggle-recording");
    await vi.waitFor(() => expect(ipc.menuBarHide).toHaveBeenCalledTimes(1));
  });

  it("renders pushed recording state and forwards stop", async () => {
    render(<MenuBarApp />);
    await act(async () => {});
    emit("menu-bar-state", {
      kind: "recording",
      title: "Pricing forum",
      path: "/tmp/pricing.md",
      started_at_ms: Date.now() - 72_000,
    });
    expect(screen.getByText("Pricing forum")).toBeVisible();
    expect(screen.getByText(/Recording locally · 01:12/)).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: /^stop$/i }));
    expect(emitTo).toHaveBeenCalledWith("main", "menu-bar-toggle-recording");
  });

  it("seeds a recording that began before the panel opened", async () => {
    vi.mocked(ipc.menuBarCurrentState).mockResolvedValue({
      kind: "recording",
      title: "Daily",
      path: "/tmp/daily.md",
      started_at_ms: Date.now() - 5_000,
    });
    render(<MenuBarApp />);
    expect(await screen.findByText("Daily")).toBeVisible();
    expect(screen.getByText(/Recording locally · 00:05/)).toBeVisible();
  });

  it("opens and focuses the main window then hides", async () => {
    render(<MenuBarApp />);
    fireEvent.click(await screen.findByRole("button", { name: /open window/i }));
    await vi.waitFor(() => expect(ipc.showMainWindow).toHaveBeenCalledTimes(1));
    expect(ipc.menuBarHide).toHaveBeenCalledTimes(1);
  });
});
