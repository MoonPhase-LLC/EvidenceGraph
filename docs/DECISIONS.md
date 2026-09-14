# EvidenceGraph — Architecture Decision Log

Lightweight ADR-style entries. Each decision: context, decision, consequences, status. Add new
entries at the bottom; do not renumber or delete past entries even if later superseded — mark them
Superseded and link forward instead.

---

## D-001: Accept the proposed stack (Tauri + React/TS + Python/FastAPI + SQLite + llama.cpp)

**Status:** Accepted

**Context:** Sprint 0 brief proposed a specific stack and asked for critical evaluation rather
than blind acceptance.

**Decision:** Accept the proposed stack as-is for V0.1, with the packaging risk of bundling a
Python runtime inside a Tauri app flagged as the top technical risk to validate early in Sprint 1
(a minimal "hello world" Tauri-spawns-FastAPI-and-talks-to-it slice should be one of the first
things built, before deeper feature work).

**Consequences:** Two-runtime desktop app (Rust/Tauri host + Python local service) adds packaging
complexity versus an all-JS or all-Rust app, but is justified by the maturity of Python's document
parsing / ML tooling ecosystem for this domain. See `ARCHITECTURE.md` §13.

---

## D-002: No dedicated graph database in V0.1

**Status:** Accepted

**Context:** The product is conceptually "a compliance knowledge graph," which could suggest a
graph database (Neo4j etc.).

**Decision:** Represent the graph relationally in SQLite; assemble graph structures at query time
for the frontend. Do not introduce a graph database, or any additional database engine, in V0.1.

**Consequences:** Simpler single-database architecture, consistent with `CLAUDE.md` engineering
rules (avoid unnecessary infrastructure). Revisit only if graph query complexity or scale
genuinely outgrows relational queries with evidence, not speculatively.

---

## D-003: Minimal V0.1 relationship-type enum

**Status:** Accepted

**Context:** The Sprint 0 brief listed nine candidate relationship types (SUPPORTS,
PARTIALLY_SUPPORTS, CONFLICTS_WITH, IMPLEMENTS, REFERENCES, SUPERSEDES, STALE, INSUFFICIENT,
REQUIRES_REVIEW) and asked for a critical, minimal recommendation rather than implementing all of
them.

**Decision:** V0.1 implements four: `SUPPORTS`, `PARTIALLY_SUPPORTS`, `REFERENCES`,
`CONFLICTS_WITH`. The other five are either review-status concepts (`REQUIRES_REVIEW`,
`INSUFFICIENT`), a time-based computed property (`STALE`), or belong to relationship scopes not
yet modeled (`IMPLEMENTS` for Policy Statement↔Control, `SUPERSEDES` for Artifact↔Artifact
versioning).

**Consequences:** Smaller enum reduces ambiguity for the LLM at evaluation time and avoids
conflating relationship type with review status or data-quality flags in the schema. See
`COMPLIANCE_MODEL.md` §3 for full reasoning.

---

## D-004: Policy is an Artifact classification value, not a separate entity, in V0.1

**Status:** Accepted

**Context:** The brief's relationship model lists Policy and Policy Statement as first-class
concepts with their own Control relationships.

**Decision:** Defer Policy and Policy Statement as separate entities. In V0.1, "policy" is simply
one value of `artifact.classification`; policies are mapped to controls the same way any other
artifact is — via Artifact Section → Control mappings.

**Consequences:** Keeps V0.1's extraction pipeline to one level (document → sections) instead of
two (document → statements → controls). A future sprint can add Policy Statement extraction as an
additional, optional refinement stage without breaking the existing Artifact Section model, since
policy statements would essentially be a finer-grained section type.

---

## D-005: "Approved Mapping" is a Mapping Candidate + latest Analyst Decision, not a separate table

**Status:** Accepted

**Context:** The brief's language ("mapping candidates," "approved mappings") could imply two
separate tables.

