import { defineConfig } from "@playwright/test";

// Opt-in NATIVE smoke test (Windows only): drives the real debug Tauri app
// through WebView2. Kept separate from `playwright.config.ts` so the browser
// smoke test never depends on a native build being present. See
// `e2e-native/native.spec.ts` for prerequisites.
export default defineConfig({
  testDir: "./e2e-native",
  forbidOnly: true,
  retries: 0,
  workers: 1,
  timeout: 90_000,
  reporter: [["list"]],
});
