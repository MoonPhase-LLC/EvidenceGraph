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
bound to localhost; a minimal frontend screen that calls one localhost API endpoint and displays
the result; basic project tooling (linting, formatting, test runners) wired up for both the
Rust/TS and Python sides; CI running those checks on PRs.

**Claude Code responsibilities:** Tauri shell, React/TS frontend skeleton, repo-wide tooling
config, CI workflow.

**Codex responsibilities:** FastAPI service skeleton, Python project tooling (linting/typing/test
runner), backend unit test scaffolding.

**Human responsibilities:** Approve the validated Tauri↔FastAPI process-supervision approach
before Sprint 2 builds on it; resolve the port-allocation open question.

**Dependencies:** Sprint 0 docs.

**Exit criteria:** App launches on Windows; frontend successfully round-trips a call to the local
service; CI passes on a clean checkout; the Python-runtime-bundling risk flagged in `DECISIONS.md`
D-001 is either resolved or has a concrete mitigation plan. See `docs/SPRINT_1_BACKLOG.md` for
tickets.

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
the size/decompression/path-traversal mitigations from `SECURITY.md`; per-file ingestion status
UI.

**Claude Code responsibilities:** Upload UI, ingestion status UI.

**Codex responsibilities:** Parsers, security hardening (size limits, sandboxing approach per the
open question in `SECURITY.md`), hashing/duplicate detection, backend tests including adversarial
file inputs.

**Human responsibilities:** Approve file size/count limits; approve the parser-sandboxing
approach.

**Dependencies:** Sprint 2.

**Exit criteria:** PDF/DOCX/TXT/CSV upload works; malicious/oversized/malformed test files are
rejected safely, not crash the app; duplicate files are flagged; no evidence content appears in
logs.

---

## Sprint 5 — Local Model Runtime

**Objective:** Implement `ModelProvider` and `LlamaCppProvider` per `docs/MODEL_RUNTIME.md`.

**Deliverables:** `ModelProvider` interface; `LlamaCppProvider` implementation; manual model
import (GGUF) flow; health check; start/stop lifecycle.

**Claude Code responsibilities:** Model management UI (import, start/stop, health status).

**Codex responsibilities:** `ModelProvider` interface and `LlamaCppProvider` implementation,
subprocess/lifecycle management, structured-output/grammar integration.

**Human responsibilities:** Approve which initial GGUF model(s) are used for development/testing.

**Dependencies:** Sprint 1.

**Exit criteria:** A locally imported GGUF model can be started, health-checked, used to generate
a structured JSON response to a test prompt, and stopped — fully offline after import.

---

## Sprint 6 — Hardware Detection and Model Recommendations

**Objective:** Implement hardware detection and the model catalog/recommendation logic.

**Deliverables:** Hardware detection module; model catalog manifest format + initial curated
catalog; recommendation logic (feasible/recommended per model); FAST/BALANCED/ACCURATE/CUSTOM tier
UI; model download with checksum verification.

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

**Deliverables:** Embedding-based + lexical retrieval; evaluation prompt template with
injection-resistance measures; structured output schema implementation; deterministic validation;
`mapping_candidate` (+ join table) migration and persistence.

**Claude Code responsibilities:** Mapping review UI (list/detail, approve/reject/needs-review).

**Codex responsibilities:** Retrieval implementation, LLM evaluation orchestration, schema
validation, provenance persistence, adversarial testing (prompt injection test fixtures).

**Human responsibilities:** Review mapping quality on real/sample evidence; approve retrieval
tuning (top-N, thresholds).

**Dependencies:** Sprints 3, 4, 5, 7.

**Exit criteria:** Running analysis on a test artifact produces schema-valid Mapping Candidates
with correct provenance; a crafted prompt-injection test document does not alter system behavior
beyond producing an (still-human-reviewed) mapping candidate.

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

**Objective:** Implement the computed gap/finding logic from `docs/COMPLIANCE_MODEL.md` §2 and
`DECISIONS.md` D-006.

**Deliverables:** Gap query logic (no/weak evidence per control); gap review UI.

**Claude Code responsibilities:** Gap review UI.

**Codex responsibilities:** Gap computation query logic, tests covering edge cases (partial
baselines, rejected-only mappings, etc.).

**Human responsibilities:** Resolve the baseline/applicability open question from `USER_FLOWS.md`
§11.

**Dependencies:** Sprints 3, 10.

**Exit criteria:** Gap list correctly reflects controls with no approved evidence, distinguishing
"no mappings at all" from "mappings exist but are rejected/low-confidence."

---

## Sprint 13 — Security Hardening

**Objective:** Close out unresolved items from `docs/SECURITY.md` §6 and run a focused adversarial
review.

**Deliverables:** Decisions and implementation for: parser sandboxing approach, at-rest DB
encryption (yes/no), retention/deletion policy, dependency scanning in CI. Adversarial test suite
(malicious files, injection attempts, oversized inputs).

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
(`PRODUCT.md` §8).

**Deliverables:** Hand-labeled evaluation set (small, human-curated); scoring harness; baseline
metrics recorded.

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
