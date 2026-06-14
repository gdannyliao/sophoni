import { defineConfig } from "vitest/config";
import { svelte } from "@sveltejs/vite-plugin-svelte";

export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  server: {
    strictPort: true,
  },
  resolve: {
    // Svelte 5 ships separate client/server entries; vitest + jsdom must resolve
    // the client (browser) build so component `mount` works in tests.
    conditions: ["browser"],
  },
  envPrefix: ["VITE_", "TAURI_"],
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["./src/test/setup.ts"],
  },
});
