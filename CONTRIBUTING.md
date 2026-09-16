# Contributing to EvidenceGraph

This document covers repository conventions and the toolchain versions the Tauri/Rust +
React/TypeScript desktop shell (`app/`, since S1-02) and the upcoming Python/FastAPI service
target. See
[`docs/AGENT_INSTRUCTIONS.md`](docs/AGENT_INSTRUCTIONS.md) for the rules that govern any
coding agent working in this repository, and the root [`CLAUDE.md`](CLAUDE.md) for the git
workflow.

## Repository conventions

- **`.editorconfig`** defines whitespace, indentation, encoding, and line-ending rules per
  language. JetBrains IDEs support EditorConfig natively; VS Code requires installing the
  [EditorConfig for VS Code](https://marketplace.visualstudio.com/items?itemName=EditorConfig.EditorConfig)
  extension — it is not built in. See the official
  [list of editors/plugins with EditorConfig support](https://editorconfig.org/#pre-installed)
  for your own editor. It does not reformat existing files by itself — it only guides new
  edits.
- **`.gitignore`** covers Node/frontend, Rust/Tauri, and Python build artifacts, plus common
  editor/OS clutter and local-secret filename patterns. Lockfiles (`Cargo.lock`,
  `package-lock.json`, etc.) are intentionally **not** ignored — see "Dependency lockfiles"
  below.
- Language-specific lint/format tooling (ESLint, ruff, mypy, rustfmt config, etc.) is
  introduced by the tickets that create each project, not here (see `docs/SPRINT_1_BACKLOG.md`
  S1-01's non-goals).

### Dependency lockfiles

Commit each project's lockfile so builds are reproducible:

- `app/src-tauri/Cargo.lock` (Rust/Tauri — this is an application, not a library, so the
  lockfile is committed per Cargo's own guidance). Committed as of S1-02.
- `app/package-lock.json` (npm was used as the package manager — already available with the
  Node.js install, no extra tooling required). Committed as of S1-02.
- A Python lockfile appropriate to whatever dependency manager is chosen for the FastAPI
  service (e.g. `uv.lock` or `poetry.lock`) — not yet applicable, pending S1-03.

## Minimum and recommended tool versions

"Minimum supported" is the oldest version the project is expected to work with. "Recommended"
is what contributors should actually install for day-to-day development. These were first
documented in S1-01 before any project existed, and are now validated against the actual
Tauri/React dependency set initialized in S1-02, so later tickets (starting with the Python/
FastAPI service in S1-03) have a consistent baseline instead of each picking its own.

| Tool | Minimum supported | Recommended for development | Actually tested (S1-02) |
|---|---|---|---|
| Node.js | 22.12.0+ | 24.x (Active LTS) | 24.19.0 |
| Rust | 1.98.0 (pinned, see note) | latest stable via `rustup update` | 1.98.0 (`stable-x86_64-pc-windows-msvc`) |
| Python | 3.11 | 3.12 | not applicable yet (no Python project exists — see S1-03/S1-06) |

Rationale:

- **Node.js**: Vite (used by the `app/` frontend, see below) declares
  `"engines": {"node": "^20.19.0 || >=22.12.0"}` in its own `package.json` — verified directly
  against the installed `vite@8.3.0` package during S1-02, not just Vite's prose docs. Since
  Node 20.x is already at or past end-of-life while `22.x` remains under Maintenance LTS
  (supported until 2027-04-30), the floor is `22.12.0`. Node 24.x is the current Active LTS
  line (supported until 2028-04-30); S1-02 was actually built and verified with Node `24.19.0`.
  Source: `vite`'s published `package.json` `engines` field and the
  [Node.js Release schedule](https://github.com/nodejs/Release#readme). A later ticket that adds
  a frontend dependency with a higher declared Node requirement should raise this floor and
  update this row, not silently rely on a newer Node than documented here.
- **Rust**: S1-02 initialized the Tauri v2 project (`tauri` `2.11.5`, `tauri-cli` `2.11.4`) and
  built/ran it successfully with Rust **1.98.0** stable
  (`rustc 1.98.0 (88d9e12ae 2026-08-18)`, MSVC toolchain, `x86_64-pc-windows-msvc` target). That
  exact version is pinned reproducibly in
  [`app/src-tauri/rust-toolchain.toml`](app/src-tauri/rust-toolchain.toml), so `rustup` installs
  and selects it automatically for anyone building `app/src-tauri`. This is the actually-tested
  version, not a guess: `cargo check --locked` and a real `tauri dev` launch both passed against
  it (see S1-02's PR for verification detail). Tauri v2 originally shipped with a lower
  published upstream MSRV of 1.78
  ([tauri-apps/tauri#11205](https://github.com/tauri-apps/tauri/pull/11205)), but that
  historical figure was never validated against *this* project's actual dependency set and is
  superseded by the pinned, tested `1.98.0` above — do not resurrect `1.78` as a claimed floor.
  If a future ticket needs to lower the pin (e.g. for broader contributor compatibility), it
  must re-verify the build against that lower version first.
- **Python**: unchanged from S1-01 — no Python/FastAPI project exists yet (that's S1-03), so
  there is nothing new to test here. Python 3.10 is within weeks of losing even security support
  (end-of-life 2026-10-31 per the [Python developer's guide release
  cycle](https://devguide.python.org/versions/)), so it remains an unsafe floor for a project
  starting now; Python 3.11 (security support until 2027-10-31) is the documented minimum and
  3.12 (until 2028-10-31) the recommended development version, pending S1-03 validating both
  against whatever FastAPI/SQLAlchemy/llama.cpp-binding versions it actually selects.

If these facts change (a version reaches end-of-life, or a dependency requires a newer
floor), update this table rather than letting each project silently diverge.

## Windows development prerequisites (Tauri stack)

Sourced from the [official Tauri v2 Windows prerequisites](https://v2.tauri.app/start/prerequisites/)
and the [official Rust Windows install guidance](https://www.rust-lang.org/tools/install),
checked 2026-09-15. The developer-machine items below were actually exercised while building
`app/` in S1-02 (a real Tauri window was launched and closed on Windows — see that PR for
details); the end-user items are documentation only, since no packaged installer exists yet
(that's S1-09).

### Developer machine requirements (building/running the app from source)

- **Microsoft C++ Build Tools**: install the "Desktop development with C++" workload (via the
  Visual Studio Installer or the standalone Build Tools installer). Rust's MSVC toolchain
  depends on this.
- **Rust with the MSVC toolchain**: install via [rustup](https://rustup.rs/). `app/src-tauri`
  pins its toolchain in `rust-toolchain.toml` (currently `1.98.0`, `x86_64-pc-windows-msvc`
  target) — running any `cargo`/`tauri` command inside `app/src-tauri` makes `rustup`
  auto-install and select that exact version, so no manual toolchain selection is needed beyond
  having `rustup` itself installed.
- **Node.js**: 22.12.0+ (see table above); tested with 24.19.0.
- **WebView2 Runtime**: needed to build/run the app locally if it isn't already present. See
  end-user note below — most current Windows installs already have it.

### End-user requirements (once the app is packaged and distributed — not relevant yet)

- **WebView2 Runtime**: Tauri renders the UI using Microsoft's WebView2. It ships
  preinstalled on Windows 10 (from version 1803 onward) and Windows 11. Only end users on
  older/unusual Windows configurations would need the separate "Evergreen Bootstrapper"
  installer, which the Tauri bundler can optionally include.
- End users do **not** need Rust, Node.js, or C++ Build Tools — those are build-time
  dependencies only, not runtime requirements for the packaged app.

## What you can actually run today

The Tauri desktop shell (`app/`) exists as of S1-02: a minimal Tauri v2 + React + TypeScript
project with Tailwind CSS v4 and shadcn/ui configured, rendering a static placeholder screen.
**There is still no Python/FastAPI service, no backend IPC, and no database** — those are
S1-03/S1-04, not this ticket. Do not run or expect any Python/`pytest` command yet.

From the `app/` directory:

```sh
npm install       # install frontend dependencies (also required before any cargo/tauri command)
npm run dev       # Vite dev server only (frontend in a browser, no Tauri window)
npm run build     # tsc type check (noEmit, per tsconfig) + production Vite build to app/dist
npm run tauri dev # launch the actual Tauri desktop window in dev mode (requires Rust prerequisites above)
npm run tauri build # produce a release build/installer (not yet exercised end-to-end — see S1-09)
```

`cargo check`/`cargo build` also work directly from `app/src-tauri` once Node dependencies have
been installed at least once (Tauri's build script reads the frontend's `dist/` output path from
`tauri.conf.json`).

No test runner is configured yet for either the frontend or (once it exists) the backend — that
is S1-06's scope, not S1-02's. There is no rendering smoke test in `app/` for the same reason.
