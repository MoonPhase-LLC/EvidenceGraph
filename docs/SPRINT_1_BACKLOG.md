# EvidenceGraph — Sprint 1 Backlog: Desktop Foundation

Status: Sprint 0 draft. Tickets are scoped to be independently completable and reviewable. Do not
implement these during Sprint 0 — this is planning output only.

---

### S1-01: Repository tooling baseline

**Objective:** Establish shared repo-wide tooling config so later tickets have consistent
linting/formatting to build on.

**Implementation owner recommendation:** Claude Code

**Requirements:**
- `.editorconfig` for consistent whitespace/line-ending handling across the Rust/TS/Python code
  that's about to be added.
- Root-level `.gitignore` reviewed/extended for Tauri, Node, and Python build artifacts (current
  `.gitignore` should be checked, not assumed complete).
- Decide and document (in a short `CONTRIBUTING.md` or a section of `README.md`) the minimum tool
  versions expected (Node, Rust, Python).

**Non-goals:** Do not add linting rules for code that doesn't exist yet beyond baseline
config files (e.g. `.eslintrc`, `pyproject.toml` lint sections can be added in the tickets that
introduce those codebases).

**Dependencies:** None.

**Acceptance criteria:** A fresh clone shows consistent line endings/whitespace handling; version
requirements are documented somewhere discoverable.

**Required tests:** None (config-only ticket).

---

### S1-02: Tauri application shell

**Objective:** Initialize the Tauri desktop shell with a minimal React + TypeScript frontend.

**Implementation owner recommendation:** Claude Code

**Requirements:**
- Initialize Tauri project targeting Windows.
- Minimal React + TypeScript frontend (Tailwind + shadcn/ui configured, per
  `docs/ARCHITECTURE.md`) rendering a placeholder screen.
- Tauri capability/permission configuration scoped minimally (per `docs/SECURITY.md` T-14): only
  what's needed for a file-open dialog and spawning one child process — no broad shell/filesystem
  capabilities granted "just in case."
- App builds and launches on Windows in dev mode.

