/** How long a scrollbar stays visible after the last scroll event. */
const LINGER_MS = 700;

/**
 * Tags whichever element is actively scrolling with `is-scrolling`, which
 * index.css uses to reveal its (otherwise invisible) scrollbar thumb.
 *
 * WebKit shows native overlay scrollbars only while ::-webkit-scrollbar is
 * left unstyled; once it is customized, the thumb is either always visible
 * or never. CSS alone recovers the hover half ("*:hover" reveals it), and
 * this listener supplies the scroll half, so keyboard scrolling and
 * programmatic auto-scroll (live transcript) still flash the bar the way
 * native macOS overlay scrollbars do.
 *
 * One capture-phase listener on the document sees scrolls from every
 * container, present and future. Returns a teardown function.
 */
export function installScrollbarReveal(doc: Document = document): () => void {
  const timers = new Map<HTMLElement, ReturnType<typeof setTimeout>>();

  const onScroll = (event: Event) => {
    // A root-scroller event targets the document itself, not an element.
    const target =
      event.target instanceof HTMLElement
        ? event.target
        : (doc.scrollingElement as HTMLElement | null);
    if (!target) return;

    target.classList.add("is-scrolling");
    const pending = timers.get(target);
    if (pending !== undefined) clearTimeout(pending);
    timers.set(
      target,
      setTimeout(() => {
        target.classList.remove("is-scrolling");
        timers.delete(target);
      }, LINGER_MS),
    );
  };

  doc.addEventListener("scroll", onScroll, { capture: true, passive: true });
  return () => {
    doc.removeEventListener("scroll", onScroll, { capture: true });
    timers.forEach((timer, element) => {
      clearTimeout(timer);
      element.classList.remove("is-scrolling");
    });
    timers.clear();
  };
}
