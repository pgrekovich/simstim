import {
  forwardRef,
  lazy,
  Suspense,
  useImperativeHandle,
  useRef,
  useState,
  type ChangeEvent,
  type FocusEvent,
  type KeyboardEvent,
} from "react";
import { cn } from "@/lib/utils";

const MarkdownRenderer = lazy(() => import("@/components/MarkdownRenderer"));

interface MarkdownContentProps {
  children: string;
  className?: string;
}

/** Safe Markdown rendering shared by user Notes and generated Summary. */
export function MarkdownContent({ children, className }: MarkdownContentProps) {
  return (
    <div className={cn("markdown-content", className)}>
      <Suspense
        fallback={
          <div aria-label="Rendering Markdown" className="space-y-2 py-1">
            <span className="block h-3 w-3/4 animate-pulse rounded bg-foreground/[0.07]" />
            <span className="block h-3 w-1/2 animate-pulse rounded bg-foreground/[0.07]" />
          </div>
        }
      >
        <MarkdownRenderer source={children} />
      </Suspense>
    </div>
  );
}

export interface MarkdownEditorHandle {
  focus: () => void;
}

interface MarkdownEditorProps {
  value: string;
  onChange: (event: ChangeEvent<HTMLTextAreaElement>) => void;
  onBlur: (event: FocusEvent<HTMLTextAreaElement>) => void;
  placeholder: string;
  className?: string;
}

/**
 * One surface with two states: rendered Markdown at rest and its plain-text
 * source while focused. The textarea always stays mounted so pending edits,
 * refs and the existing debounced save path remain stable across the switch.
 */
export const MarkdownEditor = forwardRef<MarkdownEditorHandle, MarkdownEditorProps>(
  function MarkdownEditor({ value, onChange, onBlur, placeholder, className }, ref) {
    const [editing, setEditing] = useState(false);
    const textareaRef = useRef<HTMLTextAreaElement>(null);

    function beginEditing() {
      setEditing(true);
      window.requestAnimationFrame(() => textareaRef.current?.focus());
    }

    useImperativeHandle(ref, () => ({ focus: beginEditing }));

    function handleKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
      if (event.key === "Escape" || (event.key === "Enter" && (event.metaKey || event.ctrlKey))) {
        event.preventDefault();
        event.currentTarget.blur();
      }
    }

    return (
      <div className={cn("relative flex min-h-0 w-full flex-1 flex-col", className)}>
        <textarea
          ref={textareaRef}
          value={value}
          onChange={onChange}
          onBlur={(event) => {
            setEditing(false);
            onBlur(event);
          }}
          onKeyDown={handleKeyDown}
          placeholder={placeholder}
          hidden={!editing}
          className="min-h-0 w-full flex-1 resize-none border-0 bg-transparent px-6 py-4 font-serif text-[15px] leading-[1.65] text-card-foreground outline-none placeholder:font-serif placeholder:text-muted-foreground focus-visible:ring-0"
        />

        {!editing && (
          <div
            onClick={(event) => {
              if ((event.target as HTMLElement).closest("a")) return;
              beginEditing();
            }}
            className="group min-h-0 flex-1 cursor-text overflow-y-auto px-6 py-4"
          >
            {value.trim() ? (
              <MarkdownContent>{value}</MarkdownContent>
            ) : (
              <p className="font-serif text-[15px] text-muted-foreground">{placeholder}</p>
            )}
            <button
              type="button"
              aria-label="Edit notes"
              onClick={(event) => {
                event.stopPropagation();
                beginEditing();
              }}
              className="absolute right-6 top-4 rounded-full bg-foreground/[0.06] px-2 py-1 font-mono text-[9px] uppercase tracking-[0.08em] text-muted-foreground opacity-0 outline-none transition-opacity hover:text-foreground focus-visible:opacity-100 focus-visible:ring-2 focus-visible:ring-ring/45 group-hover:opacity-100"
            >
              edit
            </button>
          </div>
        )}
      </div>
    );
  },
);
