// Explicit developer-only preview. The normal Vite config never imports this.
import path from "node:path";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: [
      { find: /^@tauri-apps\/.+$/, replacement: path.resolve(import.meta.dirname, "demo/tauri.ts") },
      { find: "@", replacement: path.resolve(import.meta.dirname, "src") },
    ],
  },
  server: { host: "127.0.0.1", port: 1422, strictPort: true },
});
