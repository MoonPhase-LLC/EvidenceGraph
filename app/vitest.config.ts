import path from "node:path";
import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// Unit/component tests (S1-06). Separate from `vite.config.ts` so the dev
// server's CSP/HMR settings stay untouched. Test files live next to the code
// they test (`src/**/*.test.ts(x)`); Vite's production build only bundles
// what `index.html` imports, so they never reach `dist/`.
export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      "@": path.resolve(import.meta.dirname, "./src"),
    },
  },
  test: {
    environment: "jsdom",
    include: ["src/**/*.test.{ts,tsx}"],
    setupFiles: ["./src/test/setup.ts"],
    restoreMocks: true,
    unstubGlobals: true,
  },
});
