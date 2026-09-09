import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { aiSetupStatus, cancelAiSetup, setupAi, type AiSetupStatus, type EnhanceProviderInfo } from "@/lib/ipc";

const ASSISTANTS = [{ id: "claude", label: "Claude", helper: "Claude Code" }, { id: "codex", label: "ChatGPT", helper: "Codex" }];

/** A helper-install action, not an account screen. The vendor owns sign-in. */
export function AiSetup({ providers, onConnected }: {
  providers: EnhanceProviderInfo[];
  onConnected: (provider: string) => Promise<void>;
}) {
  const [status, setStatus] = useState<AiSetupStatus | null>(null);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const callback = useRef(onConnected);
  callback.current = onConnected;
  const previous = useRef<string | null>(null);
  const ownRequest = useRef(false);

  useEffect(() => {
    let disposed = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    async function refresh() {
      try {
        const next = await aiSetupStatus();
        if (disposed) return;
        if (next) {
          previous.current = next.provider;
          setStatus(next);
        } else if (!ownRequest.current) {
          setStatus(null);
          if (previous.current) {
            const provider = previous.current;
            previous.current = null;
            // Reopening Settings resumes the view of an existing setup.
            // The parent rechecks readiness before selecting the provider.
            await callback.current(provider);
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

  async function connect(provider: string) {
    setError(null);
    ownRequest.current = true;
    setPending(true);
    setStatus({ provider, phase: "installing" });
    try {
      await setupAi(provider);
      await callback.current(provider);
    } catch (err) {
      setError(String(err));
    } finally {
      ownRequest.current = false;
      previous.current = null;
      setPending(false);
      setStatus(null);
    }
  }

  const missing = ASSISTANTS.filter((a) => !providers.find((p) => p.id === a.id)?.available);
  if (missing.length === 0 && !status && !error) return null;
  const busy = pending || status !== null;
  const label = ASSISTANTS.find((a) => a.id === status?.provider)?.label ?? "your assistant";
  return (
    <div className="flex flex-col gap-2 py-2">
      {!busy && <>
        <p className="text-[11.5px] leading-[1.5] text-muted-foreground">
          We'll set up the official helper and open its browser sign-in. No Simstim account.
          Summaries and questions send text, never audio. Your provider's limits apply.
          You can skip this.
        </p>
        <div className="flex flex-wrap gap-2">
          {missing.map((a) => <Button key={a.id} variant="outline" size="sm"
            title={`Uses ${a.helper}`} onClick={() => void connect(a.id)}>
            Connect {a.label}
          </Button>)}
        </div>
      </>}
      {busy && <div className="flex items-center justify-between gap-3">
        <p role="status" className="text-[12px] text-muted-foreground">
          {status?.phase === "signing_in" ? `Finish connecting ${label} in your browser.` : `Preparing ${label}...`}
        </p>
        <Button variant="ghost" size="sm" onClick={() => {
          void cancelAiSetup().catch(() => setError("Could not cancel setup. Please try again."));
        }}>Cancel</Button>
      </div>}
      {error && <p role="alert" className="text-[12px] text-destructive">{error}</p>}
    </div>
  );
}
