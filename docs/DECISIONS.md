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

**Status:** Accepted, amended by D-024

**Note (added by D-024):** "Latest decision" below was originally timestamp-only (`decided_at`).
A second-round review correctly noted equal/ambiguous timestamps and clock changes make
timestamp-only ordering non-deterministic. D-024 replaces "latest by `decided_at`" with a
monotonically increasing per-candidate revision number as the authoritative ordering; `decided_at`
remains as descriptive audit metadata only.

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

**Status:** Accepted, amended by D-022

**Note (added by D-022):** A second-round review correctly flagged that this entry's original
consequences text ("a size/time-limited subprocess-per-parse ... is sufficient") equates process
isolation with exploit containment, which is false — an ordinary subprocess with inherited
filesystem/network/credential access does not stop exploited parser code from reading other
evidence, the database, or model credentials, or reaching the network. D-022 replaces "a subprocess
is sufficient" with a specific set of required permission restrictions the containment mechanism
must provide; a subprocess is a plausible *mechanism* for meeting them, not a substitute for them.

**Context:** `SPRINTS.md` originally left the parser sandboxing *approach* as an open question
(`SECURITY.md` S-1) to be resolved during Sprint 13 ("Security Hardening"), while Sprint 4
("Evidence Ingestion") already ships real parsers that touch untrusted files. An independent
review correctly flagged that this sequencing means the app runs untrusted-file parsing for nine
sprints before its containment boundary is even decided, and Sprint 13 would then have to retrofit
a security boundary around code that already exists and already has callers — which is backwards.

**Decision:** The parser containment/security boundary must satisfy D-022's required permission
and resource restrictions. The actual mechanism remains the product owner's call (see
`OPEN_QUESTIONS.md` S-1) and must be **decided and implemented as part of Sprint 4**, before parsers are
wired into the rest of the pipeline. Sprint 13 shifts to *hardening and adversarial verification* of
that existing boundary (fuzzing, malicious-file test corpus, resource-limit tuning under load) —
not introducing the boundary itself.

**Consequences:** Sprint 4 gets slightly larger (must include a containment decision + baseline
implementation, not just parsing logic). `SPRINTS.md` Sprint 4 and Sprint 13 are updated accordingly.
This does not mandate a specific mechanism (no new sandboxing infrastructure like a container
runtime is implied). Any chosen mechanism must satisfy and demonstrate D-022's required permission
and resource restrictions; ordinary subprocess isolation with size/time limits alone is not
sufficient containment for V0.1.

---

## D-011: Generation and embedding are independent capabilities in the model runtime

**Status:** Accepted, amended by D-023

**Note (added by D-023):** Splitting the interface (below) without also splitting the *dependent
contracts* left several gaps: a shared health check that only exercises generation, hardware
recommendations that don't budget for two simultaneously loaded models, and provenance that
records only one model identity. D-023 closes these.

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
retrieval quality testing (see `AI_PIPELINE.md` §5 and `SPRINTS.md` Sprint 8).

---

## D-012: AnalysisRun entity added to track analysis attempts and bundle analysis configuration

**Status:** Accepted, amended by D-015, D-020, D-021 and D-025

**Note (added by D-020/D-021):** The original `status` enum (`succeeded` / `succeeded_no_mappings`
/ `failed` / `superseded`) conflated three different axes: execution progress (did the run finish,
partially finish, or crash), output volume (did it produce zero mappings), and supersession (has a
later run replaced this one). A second-round review correctly noted this lets a run with many
failed sections and one trivially-successful section report as blanket "succeeded" or
"succeeded_no_mappings," silently converting unexamined evidence into an apparent absence-of-
evidence conclusion. D-020 replaces `status` with a pure execution-progress enum plus per-section
outcome records; D-021 removes `superseded` from `status` entirely and represents supersession as
its own explicit relationship, separate from execution outcome. See both below.

**Context:** The original model could not distinguish "this artifact was never analyzed" from
"analysis ran and legitimately found nothing" from "analysis failed" from "this artifact was
analyzed under an older framework/model version and should be considered superseded." It also had
no single place to record the *configuration* under which a batch of Mapping Candidates was
produced (model version, framework version, parser/chunker version, prompt version), which matters
for provenance and reproducibility.

**Decision:** Add an `AnalysisRun` entity: one row per analysis attempt against one artifact
(batch operations create independent runs, clarified by D-025), with persisted lifecycle state
defined by D-020/D-025, a separate supersession relationship (D-021), and immutable configuration
identity (D-013/D-023). `Mapping Candidate.analysis_run_id` is required for AI-sourced mappings and
null for `analyst_manual` mappings (D-015). AI provenance lives on the referenced run rather than
duplicating `model_provider`/`model_identifier`/`model_version` on each mapping.

