import { useRef, type PointerEvent as ReactPointerEvent } from "react";

interface ResizeHandleProps {
  /** Accessible name ("Resize sidebar", "Resize Ask this call"). */
  label: string;
  /**
 * On drag start, the parent snapshots the pane width.
 *
 */
  onDragStart: () => void;
  /** Total horizontal travel since the drag began, in px (signed). */
  onDrag: (deltaX: number) => void;
  /**
 * On double-click, the parent restores the default width.
 *
 */
  onReset: () => void;
}

/**
 * Pane divider with a nine-pixel hit area and zero layout width. Window
 * listeners keep dragging active outside the divider and are removed on
 * pointerup.
 *
 */
export function ResizeHandle({ label, onDragStart, onDrag, onReset }: ResizeHandleProps) {
  // The active drag's cleanup, so an unmount mid-drag can't leak listeners.
  const cleanupRef = useRef<(() => void) | null>(null);

  function handlePointerDown(event: ReactPointerEvent<HTMLDivElement>) {
    if (event.button !== 0) return;
    event.preventDefault();
    cleanupRef.current?.();
    const startX = event.clientX;
    onDragStart();
    // The pointer roams the whole window during a drag: keep the resize
    // cursor and suppress text selection until release.
    const previousCursor = document.body.style.cursor;
    const previousUserSelect = document.body.style.userSelect;
    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";
    function handleMove(moveEvent: globalThis.PointerEvent) {
      onDrag(moveEvent.clientX - startX);
    }
    function cleanup() {
      window.removeEventListener("pointermove", handleMove);
      window.removeEventListener("pointerup", cleanup);
      document.body.style.cursor = previousCursor;
      document.body.style.userSelect = previousUserSelect;
      cleanupRef.current = null;
    }
    window.addEventListener("pointermove", handleMove);
    window.addEventListener("pointerup", cleanup);
    cleanupRef.current = cleanup;
  }

  return (
    <div className="relative z-20 w-0 shrink-0">
      <div
        role="separator"
        aria-orientation="vertical"
        aria-label={label}
        title="Drag to resize · double-click to reset"
        onPointerDown={handlePointerDown}
        onDoubleClick={onReset}
        className="absolute inset-y-0 -left-[4px] w-[9px] cursor-col-resize transition-colors after:absolute after:inset-y-0 after:left-[3.5px] after:w-[2px] after:rounded-full after:bg-transparent after:transition-colors hover:after:bg-foreground/[0.16] active:after:bg-foreground/[0.28]"
      />
    </div>
  );
}
