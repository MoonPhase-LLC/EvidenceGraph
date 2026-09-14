# EvidenceGraph — Sprint Plan (V0.1)

Status: Sprint 0 draft. Sequencing and sprint-level scope for reaching a usable V0.1. Only
Sprint 1 has a detailed ticket backlog (`docs/SPRINT_1_BACKLOG.md`); later sprints are
intentionally sketched at objective/deliverable level only, per the Sprint 0 brief.

## Agent Roles (context for "responsibilities" below)

- **Human Product Owner**: product decisions, compliance interpretation, architecture approval,
  UX decisions, acceptance testing, tie-breaking when agents disagree.
- **Claude Code**: primary implementation agent — larger features, frontend, repo-wide changes,
  refactors, integration work.
- **OpenAI Codex**: secondary implementation/review agent — backend implementation, tests,
  debugging, security review, isolated technical features, adversarial code review.

These are preferences, not rigid limits. Normally one agent implements a feature and the other
reviews it; avoid both agents modifying the same subsystem concurrently.

---

## Sprint 0 — Architecture and Product Definition

**Objective:** Establish documentation, structure, and decisions needed before any
implementation begins.

**Deliverables:** This document set (`docs/*.md`), `frameworks/nist-800-53-rev5/` placeholder,
`docs/DECISIONS.md`, `docs/OPEN_QUESTIONS.md`, updated `README.md`.

**Claude Code responsibilities:** Author the documentation set (this sprint).

**Codex responsibilities:** None assigned this sprint; may review documentation for
implementation feasibility once Sprint 1 begins.

**Human responsibilities:** Resolve items in `docs/OPEN_QUESTIONS.md`; approve architecture.

**Dependencies:** None.

**Exit criteria:** All Sprint 0 deliverables exist; no fabricated NIST content; no application
code written; open questions documented rather than silently resolved.

---

## Sprint 1 — Desktop Foundation

**Objective:** Prove the Tauri + React/TS + Python/FastAPI shape actually works end to end as a
minimal skeleton, before building real features on top of an unvalidated assumption.

**Deliverables:** Tauri app shell that launches; FastAPI local service spawned as a child process,
bound to localhost and authenticated via the per-launch shared-secret token plus the fail-closed
startup identity-verification handshake (`DECISIONS.md` D-009, D-018) — not just a token check, but
verifying the service Tauri is talking to is actually the one it spawned before any credential or
data crosses the boundary; a minimal frontend screen that calls one localhost API endpoint and
displays the result; basic project tooling (linting, formatting, test runners) wired up for both
the Rust/TS and Python sides; CI running those checks on PRs; a narrow **packaged-build spike** (see
S1-09 in `docs/SPRINT_1_BACKLOG.md`) that runs the Tauri bundler to produce an installable package
embedding the Python service, and launches that package on a clean Windows machine (not just `tauri
dev`) — distinct from and much smaller than Sprint 16's full packaging/release work, but validating
the actual riskiest assumption (`DECISIONS.md` D-001) far earlier than "Sprint 16" would otherwise
allow.

**Claude Code responsibilities:** Tauri shell, React/TS frontend skeleton, repo-wide tooling
config, CI workflow, packaged-build spike.

**Codex responsibilities:** FastAPI service skeleton (including the shared-secret auth check),
Python project tooling (linting/typing/test runner), backend unit test scaffolding.

**Human responsibilities:** Approve the validated Tauri↔FastAPI process-supervision approach
before Sprint 2 builds on it; resolve the port-allocation open question; confirm the packaged-build
spike's outcome (clean pass / needs follow-up mitigation) before treating D-001's risk as addressed.

**Dependencies:** Sprint 0 docs.

**Exit criteria:** App launches on Windows in dev mode; frontend successfully round-trips a call to
the local service through the authenticated, identity-verified channel (D-018's handshake, not just
a bearer-token check); an occupied-port / impersonating-process test case is demonstrated to fail
closed rather than leak the token; CI passes on a clean checkout; the packaged-build spike produces
a launchable installed package on a machine without dev tooling, or documents a concrete blocking
issue and mitigation plan; the Python-runtime-bundling risk flagged in `DECISIONS.md` D-001 is
either resolved or has a concrete mitigation plan grounded in that spike's actual result, not just a
dev-mode assumption. See `docs/SPRINT_1_BACKLOG.md` for tickets.

---

