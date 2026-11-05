import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { listen } from "@tauri-apps/api/event";
import { AppUpdateBanner, describeUpdateProgress } from "./AppUpdateBanner";
import * as ipc from "@/lib/ipc";

vi.mock("@/lib/ipc", () => ({ checkAppUpdate: vi.fn(), installAppUpdate: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(vi.fn())) }));

const available = { current_version: "0.12.0", update: { current_version: "0.12.0", latest_version: "0.13.0", notes: null, published: "2026-11-05" } };
const upToDate = { current_version: "0.12.0", update: null };

/** The handler the component registered for `name`, so a test can push an event through it. */
function listener(name: string): (event: { payload: unknown }) => void {
  const call = vi.mocked(listen).mock.calls.find(([event]) => event === name);
  if (!call) throw new Error(`no listener for ${name}`);
  return call[1] as (event: { payload: unknown }) => void;
}

beforeEach(() => {
  vi.mocked(ipc.checkAppUpdate).mockReset().mockResolvedValue(available);
  vi.mocked(ipc.installAppUpdate).mockReset().mockImplementation(() => new Promise(() => {}));
  vi.mocked(listen).mockClear();
});

it("offers the newer version without installing anything on mount", async () => {
  render(<AppUpdateBanner />);
  expect(await screen.findByRole("complementary", { name: "Simstim update" })).toBeVisible();
  expect(screen.getByRole("status")).toHaveTextContent("Simstim 0.13.0 is available");
  expect(screen.getByText(/You have 0.12.0/)).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Update and restart" })).toBeEnabled();
  expect(ipc.installAppUpdate).not.toHaveBeenCalled();
});

it("stays quiet when the app is current or the check fails", async () => {
  vi.mocked(ipc.checkAppUpdate).mockRejectedValue("offline");
  const view = render(<AppUpdateBanner />);
  await waitFor(() => expect(ipc.checkAppUpdate).toHaveBeenCalled());
  expect(screen.queryByRole("complementary")).not.toBeInTheDocument();
  view.unmount();
  vi.mocked(ipc.checkAppUpdate).mockResolvedValue(upToDate);
  render(<AppUpdateBanner />);
  await waitFor(() => expect(ipc.checkAppUpdate).toHaveBeenCalledTimes(2));
  expect(screen.queryByRole("complementary")).not.toBeInTheDocument();
});

it("installs on click and follows download progress until the restart", async () => {
  render(<AppUpdateBanner />);
  fireEvent.click(await screen.findByRole("button", { name: "Update and restart" }));
  expect(ipc.installAppUpdate).toHaveBeenCalledOnce();
  expect(screen.getByRole("status")).toHaveTextContent("Downloading...");
  expect(screen.queryByRole("button", { name: "Update and restart" })).not.toBeInTheDocument();

  act(() => listener("app-update-progress")({ payload: { phase: "downloading", received: 43, total: 100, message: null } }));
  expect(screen.getByRole("status")).toHaveTextContent("Downloading 43%");
  act(() => listener("app-update-progress")({ payload: { phase: "installing", received: 100, total: 100, message: null } }));
  expect(screen.getByRole("status")).toHaveTextContent("Installing...");
  act(() => listener("app-update-progress")({ payload: { phase: "restarting", received: 0, total: null, message: null } }));
  expect(screen.getByRole("status")).toHaveTextContent("Restarting...");
});

it("keeps the offer with the error when installing fails, so it can be retried", async () => {
  vi.mocked(ipc.installAppUpdate).mockRejectedValue("could not install the update: no space left");
  render(<AppUpdateBanner />);
  fireEvent.click(await screen.findByRole("button", { name: "Update and restart" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("no space left");
  expect(screen.getByRole("button", { name: "Update and restart" })).toBeEnabled();
});

it("hides a dismissed version and re-checks when Settings asks explicitly", async () => {
  render(<AppUpdateBanner />);
  fireEvent.click(await screen.findByRole("button", { name: "Later" }));
  expect(screen.queryByRole("complementary")).not.toBeInTheDocument();

  const newer = { current_version: "0.12.0", update: { ...available.update, latest_version: "0.14.0" } };
  vi.mocked(ipc.checkAppUpdate).mockResolvedValue(newer);
  await act(async () => listener("app-update-checked")({ payload: null }));
  expect(await screen.findByRole("status")).toHaveTextContent("Simstim 0.14.0 is available");
});

describe("describeUpdateProgress", () => {
  it("rounds the download percent and names the other phases", () => {
    expect(describeUpdateProgress({ phase: "downloading", received: 1, total: 3, message: null })).toBe("Downloading 33%");
    expect(describeUpdateProgress({ phase: "downloading", received: 1, total: null, message: null })).toBe("Downloading...");
    expect(describeUpdateProgress({ phase: "installing", received: 0, total: null, message: null })).toBe("Installing...");
    expect(describeUpdateProgress({ phase: "error", received: 0, total: null, message: "boom" })).toBe("boom");
  });
});
