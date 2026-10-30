import { useCallback, useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { aiSetupStatus, cancelAiSetup, getAiProviderStatus, setupAi, type AiSetupStatus, type EnhanceProviderInfo, type ProviderStatus } from "@/lib/ipc";

const ASSISTANTS = [{ id: "claude", label: "Claude", helper: "Claude Code" }, { id: "codex", label: "ChatGPT", helper: "Codex" }];

/** A helper-install action, not an account screen. The vendor owns sign-in. */
export function AiSetup({ providers, onConnected, allowUpdates = false }: {
  providers: EnhanceProviderInfo[];
  onConnected: (provider: string, updating?: boolean) => Promise<void>;
  allowUpdates?: boolean;
}) {
  const [status, setStatus] = useState<AiSetupStatus | null>(null);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [versions, setVersions] = useState<ProviderStatus[]>([]);
  const [checkingUpdates, setCheckingUpdates] = useState(false);
  const [checkFailed, setCheckFailed] = useState(false);
  const updateCheckRun = useRef(0);
  const callback = useRef(onConnected);
  callback.current = onConnected;
  const previous = useRef<AiSetupStatus | null>(null);
  const ownRequest = useRef(false);

  const refreshUpdates = useCallback(async (force: boolean) => {
    const run = ++updateCheckRun.current;
    setCheckingUpdates(true);
    setCheckFailed(false);
    try {
      const next = await getAiProviderStatus(force);
      if (updateCheckRun.current === run) setVersions(next);
    } catch {
      if (updateCheckRun.current === run) setCheckFailed(true);
    } finally {
      if (updateCheckRun.current === run) setCheckingUpdates(false);
    }
  }, []);

  const connectedVersions = providers.filter((provider) => provider.available)
    .map((provider) => `${provider.id}:${provider.version}`).join(",");
  useEffect(() => {
    if (!allowUpdates || !connectedVersions) return;
    void refreshUpdates(false);
    return () => { ++updateCheckRun.current; };
  }, [allowUpdates, connectedVersions, refreshUpdates]);

  useEffect(() => {
    let disposed = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    async function refresh() {
      try {
        const next = await aiSetupStatus();
        if (disposed) return;
        if (next) {
          previous.current = next;
          setStatus(next);
        } else if (!ownRequest.current) {
          setStatus(null);
          if (previous.current) {
            const completed = previous.current;
            previous.current = null;
            // Reopening Settings resumes the view of an existing setup.
            // The parent rechecks readiness before selecting the provider.
            await callback.current(completed.provider, completed.updating);
          }
        }
        if (!disposed && (next || ownRequest.current)) timer = setTimeout(refresh, 800);
      } catch {
        if (!disposed) setError("Could not check AI setup. Reopen Settings to try again.");
      }
    }
    void refresh();
    return () => { disposed = true; if (timer) clearTimeout(timer); };
  }, [pending]);

  async function connect(provider: string, updating = false) {
    setError(null);
    ownRequest.current = true;
    setPending(true);
    setStatus({ provider, phase: "installing", updating });
    try {
      await setupAi(provider, updating);
      await callback.current(provider, updating);
    } catch (err) {
      setError(String(err));
    } finally {
      ownRequest.current = false;
      previous.current = null;
      setPending(false);
      setStatus(null);
      if (allowUpdates) await refreshUpdates(false);
    }
  }

  const missing = ASSISTANTS.filter((a) => !providers.find((p) => p.id === a.id)?.available);
  const connected = allowUpdates ? ASSISTANTS.filter((a) => providers.find((p) => p.id === a.id)?.available) : [];
  if (missing.length === 0 && connected.length === 0 && !status && !error) return null;
  const busy = pending || status !== null;
  const label = ASSISTANTS.find((a) => a.id === status?.provider)?.label ?? "your assistant";
  return (
    <div className="flex flex-col gap-2 py-2">
      {!busy && <>
        {missing.length > 0 && <p className="text-[11.5px] leading-[1.5] text-muted-foreground">
          We'll set up the official helper and open its browser sign-in. No Simstim account.
          Summaries and questions send text, never audio. Your provider's limits apply.
          You can skip this.
        </p>}
        <div className="flex flex-wrap gap-2">
          {missing.map((a) => <Button key={a.id} variant="outline" size="sm"
            title={`Uses ${a.helper}`} onClick={() => void connect(a.id)}>
            Connect {a.label}
          </Button>)}
        </div>
        {connected.length > 0 && <div className="flex flex-col gap-2">
          {connected.map((assistant) => {
            const version = versions.find((provider) => provider.id === assistant.id);
            const known = !checkFailed && version?.installed_version && version.latest_version;
            return <div key={assistant.id} className="flex items-center justify-between gap-3 text-[12px]">
              <span>{assistant.helper}</span>
              {known && version.update_available ? <Button variant="outline" size="sm"
                title={`${version.installed_version} → ${version.latest_version}`}
                disabled={checkingUpdates} onClick={() => void connect(assistant.id, true)}>
                Update {assistant.helper}
              </Button> : <span className="text-muted-foreground" title={known ? `Installed ${version.installed_version}; latest ${version.latest_version}` : undefined}>
                {checkingUpdates ? "Checking..." : known ? "Up to date" : "Update check unavailable"}
              </span>}
            </div>;
          })}
          <Button variant="ghost" size="sm" className="self-start" disabled={checkingUpdates}
            onClick={() => void refreshUpdates(true)}>
            Check for updates
          </Button>
        </div>}
      </>}
      {busy && <div className="flex items-center justify-between gap-3">
        <p role="status" className="text-[12px] text-muted-foreground">
          {status?.phase === "signing_in" ? `Finish connecting ${label} in your browser.` : status?.updating ? `Updating ${ASSISTANTS.find((a) => a.id === status.provider)?.helper ?? label}...` : `Preparing ${label}...`}
        </p>
        <Button variant="ghost" size="sm" onClick={() => {
          void cancelAiSetup().catch(() => setError("Could not cancel setup. Please try again."));
        }}>Cancel</Button>
      </div>}
      {error && <p role="alert" className="text-[12px] text-destructive">{error}</p>}
    </div>
  );
}
