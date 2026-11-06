import "@testing-library/jest-dom/vitest";
import { configure } from "@testing-library/react";

// findBy and waitFor give up after one second by default, which a busy Mac
// can spend on a first render. A wait still returns as soon as it passes.
configure({ asyncUtilTimeout: 10_000 });

// jsdom has no ResizeObserver; radix-ui's Slider (AudioPlayer's seek bar)
// requires one at mount. A no-op stand-in is enough -- no test asserts on
// actual resize geometry.
class ResizeObserverStub {
  observe() {}
  unobserve() {}
  disconnect() {}
}
globalThis.ResizeObserver = ResizeObserverStub as unknown as typeof ResizeObserver;