**Consequences:** One additional table and migration (Sprint 8, alongside `mapping_candidate`
itself — no schema churn since `mapping_candidate` doesn't exist yet in any shipped migration).
Enables an artifact analysis view that distinguishes lifecycle state, output count, and
supersession instead of combining them into one status. See `COMPLIANCE_MODEL.md` §2 and
`DATABASE.md` for the current D-020/D-021/D-025 contract.

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

**Status:** Accepted, amended by D-019

**Note (added by D-019):** This entry correctly excluded `CONFLICTS_WITH`/`REFERENCES` from
coverage, but the follow-on "Evidence Sufficiency" concept added afterward (in the same review
round) reintroduced a similar problem one level up: it declared a control "sufficiently covered"
automatically from one approved `SUPPORTS` mapping, with `PARTIALLY_SUPPORTS`-only treated as fully
resolved rather than remaining visible as a gap. A second-round review correctly identified this as
an unreviewed automatic judgment inconsistent with this project's human-authority principle
(`PRODUCT.md` principle 3, `COMPLIANCE_MODEL.md` §4). D-019 removes the automatic "sufficiency"
label entirely and replaces it with independent, factual coverage-state signals, keeping
partial-only coverage in the unresolved/gap view rather than closing it out.

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

**Decision (coverage amended by D-019):** (a) Only an approved `SUPPORTS` mapping establishes the
"Support present" signal and removes a control from the coverage-gap view. Approved
`PARTIALLY_SUPPORTS` mappings establish a separate partial-support signal and remain in that view
when no approved `SUPPORTS` mapping exists. Approved `CONFLICTS_WITH` and `REFERENCES` mappings
never establish support. A confirmed conflict remains visible alongside support when both exist;
none of these signals is an automatic sufficiency judgment. (b) `CONFLICTS_WITH` in V0.1
is scoped to **a single artifact section's content appearing to contradict a single candidate
control's requirement**, evaluated using the official requirements supplied from the assessment's
loaded framework data, including framework version and enhancement/parent text where applicable
(`AI_PIPELINE.md` §6–§8), never model memory or invented requirements
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

**Status:** Accepted, amended by D-018

**Note (added by D-018):** A bearer token authenticates a *caller* to a server; it does not
authenticate the *server* to the caller. As originally written, this decision did not close the
port-allocation race already flagged as an open question (`OPEN_QUESTIONS.md` A-1): if another
process occupied the selected loopback port before or instead of the real FastAPI service, Tauri
could hand that impostor the frontend's bearer token and, subsequently, evidence. D-018 adds a
fail-closed startup handshake that verifies the actual child process's identity before any token or
evidence is sent to the endpoint. The token mechanism below is still required; D-018 is what makes
it safe to rely on.

**Context:** D-007 conflated "localhost-only binding" with "authenticated." A local unprivileged
process (threat A2 in `SECURITY.md`) can reach a `127.0.0.1`-bound port without needing network
access at all, so the service needs its own authentication independent of network binding.

**Decision (delivery amended by D-025):** After service-identity verification, the Tauri host
generates a random per-launch session secret, sends it to the child over private inherited
pipes/handles, waits for authentication readiness, then exposes it to the frontend via Tauri IPC.
Startup credential delivery never uses CLI arguments or HTTP. Subsequent requests from the
frontend to the local service must
include this secret as a bearer token / custom header. The service rejects any request missing or
mismatching the token before doing any other work; the bounded startup challenge defined in D-025
is the sole pre-authentication protocol exception for FastAPI. `GET /health` requires the installed
session credential: missing or invalid tokens return 401. Missing credential configuration must
never disable authentication; `/health` returns 401 even if a token is supplied in that state.
The secret is held only in memory for the
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

## D-017: The local model-runtime server has its own independent authentication credential

**Status:** Accepted

**Context:** `MODEL_RUNTIME.md` §10 previously claimed that binding the llama.cpp server to
`127.0.0.1` was sufficient because "it has exactly one intended caller" (the FastAPI service). A
second-round review correctly identified this as the same mistake D-009 fixed for the frontend↔
FastAPI boundary, recurring one hop downstream: "intended caller" is a design assumption, not an
enforcement mechanism. Any other local process can still open a TCP connection to a
`127.0.0.1`-bound port and issue inference requests directly, bypassing FastAPI's own auth entirely
and reaching the model runtime — and, transitively, whatever evidence-derived content the pipeline
sends it — without ever presenting the D-009 token (which authenticates callers of FastAPI, not
callers of llama.cpp).

**Decision:** The model-runtime server requires its own independently generated credential,
distinct from the D-009 frontend↔FastAPI token:

- FastAPI (not Tauri, not the frontend) generates a fresh random credential each time it starts a
  model-runtime instance and holds it entirely within the backend process boundary — it is never
  sent to the frontend and never leaves the machine's local process space.
- Every inference and administrative request to the model-runtime server must present this
  credential; requests without it are rejected before any processing occurs. A basic liveness-only
  health endpoint may be explicitly exempted if the runtime supports one, but inference and any
  endpoint that reveals loaded-model state or accepts input must require the credential.
- The credential is never written to logs (`SECURITY.md` T-10) and never appears in a CLI
  argument (visible via the OS process list) — pass it the same way as the D-009 token, via an
  environment variable scoped to the child process, or an equivalent inherited-handle mechanism.
- If llama.cpp's server mode cannot itself enforce per-request header authentication, FastAPI must
  front it with a thin authenticating wrapper/reverse-proxy in the same process boundary, such that
  no request reaches llama.cpp without having first passed the credential check — an unauthenticated
  path directly to llama.cpp's port must not exist as a side channel around the wrapper.
- Loopback-only binding (`MODEL_RUNTIME.md` §10) remains required as defense-in-depth, but is no
  longer described as sufficient on its own.

**Consequences:** Small addition to Sprint 5 (`ModelProvider`/`LlamaCppProvider` implementation):
generate and check a second credential, scoped to the backend only. Closes the gap where any local
process could otherwise drive the model runtime directly — triggering inference, consuming
resources, or (if the runtime ever gains a way to read back arbitrary context) observing
evidence-derived prompt content — without ever touching FastAPI's own auth. See `MODEL_RUNTIME.md`
§10, `SECURITY.md` T-23.

---

## D-018: Fail-closed service-identity verification before sending credentials or evidence

**Status:** Accepted, amended by D-025 and D-026

**Note (added by D-026):** Identity verification proves the peer at startup only. Later
credential-bearing requests must be sent over that same verified connection and never over a
redialed one; see D-026.

**Context:** D-009 authenticates *callers* of the local FastAPI service. It does not authenticate
the *service* to the frontend. `ARCHITECTURE.md` §4 and `OPEN_QUESTIONS.md` A-1 already flagged
port allocation as an open, security-adjacent decision; a second-round review connected that open
question to a concrete risk: if the chosen local port is occupied by an unrelated or malicious
process (a race between "pick a free port" and "bind it"), or a fake service is listening on it,
Tauri could send that process the frontend's bearer token, and subsequently route evidence to it —
with the frontend having no way to tell the difference from the legitimate service.

**Decision:** Startup requires a fail-closed handshake that verifies the receiving process's
identity before any credential or evidence is sent to the HTTP endpoint:

1. Tauri launches the exact bundled service executable it shipped (not a path resolved by PATH
   lookup or any mechanism an unrelated program could intercept), and supplies a one-time startup
   secret through private pipes/handles the child inherits at spawn, supporting both directions
   and accessible only to the parent and intended child (D-025) — this startup
   secret is separate from, and a precursor to, the D-009 session token.
2. The child binds a loopback port using OS-assigned port allocation (bind to port 0 and let the OS
   choose and atomically reserve an available port), avoiding the separate "find a free port, then
   bind it later" race entirely.
3. The child reports the port it actually bound back to Tauri over the same private channel — never
   by, e.g., writing it somewhere another process could also observe or race to claim.
4. Tauri issues a fresh challenge over the now-known HTTP endpoint and verifies the response is
   correctly computed from the startup secret from step 1 — proving the process answering on that
   port is the one Tauri spawned — without ever transmitting the startup secret itself over that
   HTTP channel.
5. Once verification succeeds, Tauri generates the session token and delivers it through the
   private channel, waits for the child's authentication-ready acknowledgement, then exposes the
   verified endpoint/token to the frontend. Environment variables are not a bidirectional channel
   and cannot deliver a newly generated token to a running child (D-025). If the child exits,
   fails to bind, fails to respond within a
   bounded timeout, or fails challenge verification, startup fails closed: no endpoint or token is
   exposed to the frontend, and Tauri does not fall back to connecting to whatever else may be
   listening on any port.
6. Restarting the child (including a user-triggered model/service restart) generates a new startup
   secret and a new session token; a token issued to a prior child instance is not honored by a
   newly spawned one.

**Consequences:** Extends S1-04's process-supervision ticket with the handshake logic (steps 1–4)
and S1-09's packaged-build spike should exercise it, not just the plain health check. No new
external dependency — this is local process/IPC logic. Directly closes the scenario where an
occupied port or an impersonating process could obtain the frontend's credential or receive
evidence intended for the real service. See `ARCHITECTURE.md` §4, `SPRINT_1_BACKLOG.md` S1-04.

---

## D-019: Coverage is reported as independent factual signals, not an automatic "sufficiency" judgment

**Status:** Accepted

**Context:** The "Evidence Sufficiency" concept introduced in the prior review round computed a
control as "sufficiently covered" from a single approved `SUPPORTS` mapping, and treated
`PARTIALLY_SUPPORTS`-only coverage as fully resolved ("partially covered," removed from the gap
list). A second-round review correctly identified this as an automatic judgment the product
principles explicitly forbid the system from making on its own (`PRODUCT.md` principle 3,
`COMPLIANCE_MODEL.md` §4: only a human analyst decision is authoritative) — approving one mapping
confirms that *one relationship*, not that the *whole control* is adequately addressed, and
collapsing partial evidence into "resolved" hides genuinely incomplete coverage from the gap view
the product's core value proposition depends on.

**Decision:** Remove "Evidence Sufficiency" as a computed judgment. Replace it with independent,
simultaneously-possible factual signals per control, none of which is itself a sufficiency
determination:

| Signal | Meaning |
|---|---|
| Support present | At least one current, approved `SUPPORTS` mapping exists. |
| Partial support present | At least one current, approved `PARTIALLY_SUPPORTS` mapping exists. |
| Confirmed conflict | At least one current, approved `CONFLICTS_WITH` mapping exists. |
| References only | At least one current, approved `REFERENCES` mapping exists, with no Support present, Partial support present, or Confirmed conflict signal. |
| Review pending | At least one relevant Mapping Candidate is still `needs_review` (no Analyst Decision has been made, or it was reopened for review). |
| Analysis incomplete | Analysis relevant to this control failed, is still pending, or only partially completed (`DECISIONS.md` D-020) — this signal means "don't trust an apparent absence of evidence yet," not "no evidence." |

These are not mutually exclusive: a control can show both "Support present" and "Confirmed
conflict" at once, and the UI must show both rather than collapsing them into one verdict. The
**coverage-gap view** (`COMPLIANCE_MODEL.md` §2 "Finding / Gap") is a control **without** "Support
present" — this includes controls with only "Partial support present," which stays visible as
unresolved, not removed from the gap view the way D-014's original wording (approved
`PARTIALLY_SUPPORTS` = coverage) allowed. If a future feature needs an actual, holistic
"this control is adequately addressed" determination, it must be an explicit, rationale-carrying
human decision with its own history (like an Analyst Decision), never a computed default.

