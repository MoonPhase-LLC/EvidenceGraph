import { defineConfig, devices } from "@playwright/test";

// Browser smoke test (S1-06): the real frontend in Chromium against the Vite
// dev server, with Tauri IPC mocked inside test code (`e2e/smoke-entry.ts`).
// This does not launch Tauri or the Python service; native coverage is the
// separate, opt-in `playwright.native.config.ts`.
//
// The dev server is bound to localhost only, on Vite's configured port 1420
// with strictPort, and is started and stopped by Playwright. It is never
// reused: if port 1420 is already taken (e.g. `npm run tauri dev` is running),
// the run fails instead of testing whatever is listening there.
const PORT = 1420;

export default defineConfig({
  testDir: "./e2e",
  forbidOnly: true,
  retries: 0,
  workers: 1,
  reporter: [["list"]],
  use: {
    baseURL: `http://localhost:${PORT}`,
    trace: "retain-on-failure",
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
  webServer: {
    command: `npx vite --host localhost --port ${PORT} --strictPort`,
    url: `http://localhost:${PORT}/e2e/smoke.html`,
    reuseExistingServer: false,
    timeout: 60_000,
  },
});
