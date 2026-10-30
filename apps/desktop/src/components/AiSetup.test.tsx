import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { AiSetup } from "./AiSetup";
import * as ipc from "@/lib/ipc";
vi.mock("@/lib/ipc", () => ({
  aiSetupStatus: vi.fn(), setupAi: vi.fn(), cancelAiSetup: vi.fn(), getAiProviderStatus: vi.fn(),
}));
beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(ipc.aiSetupStatus).mockResolvedValue(null);
  vi.mocked(ipc.cancelAiSetup).mockResolvedValue(undefined);
  vi.mocked(ipc.getAiProviderStatus).mockResolvedValue([
    { id: "claude", name: "Claude Code", installed_version: "2.1.236", latest_version: "2.1.314", update_available: true },
    { id: "codex", name: "Codex", installed_version: "0.152.0", latest_version: "0.166.0", update_available: true },
  ]);
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
  expect(ipc.setupAi).toHaveBeenCalledWith("codex", false);
  expect(screen.getByRole("status")).toHaveTextContent("Preparing ChatGPT");
  expect(connected).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(ipc.cancelAiSetup).toHaveBeenCalledOnce();
  await act(async () => { finish(); });
  expect(connected).toHaveBeenCalledWith("codex", false);
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

it.each([
  { id: "codex", name: "Codex" },
  { id: "claude", name: "Claude Code" },
])("updates $name only after an explicit click and keeps it an update", async ({ id, name }) => {
  const connected = vi.fn().mockResolvedValue(undefined);
  vi.mocked(ipc.setupAi).mockResolvedValue(undefined);
  render(<AiSetup allowUpdates providers={[{ id, name, available: true, version: "test-version" }]} onConnected={connected} />);
  await waitFor(() => expect(ipc.aiSetupStatus).toHaveBeenCalled());
  expect(ipc.setupAi).not.toHaveBeenCalled();
  fireEvent.click(await screen.findByRole("button", { name: `Update ${name}` }));
  await waitFor(() => expect(ipc.setupAi).toHaveBeenCalledExactlyOnceWith(id, true));
  await waitFor(() => expect(connected).toHaveBeenCalledWith(id, true));
});
it("keeps the update action available when a download fails", async () => {
  vi.mocked(ipc.setupAi).mockRejectedValue("Download failed.");
  const connected = vi.fn();
  render(<AiSetup allowUpdates providers={[{ id: "codex", name: "Codex", available: true, version: "0.152.0" }]} onConnected={connected} />);
  fireEvent.click(await screen.findByRole("button", { name: "Update Codex" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("Download failed.");
  expect(screen.getByRole("button", { name: "Update Codex" })).toBeEnabled();
  expect(connected).not.toHaveBeenCalled();
});

it("shows current providers as Up to date without offering an update", async () => {
  vi.mocked(ipc.getAiProviderStatus).mockResolvedValue([
    { id: "claude", name: "Claude Code", installed_version: "2.1.314", latest_version: "2.1.314", update_available: false },
    { id: "codex", name: "Codex", installed_version: "0.166.0", latest_version: "0.166.0", update_available: false },
  ]);
  render(<AiSetup allowUpdates providers={[
    { id: "claude", name: "Claude Code", available: true, version: "2.1.314" },
    { id: "codex", name: "Codex", available: true, version: "0.166.0" },
  ]} onConnected={vi.fn()} />);
  expect(await screen.findAllByText("Up to date")).toHaveLength(2);
  expect(screen.queryByRole("button", { name: "Update Claude Code" })).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Update Codex" })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));
  await waitFor(() => expect(ipc.getAiProviderStatus).toHaveBeenLastCalledWith(true));
  expect(ipc.setupAi).not.toHaveBeenCalled();
});

it("does not call an unavailable version check Up to date", async () => {
  vi.mocked(ipc.getAiProviderStatus).mockResolvedValue([
    { id: "claude", name: "Claude Code", installed_version: "2.1.314", latest_version: null, update_available: false },
  ]);
  render(<AiSetup allowUpdates providers={[
    { id: "claude", name: "Claude Code", available: true, version: "2.1.314" },
  ]} onConnected={vi.fn()} />);
  expect(await screen.findByText("Update check unavailable")).toBeVisible();
  expect(screen.queryByText("Up to date")).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Update Claude Code" })).not.toBeInTheDocument();
});

it("offers an update only for the provider confirmed to be behind", async () => {
  vi.mocked(ipc.getAiProviderStatus).mockResolvedValue([
    { id: "claude", name: "Claude Code", installed_version: "2.1.236", latest_version: "2.1.314", update_available: true },
    { id: "codex", name: "Codex", installed_version: "0.166.0", latest_version: "0.166.0", update_available: false },
  ]);
  render(<AiSetup allowUpdates providers={[
    { id: "claude", name: "Claude Code", available: true, version: "2.1.236" },
    { id: "codex", name: "Codex", available: true, version: "0.166.0" },
  ]} onConnected={vi.fn()} />);
  expect(await screen.findByRole("button", { name: "Update Claude Code" })).toBeEnabled();
  expect(screen.queryByRole("button", { name: "Update Codex" })).not.toBeInTheDocument();
  expect(screen.getByText("Up to date")).toBeVisible();
});

it("rechecks versions if installation succeeds but provider sign-in fails", async () => {
  vi.mocked(ipc.getAiProviderStatus)
    .mockResolvedValueOnce([{ id: "codex", name: "Codex", installed_version: "0.152.0", latest_version: "0.166.0", update_available: true }])
    .mockResolvedValue([{ id: "codex", name: "Codex", installed_version: "0.166.0", latest_version: "0.166.0", update_available: false }]);
  vi.mocked(ipc.setupAi).mockRejectedValue("Sign-in did not finish.");
  render(<AiSetup allowUpdates providers={[
    { id: "codex", name: "Codex", available: true, version: "0.152.0" },
  ]} onConnected={vi.fn()} />);
  fireEvent.click(await screen.findByRole("button", { name: "Update Codex" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("Sign-in did not finish.");
  expect(await screen.findByText("Up to date")).toBeVisible();
  expect(screen.queryByRole("button", { name: "Update Codex" })).not.toBeInTheDocument();
});