**Consequences:** `COMPLIANCE_MODEL.md`'s "Evidence Sufficiency" section is replaced with this
signal table. Gap computation (`DECISIONS.md` D-006/D-014) changes: partial-only controls remain in
the gap/coverage-gap view rather than being reported as resolved. `OPEN_QUESTIONS.md` U-7 is
narrowed accordingly (see that entry).

---

## D-020: AnalysisRun gains explicit execution-progress states and persisted per-section outcomes

**Status:** Accepted

**Context:** D-012's original `status` enum (`succeeded` / `succeeded_no_mappings` / `failed` /
`superseded`) let a run with many failed section evaluations and one trivially successful one
report as "succeeded" or "succeeded_no_mappings" — indistinguishable from a run that genuinely
examined everything and found nothing. That silently converts unexamined evidence into an apparent
absence-of-evidence conclusion, which is exactly the kind of overconfident automatic claim this
project's principles reject.

**Decision:** `AnalysisRun.status` becomes a pure execution-progress enum: `queued` → `running` →
one of `succeeded` / `partially_succeeded` / `failed` / `cancelled` / `interrupted`.
`succeeded_no_mappings` is removed as a status value — "zero mappings produced" is now a fact about
*output*, entirely orthogonal to execution status (a `succeeded` run can produce zero mappings; a
`partially_succeeded` run can also produce zero mappings, and the two must not be confused). A new
`analysis_run_section_result` record is persisted per (run, artifact section) with: the section's
evaluation outcome (`no_candidates_retrieved` / `evaluated_no_mappings` / `evaluated_with_mappings`
/ `failed`), a sanitized failure category where applicable (no evidence content, per `SECURITY.md`
T-10), attempt count, and timestamps. **Amended by D-025:** rows start as `pending` for the entire
intended section set before execution, with `running` also represented. Run lifecycle is persisted
by the coordinator, not inferred solely from results. `succeeded` requires every intended section
to reach a successful terminal outcome. A non-cancellation/non-interruption stop is
`partially_succeeded` with some successful sections and some failed/unfinished, or `failed` with
none successful. Explicit cancellation is `cancelled`; a running run found after restart becomes
`interrupted`. Lifecycle stops take precedence over result aggregation and preserve all section
outcomes. Sanitized run-level stop reasons distinguish startup failure from queued work.

