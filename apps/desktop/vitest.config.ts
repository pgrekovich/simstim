import path from "node:path";
import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// Minimal, separate from vite.config.ts (which Tauri also drives for the
// actual app build): component tests don't need the Tailwind plugin or the
// dev-server/Tauri-specific settings there, just React + the `@/*` alias
// and a DOM environment.
export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      "@": path.resolve(import.meta.dirname, "./src"),
    },
  },
  test: {
    environment: "jsdom",
    // Node's experimental storage globals conflict with jsdom on Node 24.
    // Keep the browser implementation provided by jsdom in every worker.
    execArgv: ["--no-experimental-webstorage"],
    globals: true,
    setupFiles: ["./src/test/setup.ts"],
  },
});
