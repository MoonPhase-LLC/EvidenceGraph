import { expect, test } from "@playwright/test";

// Browser smoke test: the real frontend (real `src/main.tsx`, real CSS, the
// Vite dev server's real development CSP) in Chromium, with Tauri's official
// IPC mock standing in for Rust. It does NOT launch Tauri, the Python
// service, or supervision -- see `e2e-native/` for that.

test("app shell renders with mocked service status and health", async ({ page }) => {
  const problems: string[] = [];
  page.on("console", (msg) => {
    if (msg.type() === "error") problems.push(`console: ${msg.text()}`);
  });
  page.on("pageerror", (err) => problems.push(`pageerror: ${err.message}`));

  await page.goto("/e2e/smoke.html");

  // Shell structure.
  await expect(page.getByRole("heading", { level: 1, name: "Welcome to EvidenceGraph" })).toBeVisible();
  const nav = page.getByRole("navigation", { name: "Main" });
  await expect(nav).toBeVisible();
  for (const label of ["Assessments", "Settings"]) {
    // Exact accessible name as computed by Chromium (jsdom cannot compute it).
    const item = nav.getByRole("button", { name: `${label} Not available yet`, exact: true });
    await expect(item).toBeVisible();
    await expect(item).toHaveAttribute("aria-disabled", "true");
  }
  await expect(page.getByText("Assessment features are not available yet")).toBeVisible();

  // Mocked service: ready, with an authenticated health result.
  const service = page.getByRole("region", { name: "Local service" });
  await expect(service.getByText("Service ready")).toBeVisible();
  await expect(service.getByText("evidencegraph-service", { exact: true })).toBeVisible();
  await expect(service.getByText("0.1.0", { exact: true })).toBeVisible();

  // The supervisor reports a failure: wording changes, health is cleared.
  await page.evaluate(() =>
    (window as unknown as { __e2e: { setStatus(s: unknown): void } }).__e2e.setStatus({
      state: "failed",
      reason: "authenticated_connection_lost",
    }),
  );
  await expect(service.getByText("Service unavailable")).toBeVisible();
  await expect(service.getByText("authenticated_connection_lost")).toBeVisible();
  await expect(service.getByText("evidencegraph-service", { exact: true })).toHaveCount(0);

  // Only the two existing commands were used, and the page logged no errors
  // (including CSP violations).
  const calls = await page.evaluate(() => (window as unknown as { __e2e: { calls: string[] } }).__e2e.calls);
  expect(new Set(calls)).toEqual(new Set(["get_service_status", "check_service_health"]));
  expect(problems).toEqual([]);
});