**Consequences:** One additional table (`analysis_run_section_result`) and a revised
`analysis_run.status` enum (Sprint 8 migration, alongside `analysis_run` and `mapping_candidate`
themselves — no existing migration is altered). `AI_PIPELINE.md` §11's failure-handling table is
rewritten around per-section outcomes rather than a single all-or-nothing run verdict. This is also
what "Analysis incomplete" in D-019's signal table is grounded in.

---

## D-021: Supersession is a relationship between runs, not an execution-outcome value; old approvals are retained, not silently revoked

**Status:** Accepted

**Context:** D-012 originally used `superseded` as one value of `AnalysisRun.status`, which
conflated "this run's *execution* outcome" with "a later run has replaced this one for current-
coverage purposes" — two different facts that can vary independently (a successful run can later be
superseded; a failed rerun should not retroactively make an earlier successful run's results
disappear). It also left unspecified whether older approved mappings still count toward current
coverage after a rerun, and whether a failed rerun could inadvertently erase a previously working
analysis.

**Decision:**
- `AnalysisRun` permanently retains its own execution outcome (D-020) — it is never rewritten to
  `superseded` or anything else after completion.
- Supersession is a separate, explicit relationship: `AnalysisRun.superseded_by_analysis_run_id`
  (nullable, self-referencing), set only when a later run is intended to replace an earlier one for
  current-coverage purposes. Creating a new `AnalysisRun` does **not** automatically set this on the
  most recent prior run — it is set deliberately (e.g. once the new run reaches a `succeeded` or
  `partially_succeeded` outcome), not merely because a rerun was *started*.