**Decision:** One `mapping_candidate` table plus an append-only `analyst_decision` table. A
mapping's current status is derived from its latest decision, not stored as a mutable status flag
overwritten in place, and not duplicated into a second "approved" table.

**Consequences:** Full decision history preserved (supports the audit-trail/provenance
requirement) without a data-duplication/sync problem between a "candidates" table and an
"approved" table. Query for "current status" requires a latest-row-per-group lookup — an accepted
cost given the auditability benefit. See `DATABASE.md` §2.

---

## D-006: Findings/Gaps are computed at query time, not persisted AI output

**Status:** Accepted

**Context:** The brief lists "findings/gaps" as a required concept alongside AI provenance.

**Decision:** Do not treat a gap as a stored, AI-authored assertion needing its own review
lifecycle. Compute "controls with no/weak evidence" from existing `control` + `mapping_candidate`
+ `analyst_decision` data at query time.

**Consequences:** Avoids a second parallel category of "thing the AI outputs that needs human
review" alongside Mapping Candidates, which would have doubled the provenance/review UX for
limited benefit in V0.1. If gap analysis logic becomes complex enough to warrant caching, that's a
performance optimization on top of this model, not a change to it.

---

## D-007: No authentication/authorization layer on the local API in V0.1

**Status:** Superseded by D-009

**Context:** The FastAPI local service exposes an HTTP API consumed by the frontend.

**Decision (superseded):** No auth token/session on the local API in V0.1. This was treated as
acceptable **only** because the service is strictly bound to `127.0.0.1` (see `SECURITY.md` T-09)
and the app is single-user.

**Why superseded:** An independent architectural review (Codex, 2026-09) correctly identified that
"bound to localhost" is a *network-reachability* control, not an *authentication* control. Any other
locally-running process/user on the same machine (malware, another app, a browser tab if the port
is ever guessed/reused) can still reach a `127.0.0.1`-bound port with no credential. Localhost
binding and authentication address different threats (A3 network-adjacent vs. A2 local
unprivileged process in `SECURITY.md` §2) and should not have been conflated. See D-009.

---

## D-008: Single active model at a time in V0.1

**Status:** Accepted, narrowed by D-011

**Note (added by D-011):** "Exactly one model process at a time" below is narrowed to "exactly one
active model process **per capability** (generation, embedding) at a time" — see D-011. This
matters because generation and embedding may legitimately be served by two different loaded
models, which was not distinguished when this decision was first made.

**Context:** Model runtime could theoretically support running multiple models concurrently.

**Decision:** V0.1 runs exactly one model process at a time; switching models stops the current
one before starting the next.

**Consequences:** Simpler resource management (no concurrent GPU/RAM contention to reason about);
matches the actual V0.1 workflow (one active model per assessment session). Concurrent multi-model
support is not precluded architecturally (the `ModelProvider` abstraction doesn't assume
singleton), just not built in V0.1.

---

## D-010: Parser containment boundary is decided and built in Sprint 4, not deferred to Sprint 13

**Status:** Accepted

**Context:** `SPRINTS.md` originally left the parser sandboxing *approach* as an open question
(`SECURITY.md` S-1) to be resolved during Sprint 13 ("Security Hardening"), while Sprint 4
("Evidence Ingestion") already ships real parsers that touch untrusted files. An independent
review correctly flagged that this sequencing means the app runs untrusted-file parsing for nine
sprints before its containment boundary is even decided, and Sprint 13 would then have to retrofit
a security boundary around code that already exists and already has callers — which is backwards.

**Decision:** The parser containment/security boundary (subprocess isolation vs. in-process with
hard resource limits vs. WASM sandbox — the actual mechanism is still the product owner's call, see
`OPEN_QUESTIONS.md` S-1) must be **decided and implemented as part of Sprint 4**, before parsers are
wired into the rest of the pipeline. Sprint 13 shifts to *hardening and adversarial verification* of
that existing boundary (fuzzing, malicious-file test corpus, resource-limit tuning under load) —
not introducing the boundary itself.