## Sprint 2 — Assessment and Database Layer

**Objective:** Stand up SQLite + SQLAlchemy with the core schema from `docs/DATABASE.md` and
Alembic migrations, plus basic Assessment CRUD end to end (API + minimal UI).

**Deliverables:** Initial migration creating `framework`, `control_family`, `control`,
`control_enhancement`, `assessment` tables (evidence/mapping tables can follow in Sprint 4/8 as
needed, or be included here if convenient); Assessment create/list/view API and UI.

**Claude Code responsibilities:** Frontend assessment screens; integration wiring.

**Codex responsibilities:** SQLAlchemy models, Alembic migration setup, backend CRUD endpoints,
backend tests.

**Human responsibilities:** Approve final schema deviations from `docs/DATABASE.md` if any arise.

**Dependencies:** Sprint 1.

**Exit criteria:** User can create/list/view an assessment; data persists across app restarts;
migration is reproducible on a clean database.

---

## Sprint 3 — NIST 800-53 Framework Engine

**Objective:** Build the generic Framework → Family → Control → Enhancement engine and load real
NIST SP 800-53 Rev. 5 data (from an official/authoritative source, obtained explicitly for this
task — not fabricated) into `frameworks/nist-800-53-rev5/`.

**Deliverables:** Framework data file format (finalized); loader/importer; control browsing API
and minimal UI; official NIST text rendered and clearly labeled as such.

**Claude Code responsibilities:** Framework browsing UI.

**Codex responsibilities:** Framework data loader, data format validation, import tooling.

**Human responsibilities:** Provide or approve the authoritative NIST 800-53 Rev. 5 source data;
confirm licensing/attribution requirements are met.

**Dependencies:** Sprint 2.