- A **failed or cancelled** replacement run does not supersede anything: the previous run's results
  remain the current basis for coverage, untouched. The default V0.1 policy for a
  **partially-succeeded** replacement is that it also does **not** automatically supersede the
  prior complete run — an explicit analyst or system action is required to treat a partial rerun as
  the new basis for coverage, rather than silently downgrading previously-complete results.
- Mapping Candidates and their Analyst Decisions from a superseded run are **not deleted or
  hidden**, and their approvals are **not automatically transferred** to any new candidate a rerun
  produces — a new candidate always starts `needs_review` regardless of what was approved on an
  earlier run's analogous suggestion. Current coverage (`DECISIONS.md` D-019) continues to count an
  older approved mapping even if its originating run has since been superseded, but the review UI
  must visibly flag such a mapping as based on a superseded analysis run ("older analysis"),
  pending the analyst either re-confirming, replacing, or withdrawing that decision — coverage is
  not silently revoked out from under an assessment just because a newer run exists.

**Consequences:** `analysis_run.superseded_by_analysis_run_id` added to the schema
(`DATABASE.md`). Coverage/gap queries join through current Analyst Decisions as before (D-005/D-019)
and are unaffected by which run produced the underlying candidate, except for the added "older
analysis" UI flag. Prevents two failure modes: (a) a failed rerun silently erasing previously-good
coverage, and (b) duplicate/regenerated candidates being treated as independent additional evidence
rather than a re-assessment of the same relationship.

---

## D-022: Parser containment is defined by required permission properties, not by process count

**Status:** Accepted

**Context:** D-010 correctly moved the *timing* of the parser containment decision to Sprint 4, but
its consequences text said "a size/time-limited subprocess-per-parse ... is sufficient." A
second-round review correctly identified that a plain subprocess with inherited filesystem/network/
credential access does not stop code exploited via a malicious PDF/DOCX/CSV from reading other
evidence, opening the database, exfiltrating over the network, or reading the D-009/D-017
credentials — process separation alone bounds *availability* (a hung/crashed parser doesn't take
down the whole service) but not *confidentiality/integrity* under active exploitation, which is the
threat T-01/T-02/T-03 actually describe.