**Consequences:** Sprint 4 gets slightly larger (must include a containment decision + baseline
implementation, not just parsing logic). `SPRINTS.md` Sprint 4 and Sprint 13 are updated accordingly.
This does not mandate a specific mechanism (no new sandboxing infrastructure like a container
runtime is implied) — a size/time-limited subprocess-per-parse using standard library process
isolation is sufficient for V0.1 and keeps within the "avoid unnecessary infrastructure" engineering
rule.

---

## D-011: Generation and embedding are independent capabilities in the model runtime

**Status:** Accepted

**Context:** `MODEL_RUNTIME.md` originally described a single `ModelProvider` with both
`generate()` and `embed()` methods, implicitly served by one loaded model, and D-008 said "V0.1
runs exactly one model process at a time." A GGUF model good at structured JSON generation is not
necessarily a good (or even a valid) embedding model, and vice versa — conflating the two forces an
unnecessary compromise or an incorrect assumption that one model does both well.

**Decision:** Split the capability, not necessarily the process count requirement: `ModelProvider`
is factored into two capability interfaces, `GenerationCapable` (`generate`) and `EmbeddingCapable`
(`embed`), which a concrete provider may implement one or both of. V0.1 may still run a single
`LlamaCppProvider` process that implements both interfaces if a chosen model supports both
reasonably (simplest case), **or** run two independent `LlamaCppProvider` instances — one loaded
with a generation model, one with a dedicated embedding model — if quality requires it. D-008's
"single active model at a time" is narrowed to mean "single active model *per capability* at a
time," not "one model process total."

**Consequences:** `MODEL_RUNTIME.md` §1 and §8 updated. No change to V0.1's actual footprint
requirement (still local-only, still no concurrent multi-model *serving fleet*) — this only removes
an incorrect architectural assumption that generation and embedding must share one model/process.
Whether V0.1 ships one process or two is an implementation decision for Sprint 5/6, informed by
retrieval quality testing (see D-012 note on evaluation timing).

---

## D-012: AnalysisRun entity added to track analysis attempts and bundle analysis configuration

**Status:** Accepted

**Context:** The original model could not distinguish "this artifact was never analyzed" from
"analysis ran and legitimately found nothing" from "analysis failed" from "this artifact was
analyzed under an older framework/model version and should be considered superseded." It also had
no single place to record the *configuration* under which a batch of Mapping Candidates was
produced (model version, framework version, parser/chunker version, prompt version), which matters
for provenance and reproducibility.

**Decision:** Add an `AnalysisRun` entity: one row per analysis attempt against an artifact (or a
batch of artifacts analyzed together), with a status of `succeeded` / `succeeded_no_mappings` /
`failed` / `superseded`, and immutable configuration-identity fields (see D-013). Every
`Mapping Candidate` gets a required `analysis_run_id` FK instead of loosely-typed
`model_provider`/`model_identifier`/`model_version` columns duplicated per row.

