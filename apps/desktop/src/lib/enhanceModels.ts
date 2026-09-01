/**
 * Model choices offered by the cloud summary providers. The backend passes
 * values through verbatim, so this list only controls the convenience menu.
 */
export const ENHANCE_MODEL_OPTIONS: Record<string, { value: string; label: string }[]> = {
  claude: [
    { value: "", label: "Default" },
    { value: "fable", label: "Fable" },
    { value: "opus", label: "Opus" },
    { value: "sonnet", label: "Sonnet" },
    { value: "claude-haiku-4-5", label: "Haiku" },
  ],
  codex: [
    { value: "", label: "Default" },
    { value: "gpt-5.6-sol", label: "GPT-5.6 Sol" },
    { value: "gpt-5.6-terra", label: "GPT-5.6 Terra" },
    { value: "gpt-5.6-luna", label: "GPT-5.6 Luna" },
    { value: "gpt-5.5", label: "GPT-5.5" },
    { value: "gpt-5.3-codex-spark", label: "GPT-5.3 Codex Spark (Pro)" },
  ],
};
