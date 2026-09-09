import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { AiSetup } from "./AiSetup";
import * as ipc from "@/lib/ipc";
vi.mock("@/lib/ipc", () => ({
  aiSetupStatus: vi.fn(), setupAi: vi.fn(), cancelAiSetup: vi.fn(),
}));
beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(ipc.aiSetupStatus).mockResolvedValue(null);
  vi.mocked(ipc.cancelAiSetup).mockResolvedValue(undefined);
});
it("does not install anything until the user chooses an assistant", async () => {
  render(<AiSetup providers={[]} onConnected={vi.fn()} />);
  expect(screen.getByRole("button", { name: "Connect Claude" })).toBeVisible();
  expect(screen.getByRole("button", { name: "Connect ChatGPT" })).toBeVisible();
  await waitFor(() => expect(ipc.aiSetupStatus).toHaveBeenCalled());
  expect(ipc.setupAi).not.toHaveBeenCalled();
});
it("waits for official setup completion before selecting the assistant", async () => {
  let finish!: () => void;
  vi.mocked(ipc.setupAi).mockImplementation(() => new Promise<void>((resolve) => { finish = resolve; }));
  const connected = vi.fn().mockResolvedValue(undefined);
  render(<AiSetup providers={[]} onConnected={connected} />);
  fireEvent.click(screen.getByRole("button", { name: "Connect ChatGPT" }));
  expect(ipc.setupAi).toHaveBeenCalledWith("codex");
  expect(screen.getByRole("status")).toHaveTextContent("Preparing ChatGPT");
  expect(connected).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(ipc.cancelAiSetup).toHaveBeenCalledOnce();
  await act(async () => { finish(); });
  expect(connected).toHaveBeenCalledWith("codex");
});
it("offers retry after a failed setup without selecting a provider", async () => {
  vi.mocked(ipc.setupAi).mockRejectedValue("Download failed.");
  const connected = vi.fn();
  render(<AiSetup providers={[]} onConnected={connected} />);
  fireEvent.click(screen.getByRole("button", { name: "Connect Claude" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("Download failed.");
  expect(screen.getByRole("button", { name: "Connect Claude" })).toBeEnabled();
  expect(connected).not.toHaveBeenCalled();
});
it("resumes an existing browser sign-in when settings reopen", async () => {
  vi.mocked(ipc.aiSetupStatus).mockResolvedValue({ provider: "claude", phase: "signing_in" });
  render(<AiSetup providers={[]} onConnected={vi.fn()} />);
  expect(await screen.findByRole("status")).toHaveTextContent("Finish connecting Claude in your browser.");
  expect(ipc.setupAi).not.toHaveBeenCalled();
});
it("offers connection for an installed but unauthenticated helper", () => {
  render(<AiSetup providers={[{ id: "claude", name: "Claude Code", installed: true, available: false, version: "1" }]} onConnected={vi.fn()} />);
  expect(screen.getByRole("button", { name: "Connect Claude" })).toBeVisible();
});
