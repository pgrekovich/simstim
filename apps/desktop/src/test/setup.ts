import "@testing-library/jest-dom/vitest";

// jsdom has no ResizeObserver; radix-ui's Slider (AudioPlayer's seek bar)
// requires one at mount. A no-op stand-in is enough -- no test asserts on
// actual resize geometry.
class ResizeObserverStub {
  observe() {}
  unobserve() {}
  disconnect() {}
}
globalThis.ResizeObserver = ResizeObserverStub as unknown as typeof ResizeObserver;
