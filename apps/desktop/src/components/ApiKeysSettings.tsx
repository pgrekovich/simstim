import { useCallback, useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ApiModelPicker } from "@/components/ApiModelPicker";
import { deleteApiKey, listApiModels, setApiKey, setEnhanceModel, setEnhanceProvider, type ApiModel, type AppConfig, type EnhanceProviderInfo } from "@/lib/ipc";

const API_PROVIDERS = [
  { id: "openai", name: "OpenAI" },
  { id: "anthropic", name: "Anthropic" },
  { id: "openrouter", name: "OpenRouter" },
];

export const isApiProvider = (id: string) => API_PROVIDERS.some((p) => p.id === id);

export function ApiKeysSettings({ config, providers, onChanged }: {
  config: Pick<AppConfig, "enhance_models" | "enhance_provider"> | null;
  providers: EnhanceProviderInfo[];
  onChanged: () => Promise<void>;
}) {
  const initialProvider = config && isApiProvider(config.enhance_provider) ? config.enhance_provider : "openai";
  const [provider, setProvider] = useState(initialProvider);
  const [key, setKey] = useState("");
  const [model, setModel] = useState(config?.enhance_models[initialProvider] ?? "");
  const [models, setModels] = useState<ApiModel[] | null>(null);
  const [expanded, setExpanded] = useState(false);
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [catalogError, setCatalogError] = useState<string | null>(null);
  const request = useRef(0);
  const configured = providers.some((p) => p.id === provider && p.available);
  const name = API_PROVIDERS.find((p) => p.id === provider)?.name;
  const savedModel = config?.enhance_models[provider] ?? "";

  const loadModels = useCallback(async () => {
    const run = ++request.current;
    setLoading(true);
    setCatalogError(null);
    try {
      const catalog = await listApiModels(provider, key.trim() || undefined);
      if (request.current === run) setModels(catalog);
    } catch (err) {
      if (request.current === run) setCatalogError(String(err));
    } finally {
      if (request.current === run) setLoading(false);
    }
  }, [provider, key]);

  useEffect(() => {
    if (expanded && configured && !key.trim()) void loadModels();
    return () => { ++request.current; };
  }, [expanded, configured, key, loadModels]);

  function invalidateCatalog() {
    ++request.current;
    setLoading(false);
    setModels(null);
    setCatalogError(null);
  }

  async function save() {
    setBusy(true);
    setError(null);
    try {
      if (key.trim()) {
        await setApiKey(provider, key.trim());
        setKey("");
      }
      await setEnhanceModel(provider, model);
      await setEnhanceProvider(provider);
      await onChanged();
    } catch (err) { setError(String(err)); }
    finally { setBusy(false); }
  }

  async function remove() {
    setBusy(true);
    setError(null);
    invalidateCatalog();
    try {
      await deleteApiKey(provider);
      setKey("");
      await onChanged();
    } catch (err) { setError(String(err)); }
    finally { setBusy(false); }
  }

  const canSave = !!model && (configured || !!key.trim()) && (!key.trim() || models?.some((m) => m.value === model));

  return <details className="border-y border-border py-3" onToggle={(event) => {
    setExpanded(event.currentTarget.open);
    if (!event.currentTarget.open) { setKey(""); invalidateCatalog(); }
  }}>
    <summary className="cursor-pointer text-xs">Use your own API key</summary>
    <div className="flex flex-col gap-2 pt-3">
      <p className="text-[11.5px] leading-[1.55] text-muted-foreground">
        Keys stay in macOS Keychain. Text goes directly to your chosen provider, never audio.
        API usage is billed separately from chat subscriptions. OpenRouter routes text to the model's provider.
        API questions can read your Markdown notes; connectors require Claude Code.
      </p>
      <label className="text-xs" htmlFor="api-provider">API provider</label>
      <select id="api-provider" value={provider} disabled={busy} className="h-8 rounded-md border border-input bg-transparent px-2 text-xs"
        onChange={(event) => {
          invalidateCatalog();
          setProvider(event.target.value);
          setKey("");
          setModel(config?.enhance_models[event.target.value] ?? "");
          setError(null);
        }}>
        {API_PROVIDERS.map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
      </select>
      <label className="text-xs" htmlFor="api-key">{name} API key{configured ? " (saved in Keychain)" : ""}</label>
      <Input id="api-key" type="password" autoComplete="off" spellCheck={false} value={key} disabled={busy}
        placeholder={configured ? "Leave blank to keep the saved key" : "Paste your API key"}
        onChange={(event) => { invalidateCatalog(); setKey(event.target.value); setModel(savedModel); }} />
      <div className="flex items-center justify-between">
        <label className="text-xs" htmlFor="api-model">Model</label>
        <Button variant="ghost" size="sm" className="h-7 text-xs" disabled={busy || loading || (!configured && !key.trim())}
          onClick={() => void loadModels()}>{loading ? "Loading..." : models ? "Refresh models" : "Load models"}</Button>
      </div>
      <ApiModelPicker key={provider} models={models} value={model} loading={loading} disabled={busy} onChange={setModel} />
      {catalogError && <p role="alert" className="text-xs text-destructive">{catalogError}</p>}
      {models?.length === 0 && <p role="status" className="text-xs text-muted-foreground">No compatible text models are available for this key.</p>}
      {models && model && !models.some((m) => m.value === model) && <p className="text-[11px] text-muted-foreground">Your saved model is not in this catalog. Choose another model or keep the saved selection.</p>}
      <p className="text-[11px] text-muted-foreground">Models come from your provider. Loading the list sends no notes and does not start a model request.</p>
      <div className="flex gap-2">
        <Button variant="outline" size="sm" disabled={busy || loading || !config || !canSave} onClick={() => void save()}>
          {busy ? "Saving..." : "Save and use API"}
        </Button>
        {configured && <Button variant="ghost" size="sm" disabled={busy} onClick={() => void remove()}>Remove key</Button>}
      </div>
      {error && <p role="alert" className="text-xs text-destructive">{error}</p>}
    </div>
  </details>;
}
