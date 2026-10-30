import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { listen } from "@tauri-apps/api/event";
import { ProviderUpdateBanner } from "./ProviderUpdateBanner";
import * as ipc from "@/lib/ipc";

vi.mock("@/lib/ipc", () => ({
  checkAiUpdates: vi.fn(), setupAi: vi.fn(), aiSetupStatus: vi.fn(), cancelAiSetup: vi.fn(),
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(vi.fn())) }));

const claude = { id: "claude", name: "Claude Code", installed_version: "2.1.236", latest_version: "2.1.314" };
const codex = { id: "codex", name: "Codex", installed_version: "0.152.0", latest_version: "0.166.0" };

beforeEach(() => {
  vi.mocked(ipc.checkAiUpdates).mockReset().mockResolvedValue([claude, codex]);
  vi.mocked(ipc.setupAi).mockReset().mockResolvedValue(undefined);
  vi.mocked(ipc.aiSetupStatus).mockReset().mockResolvedValue(null);
  vi.mocked(ipc.cancelAiSetup).mockReset().mockResolvedValue(undefined);
  vi.mocked(listen).mockClear();
});

it("offers visible updates without installing or changing anything on mount", async () => {
  render(<ProviderUpdateBanner />);
  expect(await screen.findByRole("complementary", { name: "AI provider updates" })).toBeVisible();
  expect(screen.getByRole("button", { name: "Update providers" })).toBeEnabled();
  expect(screen.getByText(/Claude Code and Codex/)).toHaveAttribute("title", expect.stringContaining("0.152.0 → 0.166.0"));
  expect(ipc.setupAi).not.toHaveBeenCalled();
});

it("stays quiet when no update is known or the check fails", async () => {
  vi.mocked(ipc.checkAiUpdates).mockRejectedValue("offline");
  const view = render(<ProviderUpdateBanner />);
  await waitFor(() => expect(ipc.checkAiUpdates).toHaveBeenCalled());
  expect(screen.queryByRole("complementary")).not.toBeInTheDocument();
  view.unmount();
  vi.mocked(ipc.checkAiUpdates).mockResolvedValue([]);
  render(<ProviderUpdateBanner />);
  await waitFor(() => expect(ipc.checkAiUpdates).toHaveBeenCalledTimes(2));
  expect(screen.queryByRole("complementary")).not.toBeInTheDocument();
});

it("updates providers sequentially and disappears only after completion", async () => {
  let finish!: () => void;
  vi.mocked(ipc.setupAi).mockImplementationOnce(() => new Promise<void>((resolve) => { finish = resolve; }));
  render(<ProviderUpdateBanner />);
  fireEvent.click(await screen.findByRole("button", { name: "Update providers" }));
  expect(ipc.setupAi).toHaveBeenCalledExactlyOnceWith("claude", true);
  expect(screen.getByRole("status")).toHaveTextContent("Updating Claude Code");
  await act(async () => { finish(); });
  await waitFor(() => expect(ipc.setupAi).toHaveBeenNthCalledWith(2, "codex", true));
  await waitFor(() => expect(screen.queryByRole("complementary")).not.toBeInTheDocument());
});

it("retains failed updates for retry without repeating completed providers", async () => {
  vi.mocked(ipc.setupAi).mockResolvedValueOnce(undefined).mockRejectedValueOnce("Download failed.");
  render(<ProviderUpdateBanner />);
  fireEvent.click(await screen.findByRole("button", { name: "Update providers" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("Download failed.");
  fireEvent.click(screen.getByRole("button", { name: "Update providers" }));
  await waitFor(() => expect(ipc.setupAi).toHaveBeenCalledTimes(3));
  expect(ipc.setupAi).toHaveBeenNthCalledWith(3, "codex", true);
  await waitFor(() => expect(screen.queryByRole("complementary")).not.toBeInTheDocument());
});

it("cancels setup and never starts the next provider", async () => {
  let reject!: (failure: string) => void;
  vi.mocked(ipc.setupAi).mockImplementationOnce(() => new Promise<void>((_, fail) => { reject = fail; }));
  render(<ProviderUpdateBanner />);
  fireEvent.click(await screen.findByRole("button", { name: "Update providers" }));
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  await waitFor(() => expect(ipc.cancelAiSetup).toHaveBeenCalledOnce());
  await act(async () => { reject("Setup cancelled."); });
  expect(await screen.findByRole("alert")).toHaveTextContent("Update cancelled");
  expect(ipc.setupAi).toHaveBeenCalledTimes(1);
});

it("remembers Later for the same versions and shows a newly released version on focus", async () => {
  render(<ProviderUpdateBanner />);
  fireEvent.click(await screen.findByRole("button", { name: "Later" }));
  await act(async () => { window.dispatchEvent(new Event("focus")); });
  expect(screen.queryByRole("complementary")).not.toBeInTheDocument();
  vi.mocked(ipc.checkAiUpdates).mockResolvedValue([{ ...codex, latest_version: "0.167.0" }]);
  await act(async () => { window.dispatchEvent(new Event("focus")); });
  expect(await screen.findByRole("button", { name: "Update providers" })).toBeVisible();
  expect(ipc.setupAi).not.toHaveBeenCalled();
});

it("removes a provider updated through Settings", async () => {
  vi.mocked(ipc.checkAiUpdates).mockResolvedValue([claude]);
  render(<ProviderUpdateBanner />);
  await screen.findByRole("button", { name: "Update providers" });
  const subscription = vi.mocked(listen).mock.calls.find(([name]) => name === "ai-helper-updated")!;
  const callback = subscription[1] as (event: { payload: string }) => void;
  act(() => { callback({ payload: "claude" }); });
  expect(screen.queryByRole("complementary")).not.toBeInTheDocument();
});

it("refreshes the banner after an explicit version check in Settings", async () => {
  vi.mocked(ipc.checkAiUpdates).mockResolvedValueOnce([]).mockResolvedValue([codex]);
  render(<ProviderUpdateBanner />);
  await waitFor(() => expect(ipc.checkAiUpdates).toHaveBeenCalledOnce());
  const subscription = vi.mocked(listen).mock.calls.find(([name]) => name === "ai-provider-updates-checked")!;
  const callback = subscription[1] as (event: { payload: null }) => void;
  act(() => { callback({ payload: null }); });
  expect(await screen.findByRole("button", { name: "Update providers" })).toBeVisible();
  expect(ipc.setupAi).not.toHaveBeenCalled();
});
