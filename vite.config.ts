import { defineConfig } from "vitest/config";
import { svelte } from "@sveltejs/vite-plugin-svelte";

const shared = {
  clearScreen: false,
  server: { strictPort: true },
  envPrefix: ["VITE_", "TAURI_"],
};

export default defineConfig({
  ...shared,
  plugins: [svelte()],
  resolve: {
    // Svelte 5 ships separate client/server entries; vitest + jsdom and the
    // browser dev server must resolve the client (browser) build.
    conditions: ["browser"],
  },
  test: {
    exclude: ["**/node_modules/**", "**/dist/**", ".worktrees/**"],
    projects: [
      {
        plugins: [svelte()],
        resolve: { conditions: ["browser"] },
        test: {
          name: "browser",
          environment: "jsdom",
          globals: true,
          setupFiles: ["./src/test/setup.ts"],
          include: ["src/**/*.{test,spec}.{js,ts}"],
        },
      },
      {
        test: {
          name: "node",
          environment: "node",
          globals: true,
          include: ["scripts/acceptance/**/*.{test,spec}.{js,ts}"],
        },
      },
    ],
  },
});