**Decision:** The parser containment boundary decided in Sprint 4 (per D-010's timing) must provide,
regardless of the specific mechanism chosen:

- Read access limited to the specific input file being parsed (and any explicitly-provided
  reference data, e.g. a shared decompression-limit config) — not the rest of the evidence store.
- Write access limited to a dedicated scratch location, not the application's data directory,
  database file, or other evidence.
- No access to the assessment database, other artifacts, or any authentication credential
  (D-009/D-017) — these must not be inherited into the parser worker's environment/handles at all.
- No network access.
- Enforced bounds on memory, CPU time, wall-clock time, expanded-archive size (T-02/T-03), and
  output size.
- No unintended inherited handles, environment variables, or secrets beyond what parsing strictly
  requires.
- The trusted service (FastAPI) validates the worker's bounded output before persisting any of it —
  the worker's output is still untrusted data, not a trusted result merely because it came from a
  contained process.

The specific Windows isolation mechanism (e.g. a restricted job object plus a low-integrity/
low-privilege token, a dedicated unprivileged service account, or another mechanism providing the
same properties) is still the product owner's call (`OPEN_QUESTIONS.md` S-1) and must be selected
and demonstrated to provide these properties *before* real evidence is connected to it, not assumed
from "it's a subprocess."

**Consequences:** Sprint 4's exit criteria gain a concrete containment test requirement: a test
worker that deliberately attempts to read unrelated evidence, open the database, make a network
call, or exceed a resource bound must be observably denied or terminated — tests must verify
containment, not merely that legitimate parsing still succeeds. This is a larger Sprint 4 scope
than "wrap the parser in a subprocess," proportional to the actual threat.

---

## D-023: Generation/embedding split is completed end-to-end — health checks, hardware budgeting, and provenance

**Status:** Accepted

**Context:** D-011 split the `ModelProvider` interface into `GenerationCapable`/`EmbeddingCapable`,
but left several dependent contracts pointed at the old single-model assumption: `health_check()`
was still specified as "capable of serving a minimal generation request" even for an embedding-only
provider; hardware recommendation logic (`MODEL_RUNTIME.md` §5) did not account for both
capabilities' models being resident in memory at once; and `AnalysisRun` provenance
(`DECISIONS.md` D-013) recorded only one model identity, with no place for the embedding model's own
identity or index configuration.

**Decision:**
- `GenerationCapable.health_check()` performs a minimal, bounded real generation call and confirms
  a well-formed response; `EmbeddingCapable.health_check()` embeds a fixed short string and confirms
  the returned vector is finite (no NaN/Inf) and matches the expected dimensionality for that model
  — an embedding-only provider must never be health-checked via a generation call it doesn't
  support.
- Model catalog entries (`MODEL_RUNTIME.md` §3) declare a `roles` field (`["generation"]`,
  `["embedding"]`, or both), and capability-specific fields apply only to the relevant role(s) (e.g.
  input-token limits/tokenizer identity for both roles, generation context/output budgeting, and
  embedding dimensionality, normalization and a separate similarity metric; see D-025).
- Hardware feasibility (`MODEL_RUNTIME.md` §5) must budget the **combined** memory/VRAM footprint
  when a generation model and a separate embedding model are configured to run simultaneously; if
  the combined footprint doesn't fit the detected hardware, the app supports **sequential loading**
  (load the embedding model during indexing/retrieval phases, swap to the generation model for
  evaluation phases) as a documented, supported path — never a silent degradation or an
  unexplained crash.
- `AnalysisRun` provenance (D-013) records the embedding model's identity separately from the
  generation model's: `embedding_model_identifier`, `embedding_model_version`,
  `embedding_model_digest`, plus `embedding_dimensions`, `embedding_normalization`,
  `embedding_similarity_metric`, and an `embedding_config_version` (digest covering model +
  preprocessing + normalization + metric).
- Whenever the embedding configuration changes, any previously computed embedding index is treated
  as **incompatible** and must be rebuilt (or retrieval is clearly marked degraded until it is) —
  vectors produced under different embedding configurations must never be compared to each other.

**Consequences:** `MODEL_RUNTIME.md` §3/§5/§9, `DATABASE.md`'s `analysis_run` fields, and
`SPRINTS.md` Sprints 5/6/8 are updated. This is what makes an embedding-only provider actually
health-checkable, keeps hardware recommendations honest about running two models, and prevents a
silent retrieval-quality bug from comparing embeddings produced under two different
model/preprocessing configurations.

---

## D-024: Deterministic, revision-based analyst decision ordering, plus explicit referential-integrity invariants

**Status:** Accepted

**Context:** D-005 established append-only Analyst Decisions with "current status = latest
`decided_at`," and the schema otherwise relied on individual foreign keys without stating several
cross-table invariants explicitly. A second-round review correctly noted: (a) wall-clock timestamps
can collide or move backward (system clock changes, coarse timestamp resolution under concurrent
writes), making "latest by `decided_at`" ambiguous; (b) `decided_by` was left as an unspecified
"local user identity" without stating its (lack of) assurance; (c) several referential-integrity
rules that matter for correctness were implied but never stated as explicit requirements, alongside
a genuinely disconnected join-table in the ER diagram and an invalid-JSON example.

**Decision:**
- Every `Analyst Decision` write is assigned a monotonically increasing, per-`mapping_candidate_id`
  `revision` integer inside the same transaction as the insert, with a uniqueness constraint on
  (`mapping_candidate_id`, `revision`). "Current status" is determined by highest `revision`, never
  by `decided_at`. `decided_at` is retained as descriptive audit metadata (when it happened), not as
  the ordering key.
- Writing a new decision requires the caller to state the revision it expects to be extending
  (optimistic concurrency); a write against a stale expected-revision is rejected, so two
  concurrent decision submissions on the same candidate cannot silently clobber each other.
- `decided_by` is a stable local analyst identifier, explicitly documented as a **local, unverified
  identity** in V0.1 (single-user, no authentication of "who is at the keyboard") — its assurance
  level must not be overstated in any UI or export copy (e.g. never presented as a verified
  signature).
- The following referential-integrity invariants are explicit requirements (enforced by database
  constraints where SQLite practically allows, otherwise centralized in transactional write paths
  and covered by tests — not left as an unstated assumption):
  1. Every `artifact_section_id` cited via `mapping_candidate_section` belongs to the same
     `artifact_id` as the `mapping_candidate` doing the citing.
  2. When `source = 'ai'`, the `mapping_candidate.analysis_run_id`'s `artifact_id` matches the
     `mapping_candidate`'s own `artifact_id`.
  3. `mapping_candidate.control_id` belongs to the assessment's framework (`COMPLIANCE_MODEL.md`
     §8 item 2, restated here as a concrete constraint requirement).
  4. `mapping_candidate.enhancement_id`, when set, belongs to `mapping_candidate.control_id`.
  5. `assessment_control_scope` entries belong to the assessment's own framework.
  6. (`mapping_candidate_id`, `artifact_section_id`) pairs in `mapping_candidate_section` are
     unique (no duplicate citation of the same section by the same candidate).