**Consequences:** One additional table and migration (Sprint 8, alongside `mapping_candidate`
itself — no schema churn since `mapping_candidate` doesn't exist yet in any shipped migration).
Enables an "artifact analysis status" view (not analyzed / succeeded / no mappings found / failed /
stale-superseded) that the current model could not represent. See `COMPLIANCE_MODEL.md` §2 and
`DATABASE.md`.

---

## D-013: Provenance includes immutable analysis configuration identity

**Status:** Accepted

**Context:** `COMPLIANCE_MODEL.md` §5 originally listed `model_provider` + `model_identifier` +
`model_version` as the provenance fields. An independent review noted this is insufficient to
reproduce or audit *why* a given Mapping Candidate was produced: it doesn't capture the framework
version the controls were evaluated against, the parser/chunker version that produced the section
being cited, or the prompt template version — any of which changing could change the output even
with the same model.

**Decision:** `AnalysisRun` (D-012) carries: `model_provider`, `model_identifier`, `model_version`,
`model_digest` (checksum of the loaded weights file, reusing the manifest `sha256` from
`MODEL_RUNTIME.md` §6 where available), `framework_id` + `framework_version`, `parser_version`,
`chunker_version`, `prompt_template_version`, and relevant inference settings actually used
(e.g. temperature/sampling config if non-default). These fields are immutable once the run
completes — a config change requires a new `AnalysisRun`, never an edit to an old one.

**Consequences:** Slightly larger `analysis_run` table; no behavior change to the pipeline logic
itself, only to what's recorded. This is what makes "why did this mapping look different after we
upgraded the model" answerable later, which `PRODUCT.md` §4's traceability principle already
implicitly required.

---

## D-014: Coverage semantics exclude CONFLICTS_WITH and REFERENCES; CONFLICTS_WITH is per-section, not cross-artifact

**Status:** Accepted

**Context:** Two related issues in the original model. First, `DECISIONS.md` D-006 and
`COMPLIANCE_MODEL.md`'s gap definition ("no `approved` mapping, or only low-confidence /
`needs_review` mappings") did not explicitly exclude an *approved* `CONFLICTS_WITH` or `REFERENCES`
mapping from counting as coverage — as written, an analyst approving a `CONFLICTS_WITH` mapping
(confirming "yes, this evidence really does contradict the control") could be read as satisfying
evidence coverage for that control, which is backwards: a confirmed conflict is a finding, not
coverage. Second, `SECURITY.md` T-06's own example for `CONFLICTS_WITH` ("a policy says MFA is
required; an account export shows accounts without MFA") describes a comparison **between two
different artifacts**, but `AI_PIPELINE.md` §12 point 2 states evaluation is scoped per-artifact-
section and prompts are never concatenated across artifacts — meaning V0.1's pipeline, as designed,
cannot actually detect that kind of cross-artifact contradiction.

**Decision:** (a) "Coverage" for gap computation counts only mapping candidates whose latest
Analyst Decision is `approved` **and** whose `relationship_type` is `SUPPORTS` or
`PARTIALLY_SUPPORTS`. An approved `CONFLICTS_WITH` or `REFERENCES` mapping never counts toward
coverage, regardless of approval status. An approved `CONFLICTS_WITH` instead surfaces as its own
signal (a confirmed conflict) alongside, not instead of, the gap view. (b) `CONFLICTS_WITH` in V0.1
is scoped to **a single artifact section's content appearing to contradict a single candidate
control's requirement**, evaluated using the model's own knowledge of what the control requires
(e.g. a section describing "MFA is optional" against a control requiring MFA) — not a comparison
between two separate artifacts. True cross-artifact contradiction detection (e.g. policy vs.
technical export) is **not supported in V0.1** and is explicitly deferred; the misleading example
is removed from `SECURITY.md` T-06/T-07.

**Consequences:** `COMPLIANCE_MODEL.md` §3/§4, `DATABASE.md`, and `SECURITY.md` T-06 updated to
match. Prevents the product from silently promising cross-document reasoning capability the
Sprint 8 pipeline design does not deliver. Cross-artifact contradiction analysis is added to
`docs/OPEN_QUESTIONS.md` as a genuine future-scope question, not silently implemented or silently
dropped.

---

## D-015: Manual, analyst-created mappings are allowed alongside AI-generated ones

**Status:** Accepted

**Context:** The original model implicitly assumed every `Mapping Candidate` is AI-generated
(required `model_provider`/`model_identifier`/`model_version`/`retrieval_method` fields, no way to
represent "the analyst manually linked this section to this control because retrieval/the LLM
missed it"). This under-serves the human-in-the-loop principle in `PRODUCT.md` §4: if the AI can be
wrong by omission (not just by bad suggestion), the analyst needs a way to add what it missed.

**Decision:** `mapping_candidate` gains a `source` field: `ai` or `analyst_manual`. For
`analyst_manual` rows, `analysis_run_id` and the AI-specific provenance fields are nullable
(there's no model/run behind a manual mapping), `relationship_type` is chosen directly by the
analyst, and the row still goes through the same `analyst_decision` history (a manual mapping still
needs an explicit `approved` decision to count as coverage — it isn't auto-approved just because a
human typed it, keeping D-005's single audit-trail mechanism for both origins).

**Consequences:** One nullable-FK column and a `source` enum; no new table. Keeps "coverage" and
"decision history" logic uniform across AI and human-originated mappings rather than building a
second parallel path. See `COMPLIANCE_MODEL.md` §2, `DATABASE.md`.

---

## D-016: Control hierarchy — Family becomes optional to improve real cross-framework extensibility

**Status:** Accepted, partial

**Context:** `ARCHITECTURE.md` §8 and `COMPLIANCE_MODEL.md` state the Framework → Family → Control
→ Enhancement model must not be NIST-specific, but the model as originally specified *requires*
every Control to belong to a Family. This is true for NIST 800-53 (families like AC, AU, CM) but is
not universally true of other frameworks this product claims to keep itself open to — e.g. some
frameworks group controls differently, some have only two levels (framework → control), and
enhancement-like refinements are not always one level deep. Requiring a Family for every framework
would force a future framework's data into a shape that doesn't fit it, which is exactly the
"hardcode-the-architecture-around-NIST" failure mode `CLAUDE.md` and `ARCHITECTURE.md` §8 warn
against, just one level removed (grouping-required instead of identifier-format-required).

**Decision:** Make `control_family` optional: a `Control` may belong to a `Control Family` or
directly to a `Framework` (nullable `family_id`, with `framework_id` denormalized onto `Control`
for the ungrouped case). This is a **partial** fix — it does not attempt full generalization to
arbitrary-depth nested control groupings (e.g. a framework with sub-sub-groups), which would be a
larger schema change disproportionate to a V0.1 with exactly one framework implemented. Full
n-level-hierarchy generalization is deferred until a second framework's real data exposes the
actual shapes needed, per the "don't design for hypothetical future requirements" engineering rule
— it is recorded as an open question, not silently precluded or silently built.

**Consequences:** `family_id` becomes nullable in `DATABASE.md`; NIST 800-53 Rev. 5 (which does use
families) is unaffected. See `docs/OPEN_QUESTIONS.md` A-5 for the deferred full-generalization
question.

---

## D-009: Authenticated local IPC via a per-launch shared-secret token

**Status:** Accepted

**Context:** D-007 conflated "localhost-only binding" with "authenticated." A local unprivileged
process (threat A2 in `SECURITY.md`) can reach a `127.0.0.1`-bound port without needing network
access at all, so the service needs its own authentication independent of network binding.

**Decision:** The Tauri host generates a random per-launch shared secret at app start, passes it to
the FastAPI child process via an environment variable (never a CLI argument, which would be visible
in the process list/`ps` output), and passes the same value to the frontend via Tauri's IPC (not
over the local HTTP channel itself). Every request from the frontend to the local service must
include this secret as a bearer token / custom header. The service rejects any request missing or
mismatching the token before doing any other work. The secret is held only in memory for the
lifetime of the app session; it is never persisted to disk or logged. Localhost-only binding
(`SECURITY.md` T-09) remains a defense-in-depth measure but is no longer the sole justification for
skipping auth.

**Consequences:** Small addition to the S1-04 process-supervision ticket (secret generation and
propagation) and to the FastAPI skeleton (auth-check middleware) — not a new subsystem, no user
credential management, no login flow. Closes the gap where any other local process could otherwise
call the analysis API, trigger local model inference, or read assessment data merely by reaching
the port. `AGENT_INSTRUCTIONS.md` rule 13 is updated to reference this mechanism. See
`ARCHITECTURE.md` §4 and `SECURITY.md` T-09.

---

*(Further decisions should be appended below as they are made during Sprint 1+ implementation.)*
