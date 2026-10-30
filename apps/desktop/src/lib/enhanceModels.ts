import type { EnhanceProviderInfo } from "./ipc";

interface ModelOption { value: string; label: string; }

/** Official helpers supply their model catalogs; never guess supported IDs. */
export function enhanceModelOptions(provider: EnhanceProviderInfo, selected = ""): ModelOption[] {
  const catalog = provider.models ?? [];
  const options: ModelOption[] = [{ value: "", label: "Default" }, ...catalog];
  if (selected && !options.some((option) => option.value === selected)) {
    // Preserve explicit choices, but make stale or unavailable catalog entries visible.
    options.push({ value: selected, label: `${selected} (saved; not listed)` });
  }
  return options;
}