- SQLite foreign keys (`PRAGMA foreign_keys = ON`) are enforced on every connection, not assumed
  enabled by default.
- The ER diagram's `MAPPING_CANDIDATE_SECTION` join table gains explicit relationship edges to both
  `MAPPING_CANDIDATE` and `ARTIFACT_SECTION` (it was previously described only in prose, not drawn
  as a relationship in the diagram itself). The Model Catalog JSON example's
  `"size_bytes": 4_500_000_000` (numeric underscore separators are not valid JSON) is corrected to a
  plain integer.
- Existing claims that individual foreign keys alone "make cross-assessment contamination a
  constraint violation" are narrowed: a direct FK (e.g. `artifact.assessment_id` →
  `assessment.id`) prevents an artifact from referencing a nonexistent/wrong assessment row, but
  does **not** by itself prevent a join-table row (like `mapping_candidate_section`) from citing a
  section belonging to a *different* artifact/assessment than the mapping candidate itself — that
  requires the explicit invariants above, not an assumption that individual FKs compose into full
  isolation.

**Consequences:** `analyst_decision` gains a `revision` column and its uniqueness constraint;
`DATABASE.md` §5 gains the explicit invariant list; the ER diagram and JSON example are corrected.
No change to the product-visible decision workflow — this is entirely a correctness/audit-integrity
fix underneath it.

---

## D-025: Complete startup IPC, run lifecycle, and Sprint 1 validation contracts

**Status:** Accepted. Amends D-009/D-018, D-020/D-024 and clarifies D-019/D-023.

**Context:** The correction review found that the startup sequence used environment variables as
bidirectional IPC after spawn, execution status considered only attempted sections, new run tables
lacked ownership/uniqueness rules, and Sprint 1 sign-off did not depend on its packaged-build spike.

**Decision:**
- Use private inherited pipes/handles for the full startup exchange. Environment variables may
  configure a process at spawn but cannot return its port or deliver a post-start session token.
  The child binds port 0, reports its endpoint privately, answers a fresh endpoint challenge, then
  receives the session token privately and acknowledges readiness. Only then may Tauri release
  the endpoint/token to the frontend. Define the challenge as a domain-separated HMAC-SHA-256 of a
  fresh nonce and bound endpoint using the startup secret; reject replay/mismatched responses.
  The challenge endpoint is the sole pre-authentication protocol exception: no evidence or session
  secret is accepted there. All failures, including readiness timeout, fail closed. This replaces
  D-009's environment-variable delivery for the supervised FastAPI startup flow.
- Persist coordinator-owned run lifecycle. Each run belongs to one artifact and atomically
  snapshots its nonempty intended section set as pending result rows. Membership and section
  content remain immutable for that run. Only completion of every intended section can yield
  success; partial completion, cancellation and interruption follow `COMPLIANCE_MODEL.md`'s
  rules. Store sanitized run-level stop reasons; preserve unattempted rows after stops.
- Enforce unique (run, section) rows and same-artifact section ownership. A superseding run must
  belong to the same artifact, have a permitted completed outcome, and neither reference itself
  nor create a cycle; validate the replacement chain inside the write transaction.
- S1-08 depends on S1-09 and records its actual clean-machine result. A failed spike remains a
  blocker with a mitigation plan and explicit owner go/no-go decision, never a claimed pass.
- Both model roles declare input limits. Embedding normalization and similarity metric are
  separate fields and part of embedding configuration identity. Authenticated loopback IPC is
  permitted during inference; external network calls remain prohibited.

**Required implementation acceptance cases:**
- S1-04/S1-09: exchange the bound port and post-start token over private handles; verify correct
  startup, wrong/replayed challenge, child exit, readiness timeout, and missing/wrong session
  credential. No endpoint/token reaches the frontend before acknowledgement. Test a fake endpoint
  explicitly rather than assuming port 0 selects a particular preoccupied port.
- Sprint 8: one successful section plus nine pending sections must never yield success; test zero
  mappings after full success, all failures, startup failure, cancellation after partial work,
  restart interruption, and queued work with no results. Verify intended rows survive each stop.
- Sprint 8: reject duplicate/cross-artifact section results, cross-artifact replacement,
  self-replacement and cycles; accept a valid same-artifact completed replacement. Retain the
  older approval behavior defined in D-021.
- Sprint 1: exit sign-off includes the packaged-build result; a dev-only pass is insufficient.

**Consequences:** Documentation and proposed schema only. No application code, new dependencies,
or migration is introduced. Earlier ADR history remains historical where explicitly superseded;
the amended subsystem docs and this contract govern implementation.

---

## D-026: Credential-bearing requests travel only over the identity-verified connection (no redial)

**Status:** Accepted. Amends D-018 and D-025 (S1-04 review round 2, finding 1).