**Non-goals:** No real feature screens. No IPC to a backend yet (that's S1-04).

**Dependencies:** S1-01.

**Acceptance criteria:** `tauri dev` launches a window showing the placeholder screen on Windows.

**Required tests:** A basic frontend smoke test (renders without error) if a test runner is
already configured as part of this ticket; otherwise defer to S1-06.

---

### S1-03: FastAPI local service skeleton

**Objective:** Initialize the Python/FastAPI local analysis service as a standalone,
independently runnable project.

**Implementation owner recommendation:** OpenAI Codex

**Requirements:**
- FastAPI project structure under a clear subdirectory (e.g. `service/`).
- One health-check endpoint (`GET /health`) returning basic status JSON.
- Service binds to `127.0.0.1` only, with the port configurable (not hardcoded) — see
  `docs/ARCHITECTURE.md` §4 open question on port allocation; for this ticket, an env-var or
  CLI-arg-configurable port with a documented default is sufficient, final allocation strategy
  can follow in S1-04/later.
- Auth-check middleware: every request (except perhaps `/health` itself, for simple liveness
  probing — product owner's call) must carry a shared-secret token matching an env-var the process
  was launched with; mismatched/missing token → 401. The token itself is generated and propagated
  by S1-04 — this ticket just needs to enforce it once present (`docs/DECISIONS.md` D-009).
- Python dependency management set up (e.g. `pyproject.toml`).

**Non-goals:** No database, no real business logic endpoints yet.

**Dependencies:** S1-01.

**Acceptance criteria:** `GET http://127.0.0.1:<port>/health` returns a 200 with a status payload
when run standalone (without Tauri) and a valid token env var set; a request with a missing/wrong
token to an auth-required endpoint returns 401.

**Required tests:** A test hitting `/health` and asserting the response shape; a test asserting the
401 behavior on a missing/invalid token.

---

### S1-04: Tauri-spawns-FastAPI process supervision

**Objective:** Validate the riskiest architectural assumption from `docs/DECISIONS.md` D-001 —
that Tauri can reliably launch, supervise, and cleanly terminate the Python service.

**Implementation owner recommendation:** Claude Code (Tauri side) with Codex reviewing the Python
process-lifecycle implications

**Requirements:**
- On app start, Tauri launches the FastAPI service as a child process.
- Tauri determines/passes the port the service should bind to (resolving the S1-03 open
  question — e.g. Tauri picks an available local port and passes it via env var/arg).
- Tauri generates a random per-launch shared-secret token and passes it to the FastAPI child
  process via environment variable (never a CLI argument), and separately exposes the same token to
  the frontend via Tauri's own IPC (not over the local HTTP channel) so the frontend can attach it
  to every request (`docs/DECISIONS.md` D-009). The token lives only in memory for the session.
- On app exit (including abnormal exit paths where feasible), the child process is terminated —
  no orphaned Python processes left running.
- Frontend can successfully call the `/health` endpoint through this supervised, authenticated
  process and display the result on the placeholder screen from S1-02.

**Non-goals:** No production-grade process supervision (auto-restart on crash, etc.) — that can
be a later hardening ticket if needed.

**Dependencies:** S1-02, S1-03.

**Acceptance criteria:** Launching the Tauri app starts the service; the frontend displays a
successful health check; closing the app leaves no orphaned `python`/service process running
(verified manually via OS process list during review).

**Required tests:** Manual verification steps documented in the PR description (process lifecycle
is hard to unit test meaningfully at this stage); at minimum, an automated check that the
frontend→service call succeeds in a dev/CI-runnable way if feasible.

---

### S1-05: Frontend project conventions and base UI kit

**Objective:** Establish the frontend code organization and base UI components (via shadcn/ui)
that later feature screens will build on, avoiding every future ticket reinventing basics.

**Implementation owner recommendation:** Claude Code

**Requirements:**
- Directory structure for components/screens/state (decide and document a simple convention —
  this is an implementation detail, not an architectural one, so it doesn't need product-owner
  approval, just consistency).
- A small set of shadcn/ui components installed and demonstrated on the placeholder screen
  (button, layout shell, basic nav placeholder for future screens: Assessments, Settings).
- TypeScript strict mode enabled.

**Non-goals:** No real navigation/routing to non-existent screens; no state management library
decision beyond what's needed for this skeleton (defer to when real state exists in Sprint 2).

**Dependencies:** S1-02.

**Acceptance criteria:** Placeholder screen uses at least one shadcn/ui component; TypeScript
compiles with strict mode and no errors.

**Required tests:** None beyond the smoke test from S1-02/S1-06.

---

### S1-06: Test runner setup (Vitest, pytest, Playwright scaffolding)

**Objective:** Wire up the test runners named in the proposed stack so later tickets can add real
tests rather than each reinventing configuration.

**Implementation owner recommendation:** OpenAI Codex (pytest, Playwright config), Claude Code
(Vitest config) — natural split along frontend/backend lines

**Requirements:**
- Vitest configured for the frontend; one passing smoke test.
- pytest configured for the FastAPI service; the S1-03 `/health` test runs under it.
- Playwright installed and configured with one smoke test that launches the built app (or dev
  server, if launching the packaged app isn't feasible yet) and checks the placeholder screen
  renders — acceptable to mark this test as best-effort/skippable in CI if Playwright-against-Tauri
  proves environment-fragile; document the limitation rather than silently disabling it.

**Non-goals:** No coverage targets/thresholds enforced yet.

**Dependencies:** S1-02, S1-03, S1-04 (Playwright smoke test needs the supervised process working).

**Acceptance criteria:** `vitest run`, `pytest`, and the Playwright smoke test all run locally and
produce a pass/fail result.

**Required tests:** The smoke tests described above are the deliverable.

---

### S1-07: CI pipeline

**Objective:** Run the Sprint 1 checks automatically on every PR.

**Implementation owner recommendation:** Claude Code (given repo-wide CI ownership), reviewed by
Codex

**Requirements:**
- GitHub Actions workflow running: frontend lint + typecheck + Vitest; backend lint/typecheck
  (e.g. ruff/mypy if adopted — decide minimally, document choice) + pytest.
- Playwright smoke test included if it proved stable in S1-06; otherwise a follow-up ticket, not
  silently dropped.
- CI must not require network access to any evidence-adjacent or AI service (this stage has
  neither yet, but keep the precedent clean from the start).

**Non-goals:** No deployment/release pipeline yet (that's Sprint 16).

**Dependencies:** S1-06.

**Acceptance criteria:** Opening a PR against this repo triggers the workflow; a deliberately
broken test causes CI to fail; a clean state passes.

**Required tests:** N/A (this ticket is the test infrastructure).

---

### S1-08: Sprint 1 exit validation

**Objective:** Confirm Sprint 1's exit criteria from `docs/SPRINTS.md` are actually met before
Sprint 2 begins, and record the packaging-risk outcome.

**Implementation owner recommendation:** Human Product Owner, with a written summary from
whichever agent completes S1-04

**Requirements:**
- Manually verify the full loop (app launch → service spawn → frontend health check → clean
  shutdown) on a real Windows machine, not just in a dev container.
- Update `docs/DECISIONS.md` D-001 with the actual outcome of the Python-runtime-bundling risk
  (resolved cleanly / needs a follow-up mitigation ticket) instead of leaving it as a forward-
  looking flag.

**Non-goals:** No packaging/installer work (that's Sprint 16) — this is dev-mode validation only.

**Dependencies:** S1-04, S1-07.

**Acceptance criteria:** Sign-off recorded (e.g. as a comment/update on this backlog or in
`DECISIONS.md`) that Sprint 1's exit criteria are met.

**Required tests:** N/A.

---

### S1-09: Packaged-build spike (clean-machine validation)

**Objective:** Validate the actual top technical risk from `docs/DECISIONS.md` D-001 — bundling a
Python runtime inside a Tauri app for distribution — as an early, narrow spike, rather than
discovering packaging problems for the first time in Sprint 16 after fifteen sprints of feature
work have been built on an unvalidated assumption. This is deliberately **not** full packaging/
installer work (that stays Sprint 16); it is a minimal end-to-end proof.

**Implementation owner recommendation:** Claude Code (Tauri bundler config), with Codex advising on
Python runtime bundling options (PyInstaller/embedded interpreter/etc.)

**Requirements:**
- Run the Tauri bundler to produce an installable Windows package (e.g. an MSI/NSIS installer) that
  embeds or bundles the Python FastAPI service in some form — the exact bundling approach
  (PyInstaller-frozen executable, embedded Python distribution, etc.) is this ticket's own decision
  to make and document, not assumed in advance.
- Install and launch that package on a Windows machine **without** the development toolchain
  installed (no system Python, no Node, no Rust toolchain present) — a real proxy for "a customer's
  machine," not another dev environment.
- Confirm the packaged app launches, spawns the bundled service, and completes one authenticated
  health-check round-trip (reusing S1-04's mechanism), then shuts down cleanly with no orphaned
  processes.
- Record the outcome — clean pass, or specific blocking issues found — in `docs/DECISIONS.md`
  D-001, updating it from "flagged as a risk" to either "validated early" or "needs a follow-up
  mitigation ticket before Sprint 16," with concrete detail either way.

**Non-goals:** No auto-update mechanism, no code signing, no installer UX polish, no support for
non-Windows platforms — all Sprint 16 concerns. This ticket only needs to prove the bundling
mechanism *can* work, not make it production-ready.

**Dependencies:** S1-02, S1-03, S1-04.

**Acceptance criteria:** A built installer package launches and passes the health-check round-trip
on a clean Windows machine; `docs/DECISIONS.md` D-001 is updated with the concrete outcome.

**Required tests:** Manual verification on a clean machine (VM snapshot without dev tooling is
acceptable), documented in the PR description — this is inherently a packaging/environment
validation, not something meaningfully unit-testable.
