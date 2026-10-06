# EvidenceGraph Desktop Shell

Tauri v2 desktop application shell with a React + TypeScript frontend
(Tailwind CSS v4 + shadcn/ui configured). See the root
[`CONTRIBUTING.md`](../CONTRIBUTING.md) for exact setup/build commands,
toolchain versions, and Windows prerequisites.

## Local service supervision (S1-04)

On launch, this app spawns, authenticates, and supervises the standalone
Python service in [`../service`](../service) -- see
`app/src-tauri/src/supervisor/mod.rs` for the full startup/shutdown
contract and `docs/DECISIONS.md` D-001/D-009/D-018/D-025 for the
architecture it implements. In brief:

- The child is launched by an explicit absolute path only, never PATH or a
  shell. In a dev build (`npm run tauri dev` / `cargo run` with
  `debug_assertions`), that's `../../service/.venv/Scripts/python.exe -m
  evidencegraph_service --supervised` -- run `uv sync` in `service/` first
  (see `service/README.md`), or the app fails closed with
  `executable_not_found`. In a release build it's a bundled sidecar
  executable S1-09 has not shipped yet, so release builds intentionally
  fail closed today.
- A per-launch startup secret and session token are generated fresh every
  run and exchanged over private inherited stdio pipes, never an
  environment variable, CLI argument, or HTTP request.
- Before any credential is installed, Tauri verifies the child's identity
  via a domain-separated HMAC-SHA-256 challenge over the child's own
  bound HTTP endpoint (D-025) -- the sole pre-authentication route,
  single-use and closed immediately after.
- The frontend never receives the session token, host, or port. It polls
  the `get_service_status` command for a `Starting → Ready`/`Failed`
  snapshot and calls `check_service_health`, which performs the
  authenticated `GET /health` round trip entirely in Rust.

## Frontend conventions (S1-05)

`src/` is organized by what code is for, not by file type. Add a directory
only when something needs it.

| Path | Holds |
|---|---|
| `src/components/ui/` | shadcn/ui primitives (`button`, `card`). Add them with `npx shadcn add <name>`, not by hand, and keep local edits to them minimal. |
| `src/components/layout/` | App frame pieces: `AppShell` (sidebar + scrolling main region, collapsing to a top bar below the `md` breakpoint) and `SidebarNav`. |
| `src/components/<area>/` | Reusable components for one area, e.g. `components/service/` for service status display. |
| `src/screens/` | One component per screen. `HomeScreen` is the only screen today. |
| `src/service/` | Everything that talks to the Rust side: the TypeScript mirrors of Rust types (`types.ts`), typed wrappers for the Tauri commands (`commands.ts`), and hooks over them (`useLocalService`). Components never call `invoke` directly. |
| `src/App.tsx` | Composition only: shell, navigation items, current screen. |

Rules that apply to every screen:

- **No routing or state-management library yet.** There is one screen.
  Add a router when a second screen actually exists. Until then,
  navigation items are visibly unavailable (`aria-disabled`, labelled
  "Not available yet") and do nothing.
- **No invented data.** Don't use sample assessments, scores, or
  placeholder findings. Show only what the backend actually returned, or
  say plainly that a feature is not available yet.
- **Render backend values as text.** Never use `dangerouslySetInnerHTML`
  or interpreted Markdown for service or evidence content
  (`docs/SECURITY.md` T-21).
- **Status wording comes from one place.** For the local service that is
  `components/service/serviceStatusText.ts`. Plain-language labels come
  first. Supervisor failure codes are shown only as secondary
  "Technical detail".
- **Accessibility basics.** Use semantic landmarks (`nav`, `main`) and
  headings, give every icon-only or ambiguous control an `aria-label`,
  and mark decorative icons `aria-hidden`. Use `aria-disabled` rather
  than `disabled` on a control that can hold focus while it is
  temporarily inactive (see the health "Check again" button), so keyboard
  focus is not lost. Don't fade such a control with opacity, because that
  fades its focus outline too.
- **One focus indicator.** Every keyboard-focusable control uses the same
  treatment: a 2px solid outline in the theme's `--foreground` color,
  offset 2px (`focus-visible:outline-2 focus-visible:outline-offset-2
  focus-visible:outline-solid focus-visible:outline-foreground`). It is
  built into `Button` and applied to the skip link. This measured above
  18:1 against the white and sidebar backgrounds. Apply it to any new
  focusable element that isn't a `Button`.
- **Announce asynchronous results.** Use a `role="status"` live region
  that is always mounted, so it exists before its content changes. Clear
  it when a new operation starts and fill it when the operation settles,
  so a repeated identical result is announced again. See the health
  region in `ServiceStatus.tsx`.
- Import with the `@/` alias (`@/components/...`, `@/service/...`). Note
  that the repository's root `.gitignore` ignores any `lib/` directory, so
  do not put tracked code in `src/lib/`. The shadcn `cn` helper comes from
  the `cn` package.

## Tests (S1-06)

Prerequisite: `npm ci` (from the committed lockfile). The Playwright
browser runs also need Chromium for Playwright, installed once per machine
with `npx playwright install chromium`.

| Command | Runner | What it covers |
|---|---|---|
| `npm test` | Vitest (jsdom + Testing Library) | `src/**/*.test.ts(x)`: the `useLocalService` hook and the app shell, with Tauri IPC mocked |
| `npm run test:e2e` | Playwright, Chromium | Browser smoke test: the real frontend served by the Vite dev server, with IPC mocked |
| `npm run test:e2e:native` | Playwright over WebView2 (Windows only, opt-in) | Native smoke test: the real debug Tauri app, with real supervision and the real service |

Each command exits non-zero when a test fails.

**Unit and component tests (`npm test`).**
- Tests live next to the code (`*.test.tsx`). `tsc` type-checks them as part of `npm run build`, but the Vite build never bundles them.
- The IPC boundary is replaced with Tauri's official `mockIPC` (`src/test/serviceIpc.ts`), so the real `invoke` path runs.
- Async ordering is controlled with fake timers and explicit deferred promises, never sleeps. Hook tests read the hook's returned state directly, because the UI hides health outside `ready` and would mask a stale write.
- `src/test/setup.ts` unmounts, clears the IPC mock and restores real timers after every test. It also provides a minimal `jest` timer shim that Testing Library needs under Vitest's fake timers.
- These are DOM tests. They don't prove what a screen reader speaks, or rendered colors and contrast.

**Browser smoke test (`npm run test:e2e`).**
- `e2e/smoke.html` and `e2e/smoke-entry.ts` install `mockIPC` and then load the app's real entry point, `src/main.tsx`. They are test-only files: the production build never references them, and there is no production mock mode.
- Playwright starts its own Vite dev server, bound to `localhost:1420` with the normal development CSP, and stops it afterwards. It never reuses an existing server, so the run fails if `npm run tauri dev` is already using port 1420.
- The test checks the shell, navigation, the mocked ready and health state, a failure transition, and that the page logs no errors (including CSP violations).
- **It does not launch Tauri, the Python service, or process supervision.**

**Native smoke test (`npm run test:e2e:native`).**
- Prerequisites: run `uv sync` in `service/`, build the debug app with `npm run tauri build -- --debug --no-bundle`, and close any running instance of that debug app.
- It launches `src-tauri/target/debug/app.exe` and attaches Playwright to its WebView2 through a DevTools port on `127.0.0.1`. That port is enabled only for the test process, through `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`; the app itself is unchanged.
- It checks real authenticated health, a service crash revoking readiness, and that a graceful close (WM_CLOSE) exits cleanly with the service processes gone.
- It is skipped (with a reason) on non-Windows platforms. It covers the debug build only, not an installed package (that is S1-09).

The Rust supervisor tests run with `cargo test` in `src-tauri/`, and the
Python service tests with `uv run pytest` in `../service/`.

## Recommended IDE Setup

- [VS Code](https://code.visualstudio.com/) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)