**Context:** D-018/D-025 verify the service's identity once, at startup, by challenge/response over
the child's HTTP endpoint. That proves who answered *at that moment*. The first S1-04 implementation
then opened a **new** TCP connection for every later authenticated request (`GET /health`). If the
verified child died after startup, its loopback port was released, and a different process bound the
same port, the next request would have been delivered — bearer token included — to that replacement
listener, with nothing on the client able to tell it apart from the real service. Two mitigations
were tried and rejected because they leave the window open: (1) checking a liveness flag before
and/or after sending, and (2) a fresh challenge followed by a separate connection. In both, the
service can die between the check and the connection that carries the credential.

**Required property:** every credential-bearing request is bound to the *same* connection whose peer
was cryptographically verified. No liveness flag, health poll, or fresh challenge is relied on to
establish this; a check followed by an independent dial cannot close the race.

**Options considered:**
1. *Liveness flag checked around a per-request dial* — rejected (check-then-use window).
2. *Fresh challenge, then a separate connection* — rejected (same window).
3. *One retained, explicitly owned HTTP/1.1 connection, no reconnection (chosen).* The TCP
   connection opened for the D-025 challenge is kept and used for every later request. The peer
   process of an established TCP connection cannot change: if the child dies the connection is
   closed or reset, and a listener that binds the freed port is a different socket that this
   connection can never be redirected to.
4. *An authenticated transport (e.g. TLS pinned to a per-launch key).* Would also satisfy the
   property, but adds certificate/key generation and a TLS stack for a loopback-only service.
   Not chosen as larger than needed; it remains the fallback if the retained-connection approach
   ever proves insufficient (see limitations).

**Decision:** Option 3, implemented in `app/src-tauri/src/supervisor/pinned_http.rs`:
- `PinnedConnection` wraps `hyper::client::conn::http1` (`SendRequest` + its `Connection` driver
  task). That API is constructed from one already-connected I/O object and has no facility to dial;
  it does not retry or reconnect. The rest of the module never calls `connect` a second time, and
  `PinnedConnection::connect` is invoked exactly once per launch, inside the `VerifyingIdentity`
  step, before the challenge is sent.
- `reqwest` (used by the previous revision) was removed from the supervisor. A `reqwest::Client`
  dials on demand and may pool or re-establish connections; connection reuse there is an
  optimization, not a guarantee, so no pool setting could provide this property.
- Loss of the connection for any reason (child exit, reset, protocol error, server-initiated close)
  is a terminal event: `Ready` is revoked, `SupervisorHandle::ready_connection()` returns `None`,
  the child is torn down, the state becomes `Failed("authenticated_connection_lost")` (or
  `child_exited_unexpectedly` if the process exit is observed first), and in-flight and later health
  calls fail. The old bearer token is never sent to that port again; there is no reconnect path.
  The UI clears any previously received health result when state leaves `ready`.
- The service must not reap the idle connection. `service/.../supervised.py` sets uvicorn
  `timeout_keep_alive` to 7 days (uvicorn's default of 5 s closed the connection on a healthy child
  and caused a spurious fail-closed; observed during S1-04 round 2 verification).
- Requests share the one connection through a mutex (one in flight at a time). The current traffic
  (a startup challenge and `/health`) does not need more; revisit if high-volume requests are
  routed through it.

**Limitations (accepted, not hidden):**
- After 7 days of *complete* idleness the server would close the connection and the app fails closed
  until the service is restarted. Only a ~7 s idle period is exercised in tests.
- The guarantee rests on the OS attributing the accepted socket to the spawned child. If that
  socket handle were inherited by another process, that process could keep the connection open
  after the child died. Python sockets are non-inheritable by default (PEP 446) and the child is
  in a kill-on-close Job Object; neither was independently tested for this scenario.
- Loopback HTTP remains plaintext (unchanged from D-009): a local administrator or a packet-level
  attacker who can read or inject into an established loopback TCP stream is out of scope for this
  decision. Option 4 would address that.
- A request that exceeds its timeout (10 s) makes hyper close the connection (observed in
  `a_request_that_times_out_*`), which ends the session as a lost connection. This is fail-closed
  but means one unusually slow `/health` response terminates the session until restart.
- There is no automatic recovery: restarting the service means a new supervisor, child, startup
  secret and session token (D-018 step 6).
- Verified on Windows only.

**Consequences:** `Cargo.toml`: `reqwest` removed from this crate; `hyper` (`client`, `http1`),
`hyper-util` (`tokio`), `http-body-util` and `bytes` added — all already present in the dependency
tree via Tauri, no new crates in `Cargo.lock` — and tokio's `net` feature enabled explicitly (it was
previously enabled only through `reqwest`). `HttpClientBuildFailed` removed from `FailureReason`;
`AuthenticatedConnectionLost` added. Tests: see `supervisor/pinned_http.rs` and
`supervisor/mod.rs` (`child_death_*`, `lost_authenticated_connection_*`,
`pinned_connection_survives_idling_*`).

---

*(Further decisions should be appended below as they are made during Sprint 1+ implementation.)*
