import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ApiKeysSettings } from "./ApiKeysSettings";
import * as ipc from "@/lib/ipc";

vi.mock("@/lib/ipc", () => ({
  setApiKey: vi.fn().mockResolvedValue(undefined),
  deleteApiKey: vi.fn().mockResolvedValue(undefined),
  setEnhanceModel: vi.fn().mockResolvedValue(undefined),
  setEnhanceProvider: vi.fn().mockResolvedValue(undefined),
  listApiModels: vi.fn(),
}));

const config = { enhance_models: { openai: "saved-model" }, enhance_provider: "claude" };
const provider = { id: "openai", name: "OpenAI API", available: true, version: null };
const catalog = [{ value: "saved-model", label: "Saved model" }, { value: "my-model", label: "My model" }];

function open() {
  const details = screen.getByText("Use your own API key").closest("details")!;
  details.open = true;
  fireEvent(details, new Event("toggle"));
}

async function pickModel(name: string) {
  fireEvent.click(screen.getByRole("combobox", { name: "Model" }));
  fireEvent.click(await screen.findByRole("option", { name }));
}

describe("API key settings", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(ipc.listApiModels).mockResolvedValue(catalog);
  });

  it("loads models with a new key without saving it, then saves the selected model", async () => {
    const changed = vi.fn().mockResolvedValue(undefined);
    render(<ApiKeysSettings config={config} providers={[]} onChanged={changed} />);
    open();
    const key = screen.getByLabelText("OpenAI API key");
    expect(key).toHaveAttribute("type", "password");
    fireEvent.change(key, { target: { value: " synthetic-secret " } });
    expect(screen.getByRole("button", { name: "Save and use API" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Load models" }));
    await screen.findByRole("button", { name: "Refresh models" });
    expect(ipc.listApiModels).toHaveBeenCalledWith("openai", "synthetic-secret");
    expect(ipc.setApiKey).not.toHaveBeenCalled();
    await pickModel("My model my-model");
    fireEvent.click(screen.getByRole("button", { name: "Save and use API" }));
    await waitFor(() => expect(changed).toHaveBeenCalledOnce());
    expect(ipc.setApiKey).toHaveBeenCalledWith("openai", "synthetic-secret");
    expect(ipc.setEnhanceModel).toHaveBeenCalledWith("openai", "my-model");
    expect(ipc.setEnhanceProvider).toHaveBeenCalledWith("openai");
    expect(key).toHaveValue("");
  });

  it("loads a saved key's models only when opened, searches names and supports keyboard selection", async () => {
    render(<ApiKeysSettings config={config} providers={[provider]} onChanged={vi.fn()} />);
    expect(ipc.listApiModels).not.toHaveBeenCalled();
    open();
    await screen.findByRole("button", { name: "Refresh models" });
    expect(ipc.listApiModels).toHaveBeenCalledWith("openai", undefined);
    fireEvent.click(screen.getByRole("combobox", { name: "Model" }));
    const search = await screen.findByRole("combobox", { name: "Search models" });
    fireEvent.change(search, { target: { value: "MY MODEL" } });
    expect(within(screen.getByRole("listbox", { name: "Models" })).getAllByRole("option")).toHaveLength(1);
    fireEvent.keyDown(search, { key: "Enter" });
    expect(screen.getByRole("combobox", { name: "Model" })).toHaveTextContent("My model");
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
  });

  it("keeps a saved key when changing models and can remove it", async () => {
    const changed = vi.fn().mockResolvedValue(undefined);
    render(<ApiKeysSettings config={config} providers={[provider]} onChanged={changed} />);
    open();
    await screen.findByRole("button", { name: "Refresh models" });
    fireEvent.click(screen.getByRole("button", { name: "Save and use API" }));
    await waitFor(() => expect(changed).toHaveBeenCalledOnce());
    expect(ipc.setApiKey).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Remove key" }));
    await waitFor(() => expect(ipc.deleteApiKey).toHaveBeenCalledWith("openai"));
  });

  it("clears unsaved keys and ignores stale catalogs when switching providers", async () => {
    let resolve!: (models: ipc.ApiModel[]) => void;
    vi.mocked(ipc.listApiModels).mockReturnValueOnce(new Promise((done) => { resolve = done; }));
    render(<ApiKeysSettings config={config} providers={[]} onChanged={vi.fn()} />);
    open();
    fireEvent.change(screen.getByLabelText("OpenAI API key"), { target: { value: "secret" } });
    fireEvent.click(screen.getByRole("button", { name: "Load models" }));
    fireEvent.change(screen.getByLabelText("API provider"), { target: { value: "openrouter" } });
    resolve(catalog);
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Model" })).toBeDisabled());
    expect(screen.getByLabelText("OpenRouter API key")).toHaveValue("");
    expect(screen.getByRole("button", { name: "Save and use API" })).toBeDisabled();
  });

  it("reports catalog failures and lets the user retry without saving a bad key", async () => {
    vi.mocked(ipc.listApiModels).mockRejectedValueOnce(new Error("API access denied"));
    render(<ApiKeysSettings config={config} providers={[]} onChanged={vi.fn()} />);
    open();
    fireEvent.change(screen.getByLabelText("OpenAI API key"), { target: { value: "secret" } });
    fireEvent.click(screen.getByRole("button", { name: "Load models" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("API access denied");
    expect(ipc.setApiKey).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Save and use API" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Load models" }));
    await screen.findByRole("button", { name: "Refresh models" });
  });

  it("preserves an unlisted saved model on a failed refresh", async () => {
    vi.mocked(ipc.listApiModels).mockRejectedValueOnce(new Error("Offline"));
    const changed = vi.fn().mockResolvedValue(undefined);
    render(<ApiKeysSettings config={config} providers={[provider]} onChanged={changed} />);
    open();
    expect(await screen.findByRole("alert")).toHaveTextContent("Offline");
    expect(screen.getByRole("combobox", { name: "Model" })).toHaveTextContent("saved-model (saved)");
    fireEvent.click(screen.getByRole("button", { name: "Save and use API" }));
    await waitFor(() => expect(changed).toHaveBeenCalledOnce());
    expect(ipc.setEnhanceModel).toHaveBeenCalledWith("openai", "saved-model");
  });

  it("shows empty catalogs without offering a text field for model IDs", async () => {
    vi.mocked(ipc.listApiModels).mockResolvedValueOnce([]);
    render(<ApiKeysSettings config={{ ...config, enhance_models: {} }} providers={[provider]} onChanged={vi.fn()} />);
    open();
    expect(await screen.findByRole("status")).toHaveTextContent("No compatible text models");
    expect(screen.getByRole("combobox", { name: "Model" })).toBeDisabled();
    expect(screen.queryByRole("textbox", { name: "Model ID" })).not.toBeInTheDocument();
  });
});
