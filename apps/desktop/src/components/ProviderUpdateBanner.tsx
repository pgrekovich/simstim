import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { Download, LoaderCircle } from "lucide-react";
import { Button } from "@/components/ui/button";
import { aiSetupStatus, cancelAiSetup, checkAiUpdates, setupAi, type AiSetupStatus, type ProviderUpdate } from "@/lib/ipc";

export function ProviderUpdateBanner() {
  const [updates, setUpdates] = useState<ProviderUpdate[]>([]);
  const [dismissed, setDismissed] = useState<Record<string, string>>({});
  const [updating, setUpdating] = useState<ProviderUpdate | null>(null);
  const [status, setStatus] = useState<AiSetupStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const cancelled = useRef(false);
  const mounted = useRef(true);
  const running = useRef(false);

  useEffect(() => {
    mounted.current = true;
    let disposed = false;
    let checking = false;
    async function check() {
      if (checking || running.current) return;
      checking = true;
      try {
        const available = await checkAiUpdates();
        if (!disposed) setUpdates(available);
      } catch {
        // Offline checks stay quiet. Recording and existing helpers still work.
      } finally {
        checking = false;
      }
    }
    void check();
    window.addEventListener("focus", check);
    const unlisten = listen<string>("ai-helper-updated", ({ payload }) => {
      if (mounted.current) setUpdates((current) => current.filter((provider) => provider.id !== payload));
    }).catch(() => () => {});
    const unlistenChecks = listen("ai-provider-updates-checked", () => { void check(); })
      .catch(() => () => {});
    return () => {
      mounted.current = false;
      disposed = true;
      window.removeEventListener("focus", check);
      void unlisten.then((stop) => stop());
      void unlistenChecks.then((stop) => stop());
    };
  }, []);

  useEffect(() => {
    if (!updating) return;
    let disposed = false;
    let timer: ReturnType<typeof setTimeout>;
    async function refresh() {
      try {
        const next = await aiSetupStatus();
        if (!disposed) setStatus(next);
      } catch {
        // The setup promise owns completion and error reporting.
      }
      if (!disposed) timer = setTimeout(refresh, 800);
    }
    void refresh();
    return () => { disposed = true; clearTimeout(timer); };
  }, [updating]);

  async function updateProviders() {
    if (running.current) return;
    running.current = true;
    cancelled.current = false;
    setError(null);
    try {
      // Keep one setup active at a time, and retain only failed or cancelled updates for retry.
      for (const provider of updates) {
        if (cancelled.current || !mounted.current) break;
        setUpdating(provider);
        setStatus(null);
        await setupAi(provider.id, true);
        if (mounted.current) setUpdates((current) => current.filter((entry) => entry.id !== provider.id));
      }
    } catch (failure) {
      if (mounted.current) setError(cancelled.current ? "Update cancelled. You can try again." : String(failure));
    } finally {
      running.current = false;
      if (mounted.current) { setUpdating(null); setStatus(null); }
    }
  }

  if (!updating && (updates.length === 0 || updates.every((provider) => dismissed[provider.id] === provider.latest_version))) return null;

  return (
    <aside aria-label="AI provider updates" className="mx-5 mb-3 flex shrink-0 flex-wrap items-center gap-x-3 gap-y-2 rounded-lg border border-primary/20 bg-primary/5 px-3 py-2.5">
      {updating ? <LoaderCircle aria-hidden="true" className="size-4 animate-spin text-muted-foreground" /> : <Download aria-hidden="true" className="size-4 text-muted-foreground" />}
      <div className="min-w-0 flex-1 text-[12px]">
        <p role="status" className="font-medium">
          {updating ? status?.phase === "signing_in" ? `Finish connecting ${updating.name} in your browser.` : `Updating ${updating.name}...` : "AI provider updates available"}
        </p>
        {!updating && <p className="mt-0.5 text-muted-foreground" title={updates.map((provider) => `${provider.name}: ${provider.installed_version} → ${provider.latest_version}`).join("\n")}>
          {updates.map((provider) => provider.name).join(" and ")} · Current model support
        </p>}
        {error && <p role="alert" className="mt-1 text-destructive [overflow-wrap:anywhere]">{error}</p>}
      </div>
      {updating ? <Button variant="ghost" size="xs" onClick={() => {
        cancelled.current = true;
        void cancelAiSetup().catch((failure) => setError(String(failure)));
      }}>Cancel</Button> : <>
        <Button size="xs" onClick={() => void updateProviders()}>Update providers</Button>
        <Button variant="ghost" size="xs" onClick={() => {
          setDismissed(Object.fromEntries(updates.map((provider) => [provider.id, provider.latest_version])));
          setError(null);
        }}>Later</Button>
      </>}
    </aside>
  );
}
