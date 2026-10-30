import { useEffect, useId, useRef, useState } from "react";
import { Popover } from "radix-ui";
import { Check, ChevronDown, Search } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";
import type { ApiModel } from "@/lib/ipc";

export function ApiModelPicker({ models, value, disabled, loading, onChange }: {
  models: ApiModel[] | null;
  value: string;
  disabled: boolean;
  loading: boolean;
  onChange: (model: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const listId = useId();
  const selected = models?.find((model) => model.value === value);
  const options = models ?? [];
  const filtered = options.filter((model) => `${model.label} ${model.value}`.toLowerCase().includes(query.trim().toLowerCase()));
  const search = useRef<HTMLInputElement>(null);
  const list = useRef<HTMLDivElement>(null);

  useEffect(() => {
    list.current?.querySelector(`[data-index="${active}"]`)?.scrollIntoView?.({ block: "nearest" });
  }, [active]);

  function choose(model: string) {
    onChange(model);
    setOpen(false);
  }

  return <Popover.Root open={open} onOpenChange={(next) => {
    setOpen(next);
    setQuery("");
    setActive(Math.max(0, options.findIndex((model) => model.value === value)));
  }}>
    <Popover.Trigger asChild>
      <Button id="api-model" role="combobox" aria-label="Model" aria-expanded={open} aria-controls={listId}
        variant="outline" disabled={disabled || loading || !options.length}
        className="h-9 w-full justify-between text-xs font-normal">
        <span className="truncate">{loading ? "Loading models..." : selected?.label || (value ? `${value} (saved)` : "Choose a model")}</span>
        <ChevronDown className="size-3.5 shrink-0 text-muted-foreground" />
      </Button>
    </Popover.Trigger>
    <Popover.Portal>
      <Popover.Content align="start" sideOffset={4}
        className="z-50 w-(--radix-popover-trigger-width) rounded-md border bg-popover text-popover-foreground shadow-md"
        onOpenAutoFocus={(event) => { event.preventDefault(); search.current?.focus(); }}>
        <div className="flex items-center gap-2 border-b px-2">
          <Search className="size-3.5 shrink-0 text-muted-foreground" />
          <Input ref={search} aria-label="Search models" role="combobox" aria-autocomplete="list"
            aria-expanded aria-controls={listId} aria-activedescendant={filtered[active] ? `${listId}-${active}` : undefined}
            placeholder="Search models..." value={query} autoComplete="off" spellCheck={false}
            className="border-0 px-0 text-xs shadow-none focus-visible:ring-0"
            onChange={(event) => { setQuery(event.target.value); setActive(0); }}
            onKeyDown={(event) => {
              if (event.key === "ArrowDown" || event.key === "ArrowUp") {
                event.preventDefault();
                setActive((current) => Math.max(0, Math.min(filtered.length - 1, current + (event.key === "ArrowDown" ? 1 : -1))));
              } else if (event.key === "Enter" && filtered[active]) {
                event.preventDefault();
                choose(filtered[active].value);
              }
            }} />
        </div>
        <div ref={list} id={listId} role="listbox" aria-label="Models" className="max-h-64 overflow-y-auto p-1">
          {filtered.map((model, index) => <div key={model.value} id={`${listId}-${index}`} role="option"
            aria-selected={value === model.value} data-index={index}
            className={cn("flex cursor-pointer items-center gap-2 rounded-sm px-2 py-2 text-xs", active === index && "bg-accent text-accent-foreground")}
            onMouseEnter={() => setActive(index)} onMouseDown={(event) => event.preventDefault()} onClick={() => choose(model.value)}>
            <div className="min-w-0 flex-1">
              <div className="truncate">{model.label}</div>
              {model.value !== model.label && <div className="truncate text-[10px] text-muted-foreground">{model.value}</div>}
            </div>
            {value === model.value && <Check className="size-3.5 shrink-0" />}
          </div>)}
          {!filtered.length && <p role="status" className="px-2 py-3 text-xs text-muted-foreground">No matching models</p>}
        </div>
      </Popover.Content>
    </Popover.Portal>
  </Popover.Root>;
}
