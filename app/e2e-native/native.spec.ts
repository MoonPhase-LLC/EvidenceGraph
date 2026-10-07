import { execFileSync, spawn, type ChildProcess } from "node:child_process";
import { existsSync } from "node:fs";
import { createServer } from "node:net";
import path from "node:path";
import { chromium, expect, test, type Browser, type Page } from "@playwright/test";

// NATIVE smoke test (opt-in, Windows only): launches the real debug Tauri app
// -- real Rust supervision, real Python service, real authenticated health --
// and attaches Playwright to its WebView2 through a DevTools port bound to
// 127.0.0.1, enabled only for this test process via
// WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS. Nothing here changes the app.
//
// Prerequisites (see app/README.md): `uv sync` in service/, then
// `npm run tauri build -- --debug --no-bundle` in app/.

const APP_EXE = path.resolve(import.meta.dirname, "../src-tauri/target/debug/app.exe");

test.skip(process.platform !== "win32", "native smoke test drives WebView2 and Windows processes");

test.beforeAll(() => {
  if (!existsSync(APP_EXE)) {
    throw new Error(`${APP_EXE} not found. Build it first: npm run tauri build -- --debug --no-bundle`);
  }
  const running = powershell(
    `@(Get-CimInstance Win32_Process -Filter "Name='app.exe'" | Where-Object { $_.ExecutablePath -eq '${APP_EXE}' }).Count`,
  );
  if (running.trim() !== "0") {
    // WebView2 instances sharing a user-data folder share one browser
    // process, so a second DevTools port would be ignored.
    throw new Error("close the running EvidenceGraph debug app before running the native smoke test");
  }
});

function powershell(command: string): string {
  return execFileSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-Command", command], { encoding: "utf8" });
}

async function freeLoopbackPort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const server = createServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      server.close(() => resolve(typeof address === "object" && address ? address.port : 0));
    });
  });
}

/** PIDs of every evidencegraph_service process descended from `rootPid`. */
function servicePids(rootPid: number): number[] {
  const out = powershell(
    `$all = Get-CimInstance Win32_Process; $ids = @(${rootPid}); $found = @();
     do { $next = @($all | Where-Object { $ids -contains $_.ParentProcessId }); $ids = @($next | ForEach-Object ProcessId);
          $found += @($next | Where-Object { $_.CommandLine -like '*evidencegraph_service*' } | ForEach-Object ProcessId) } while ($next.Count -gt 0);
     $found -join ','`,
  ).trim();
  return out ? out.split(",").map(Number) : [];
}

function alive(pids: number[]): number[] {
  if (pids.length === 0) return [];
  const out = powershell(`@(Get-Process -Id ${pids.join(",")} -ErrorAction SilentlyContinue | ForEach-Object Id) -join ','`).trim();
  return out ? out.split(",").map(Number) : [];
}

type Launched = { app: ChildProcess; browser: Browser; page: Page; exited: Promise<number | null> };

async function launch(): Promise<Launched> {
  const port = await freeLoopbackPort();
  const app = spawn(APP_EXE, [], {
    env: { ...process.env, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}` },
    stdio: ["ignore", "ignore", "pipe"],
  });
  // Keep only the tail of the app's stderr, for diagnosing launch failures.
  let stderrTail = "";
  app.stderr?.on("data", (chunk: Buffer) => {
    stderrTail = (stderrTail + chunk.toString("utf8")).slice(-4000);
  });
  const exited = new Promise<number | null>((resolve) => app.once("exit", (code) => resolve(code)));

  let browser: Browser | undefined;
  const deadline = Date.now() + 30_000;
  while (!browser) {
    try {
      browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
    } catch (err) {
      if (Date.now() > deadline) {
        const state = app.exitCode === null && app.signalCode === null
          ? "app.exe is still running"
          : `app.exe exited (code ${app.exitCode}, signal ${app.signalCode})`;
        throw new Error(
          `WebView2 DevTools port ${port} never opened; ${state}.\n` +
            `last connect error: ${err instanceof Error ? err.message : String(err)}\n` +
            `app stderr tail:\n${stderrTail}`,
        );
      }
      await new Promise((r) => setTimeout(r, 250));
    }
  }
  let page: Page | undefined;
  while (!page && Date.now() < deadline) {
    page = browser.contexts().flatMap((c) => c.pages()).find((p) => p.url().startsWith("http://tauri.localhost"));
    if (!page) await new Promise((r) => setTimeout(r, 250));
  }
  if (!page) throw new Error("app page never appeared in WebView2");
  return { app, browser, page, exited };
}

/** Graceful close: `taskkill` without /F posts WM_CLOSE to the app's windows. */
async function closeGracefully({ app, browser, exited }: Launched): Promise<number | null> {
  await browser.close(); // disconnects only; does not end the app
  execFileSync("taskkill.exe", ["/PID", String(app.pid)], { stdio: "ignore" });
  return Promise.race([
    exited,
    new Promise<never>((_, reject) => setTimeout(() => reject(new Error("app did not exit within 20 s")), 20_000)),
  ]);
}

test.describe.configure({ mode: "serial" });

test("real app: supervised service reaches ready with authenticated health, and close cleans up", async () => {
  const launched = await launch();
  try {
    const service = launched.page.getByRole("region", { name: "Local service" });
    await expect(service.getByText("Service ready")).toBeVisible({ timeout: 30_000 });
    await expect(service.getByText("evidencegraph-service", { exact: true })).toBeVisible();

    const children = servicePids(launched.app.pid!);
    expect(children.length).toBeGreaterThan(0);

    expect(await closeGracefully(launched)).toBe(0);
    await expect.poll(() => alive(children), { timeout: 10_000 }).toEqual([]);
  } finally {
    if (launched.app.exitCode === null) launched.app.kill();
  }
});

test("real app: service crash revokes readiness and clears health", async () => {
  const launched = await launch();
  try {
    const service = launched.page.getByRole("region", { name: "Local service" });
    await expect(service.getByText("Service ready")).toBeVisible({ timeout: 30_000 });
    const children = servicePids(launched.app.pid!);

    // Kill the deepest service process (the interpreter that owns the
    // listener; `servicePids` lists ancestors first) from outside, as a crash
    // would. Its venv launcher parent then exits on its own.
    execFileSync("taskkill.exe", ["/F", "/PID", String(children[children.length - 1])], { stdio: "ignore" });

    await expect(service.getByText("Service unavailable")).toBeVisible({ timeout: 10_000 });
    await expect(service.getByText("evidencegraph-service", { exact: true })).toHaveCount(0);

    expect(await closeGracefully(launched)).toBe(0);
    expect(alive(children)).toEqual([]);
  } finally {
    if (launched.app.exitCode === null) launched.app.kill();
  }
});