**Exit criteria:** Full NIST 800-53 Rev. 5 control catalog loaded and browsable; no
product-generated text presented as official framework language; framework engine has no
NIST-specific hardcoding (verified by code review against `ARCHITECTURE.md` §8's genericity rule).

---

## Sprint 4 — Evidence Ingestion

**Objective:** Implement secure file upload, hashing, duplicate detection, and text
extraction/chunking for PDF/DOCX/TXT/CSV.

**Deliverables:** Upload API/UI; artifact + artifact_section tables and migration; parsers with
the size/decompression/path-traversal mitigations from `SECURITY.md`; **the parser containment
boundary itself**, meeting the specific permission requirements in `DECISIONS.md` D-022 (read-only
on its own input, write access limited to scratch, no DB/credential/network access, resource
bounds) — the specific mechanism providing these properties is still per `OPEN_QUESTIONS.md` S-1,
but the properties themselves and a proof they hold are decided and implemented here, not deferred,
and "wrapped in a subprocess" alone does not satisfy this requirement; the full
`artifact.parse_status` extraction-outcome taxonomy (`DATABASE.md` §3: `parsed` / `partial` /
`empty` / `unsupported_format` / `failed`, not just a binary success/fail); per-file ingestion
status UI.

**Claude Code responsibilities:** Upload UI, ingestion status UI.

**Codex responsibilities:** Parsers, the containment boundary implementation, a containment test
harness (D-022: a test worker that deliberately attempts to read unrelated evidence, open the
database, reach the network, or exceed a resource bound, and is verified to be denied/terminated),
hashing/duplicate detection, backend tests including adversarial file inputs.

**Human responsibilities:** Approve file size/count limits; approve the specific containment
mechanism (subprocess + restricted token/job object vs. WASM vs. another approach) — the required
*properties* are fixed by D-022, only the mechanism choice is open.

**Dependencies:** Sprint 2.

**Exit criteria:** PDF/DOCX/TXT/CSV upload works; malicious/oversized/malformed test files are
rejected safely, not crash the app, and are contained by the boundary decided in this sprint (not a
placeholder); the containment test harness demonstrates the worker cannot read unrelated evidence,
reach the database, reach the network, or exceed its resource bounds — containment is verified, not
assumed from process separation alone; duplicate files are flagged; every parse outcome maps to one
of the defined `parse_status` values, not just success/fail; no evidence content appears in logs.

---

## Sprint 5 — Local Model Runtime

**Objective:** Implement `ModelProvider` and `LlamaCppProvider` per `docs/MODEL_RUNTIME.md`.

**Deliverables:** `GenerationCapable`/`EmbeddingCapable` interfaces; `LlamaCppProvider`
implementation; manual model import (GGUF) flow; capability-specific health checks (a bounded real
generation for `GenerationCapable`, a finite-vector check for `EmbeddingCapable` — `DECISIONS.md`
D-023); the model-runtime server's own independent auth credential, generated and held by FastAPI
and never exposed to the frontend (`DECISIONS.md` D-017); start/stop lifecycle.

**Claude Code responsibilities:** Model management UI (import, start/stop, health status).

**Codex responsibilities:** `ModelProvider` capability interfaces and `LlamaCppProvider`
implementation, subprocess/lifecycle management, the model-server credential mechanism (D-017),
structured-output/grammar integration.

**Human responsibilities:** Approve which initial GGUF model(s) are used for development/testing.

**Dependencies:** Sprint 1.

**Exit criteria:** A locally imported GGUF model can be started, health-checked, used to generate
a structured JSON response to a test prompt, and stopped — fully offline after import.

---

## Sprint 6 — Hardware Detection and Model Recommendations

**Objective:** Implement hardware detection and the model catalog/recommendation logic.

**Deliverables:** Hardware detection module; model catalog manifest format (including the `roles`
field distinguishing generation/embedding entries — `DECISIONS.md` D-023) + initial curated catalog;
recommendation logic (feasible/recommended per model, budgeting the **combined** footprint when
separate generation and embedding models are configured to run simultaneously, with sequential
loading as a documented fallback when combined footprint doesn't fit); FAST/BALANCED/ACCURATE/CUSTOM
tier UI; model download with checksum verification.

**Claude Code responsibilities:** Model selection UI (tiers, recommendations, download progress).

**Codex responsibilities:** Hardware detection implementation, catalog schema + validation,
download + checksum verification logic.

**Human responsibilities:** Curate/approve the initial model catalog entries and their
requirement/tier assignments; approve the download source(s).

**Dependencies:** Sprint 5.

**Exit criteria:** App detects hardware on a real Windows machine; shows accurate
feasible/recommended model tiers; downloads and verifies a real model end to end.

---

## Sprint 7 — Artifact Classification

**Objective:** Classify uploaded artifacts into the fixed taxonomy from `docs/AI_PIPELINE.md` §4.

**Deliverables:** Classification pipeline stage (method — LLM-based vs. heuristic — decided and
implemented); classification shown/correctable in artifact review UI.

**Claude Code responsibilities:** Artifact review/classification-correction UI.

**Codex responsibilities:** Classification implementation and evaluation against a small
hand-labeled test set.

**Human responsibilities:** Approve the classification taxonomy if it needs to change; provide/
label a small test set.

**Dependencies:** Sprints 4, 5.

**Exit criteria:** Uploaded artifacts receive a classification; analyst can correct it; correction
is persisted and distinguishable from the AI-assigned value.

---

## Sprint 8 — Evidence-to-Control Mapping

**Objective:** Implement candidate control retrieval and LLM evaluation producing validated
Mapping Candidates, per `docs/AI_PIPELINE.md`.

**Deliverables:** Embedding-based + lexical retrieval, with embedding-index rebuilds triggered on
any embedding-configuration change (`DECISIONS.md` D-023); evaluation prompt template with
injection-resistance measures; structured output schema implementation; deterministic validation;
`analysis_run` (execution-progress status, embedding provenance, supersession field),
`analysis_run_section_result` (per-section outcome), `mapping_candidate` (+ join table), and
`analyst_decision` (revision-based ordering, `DECISIONS.md` D-024) migrations and persistence
(`DATABASE.md`); a **small hand-labeled evaluation sample** (a handful of artifacts with
known-correct mappings, not the full Sprint 14 harness) used to sanity-check retrieval and mapping
quality before the rest of the review/graph/gap features are built on top of this pipeline.

**Claude Code responsibilities:** Mapping review UI (list/detail, approve/reject/needs-review,
surfacing the Coverage Signals from `COMPLIANCE_MODEL.md` — not a computed sufficiency verdict).

**Codex responsibilities:** Retrieval implementation, LLM evaluation orchestration, schema
validation, per-section outcome persistence and run-status derivation (`DECISIONS.md` D-020),
provenance persistence, adversarial testing (prompt injection test fixtures), running the small
evaluation sample and reporting baseline retrieval/mapping quality.

**Human responsibilities:** Review mapping quality on real/sample evidence; approve retrieval
tuning (top-N, thresholds); provide or approve the small hand-labeled evaluation sample (a scoped-
down precursor to Sprint 14's full set, not a replacement for it).

**Dependencies:** Sprints 3, 4, 5, 7.

**Exit criteria:** Running analysis on a test artifact produces schema-valid Mapping Candidates
with correct provenance, each attributable to an `analysis_run` with full configuration identity
(`DECISIONS.md` D-013, D-023); a test case with some sections succeeding and some deliberately
failing produces an `analysis_run` correctly reporting `partially_succeeded`, not `succeeded`
(`DECISIONS.md` D-020); a crafted prompt-injection test document does not alter system behavior
beyond producing an (still-human-reviewed) mapping candidate; two concurrent Analyst Decision writes
on the same candidate are resolved deterministically via revision, not by wall-clock race
(`DECISIONS.md` D-024); the small evaluation sample shows retrieval/mapping quality is not obviously
broken (not a formal precision/recall gate — that's Sprint 14) before later sprints build on this
pipeline.

---

## Sprint 9 — Policy-to-Control Mapping

**Objective:** Extend Sprint 8's pipeline to policy-classified artifacts specifically, and
evaluate whether policy-specific handling (vs. generic artifact handling) is actually warranted.

**Deliverables:** Either confirmation that policy artifacts are adequately served by the generic
Sprint 8 pipeline (likely, per `DECISIONS.md` D-004), or a scoped extension if gaps are found.

**Claude Code responsibilities:** UI differentiation for policy-classified evidence in review
screens, if needed.

**Codex responsibilities:** Pipeline evaluation/tuning against policy documents specifically.

**Human responsibilities:** Judge mapping quality against real policy documents.

**Dependencies:** Sprint 8.

**Exit criteria:** Policy documents produce useful mapping candidates via the existing pipeline;
any gap found is documented and scoped, not silently patched with an undocumented special case.

---

## Sprint 10 — Analyst Review System

**Objective:** Full approve/reject/needs-review workflow with decision history.

**Deliverables:** `analyst_decision` table/migration; review UI with decision history visible;
bulk actions if approved by product owner (`USER_FLOWS.md` open question).

**Claude Code responsibilities:** Review UI and decision history display.

**Codex responsibilities:** Decision persistence/API, latest-status query logic.

**Human responsibilities:** Resolve the bulk-action and edit-vs-accept-only open questions from
`USER_FLOWS.md` §8.

**Dependencies:** Sprint 8.

**Exit criteria:** Analyst can approve/reject/flag every mapping candidate; full decision history
is preserved and viewable; rejected mappings remain visible, not deleted.

---

## Sprint 11 — Compliance Knowledge Graph

**Objective:** Graph exploration UI over the relational data per `docs/ARCHITECTURE.md` §10.

**Deliverables:** Graph query API assembling nodes/edges from existing tables; interactive graph
UI with filtering by relationship type/confidence/status/family.

**Claude Code responsibilities:** Graph UI (library selection, rendering, filtering,
interactions).

**Codex responsibilities:** Graph assembly query/API, performance testing at realistic scale.

**Human responsibilities:** Approve graph UI library choice; define acceptable performance targets
(node count, response time).

**Dependencies:** Sprint 10.

**Exit criteria:** Graph view renders real assessment data; filters work; performance is
acceptable at the target scale defined by the product owner.

---

## Sprint 12 — Gap Analysis

**Objective:** Implement the computed gap/finding logic and Coverage Signals from
`docs/COMPLIANCE_MODEL.md` §2 and `DECISIONS.md` D-006/D-019.

**Deliverables:** Coverage-signal query logic (Support present / Partial support present /
Confirmed conflict / References only / Review pending / Analysis incomplete — computed
independently and shown together, never collapsed into one verdict); gap view = controls without
"Support present," explicitly including partial-only controls (`DECISIONS.md` D-019 — this
corrected an earlier design that removed partial-only controls from the gap view entirely); gap
review UI.

**Claude Code responsibilities:** Gap review UI, showing multiple simultaneous signals per control
rather than a single status.

**Codex responsibilities:** Coverage-signal and gap computation query logic, tests covering edge
cases (partial baselines, rejected-only mappings, partial-only support, superseded-run mappings
still counting toward coverage with an "older analysis" flag per `DECISIONS.md` D-021, confirmed
conflicts coexisting with support).

**Human responsibilities:** Resolve the baseline/applicability open question from `USER_FLOWS.md`
§11.

**Dependencies:** Sprints 3, 10.

**Exit criteria:** Gap list correctly reflects controls without "Support present," distinguishing
"no mappings at all" from "mappings exist but are rejected" from "only partial/reference-only
support approved" from "analysis incomplete"; a control with both approved support and an approved
conflict shows both signals, not one overriding the other.

---

## Sprint 13 — Security Hardening

**Objective:** Close out remaining unresolved items from `docs/SECURITY.md` §6 and run a focused
adversarial review — including **hardening and adversarially verifying** the parser containment
boundary that was decided and built in Sprint 4 (`DECISIONS.md` D-010), not deciding or introducing
that boundary for the first time here.

**Deliverables:** Adversarial hardening of the existing Sprint 4 parser containment boundary
(fuzzing, malicious-file corpus, resource-limit tuning under load); decisions and implementation
for the remaining open items: at-rest DB encryption (yes/no), retention/deletion policy (including
WAL/journal/temp-file cleanup per `SECURITY.md` T-22), dependency scanning in CI. Full adversarial
test suite (malicious files, injection attempts, oversized inputs).

**Claude Code responsibilities:** Implement UI-facing consequences of security decisions (e.g.
delete confirmations, encryption passphrase UX if adopted).

**Codex responsibilities:** Lead adversarial review and hardening implementation
(`docs/SECURITY.md` explicitly names Codex as preferred for security review).

**Human responsibilities:** Decide all open security questions in `docs/SECURITY.md` §6 /
`docs/OPEN_QUESTIONS.md`.

**Dependencies:** Sprints 4, 5, 8 (needs real ingestion + AI pipeline to test against).

**Exit criteria:** All Sprint 0 open security questions resolved and implemented or explicitly
deferred with owner sign-off; adversarial test suite passes.

---

## Sprint 14 — AI Evaluation Harness

**Objective:** Build a repeatable way to measure mapping quality (precision/recall against a
hand-labeled set), since V0.1 success is not judged against a benchmark that doesn't yet exist
(`PRODUCT.md` §8). This formalizes and extends the small evaluation sample already introduced in
Sprint 8 — Sprint 8's sample is a sanity check taken early; this sprint builds the durable,
larger, regression-tracking harness on top of it.

**Deliverables:** Hand-labeled evaluation set (small, human-curated, expanding on Sprint 8's
sample); scoring harness; baseline metrics recorded.

**Claude Code responsibilities:** Tooling/UI to support labeling if needed.

**Codex responsibilities:** Scoring harness implementation, metric computation, regression
tracking across pipeline changes.

**Human responsibilities:** Create or approve the hand-labeled evaluation set; define acceptable
quality bar.

**Dependencies:** Sprint 8.

**Exit criteria:** Repeatable evaluation run produces precision/recall/other agreed metrics on the
labeled set; harness can be re-run after future pipeline changes to catch regressions.

---

## Sprint 15 — Reporting

**Objective:** Basic export of assessment state (approved mappings, gaps) for analyst use outside
the app.

**Deliverables:** Export format (TBD — likely a structured document, not a compliance
attestation); export UI.

**Claude Code responsibilities:** Export UI and formatting.

**Codex responsibilities:** Export data assembly, format implementation.

**Human responsibilities:** Define required report format/content; confirm the report cannot be
mistaken for an official compliance certification.

**Dependencies:** Sprints 10, 12.

**Exit criteria:** Analyst can export a report reflecting current assessment state, clearly
labeled as analyst work product, not an official determination.

---

## Sprint 16 — V0.1 Polish and Release

**Objective:** Stabilization, packaging, and release readiness.

**Deliverables:** Windows installer/package; final documentation pass (README, user-facing docs);
bug triage and fixes; performance pass.

**Claude Code responsibilities:** Frontend polish, packaging integration.

**Codex responsibilities:** Backend stabilization, packaging of the Python runtime, release
testing.

**Human responsibilities:** Final acceptance testing; go/no-go decision.

**Dependencies:** All prior sprints.

**Exit criteria:** Installable Windows build; full `PRODUCT.md` §8 success criteria demonstrably
met on a clean machine with no prior setup.

---

See `docs/SPRINT_1_BACKLOG.md` for the detailed Sprint 1 ticket breakdown. Later sprints'
backlogs should be created at the start of that sprint, not now.
