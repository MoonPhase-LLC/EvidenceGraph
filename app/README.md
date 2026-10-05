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
  mark decorative icons `aria-hidden`, and keep the default visible focus
  ring. Use `aria-disabled` rather than `disabled` on a control that can
  hold focus while it is temporarily inactive (see the health
  "Check again" button), so keyboard focus is not lost.
- Import with the `@/` alias (`@/components/...`, `@/service/...`). Note
  that the repository's root `.gitignore` ignores any `lib/` directory, so
  do not put tracked code in `src/lib/`. The shadcn `cn` helper comes from
  the `cn` package.

There is no frontend test runner yet; that is S1-06.

## Recommended IDE Setup

- [VS Code](https://code.visualstudio.com/) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)
