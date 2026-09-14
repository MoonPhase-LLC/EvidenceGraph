# EvidenceGraph — Open Questions

Status: Sprint 0. These require human product-owner judgment and were deliberately **not**
answered by invented behavior during Sprint 0. Agents should not silently resolve these — surface
them again if blocked by one, and check here before assuming an answer.

Update this file as questions are resolved (move to `docs/DECISIONS.md` as an ADR, or note the
resolution inline and mark Resolved) or as new ones surface during implementation.

## Product

- P-1: Should V0.1 support NIST baseline selection (Low/Moderate/High) or a control subset at
  assessment creation, or is every control always in scope? (`USER_FLOWS.md` §4)
- P-2: Can assessments be cloned/templated in V0.1, or is each assessment created from scratch?
  (`USER_FLOWS.md` §4)
- P-3: What is the actual target list of "recommended" GGUF models for the initial catalog, and
  who curates/maintains it over time? (`MODEL_RUNTIME.md` §3, `SPRINTS.md` Sprint 6)

## Compliance

- C-1: Is a "policy" ever going to need to be a first-class entity distinct from a generic
  Artifact in V0.1, or does `DECISIONS.md` D-004's deferral hold for the full V0.1 scope? Revisit
  after real policy documents are tested in Sprint 9.
- C-2: Does gap analysis need to account for NIST baseline/applicability (tying back to P-1), or
  does V0.1 gap analysis naively assume every framework control is in scope? (`USER_FLOWS.md` §11,
  `SPRINTS.md` Sprint 12)
- C-3: What is the authoritative source for NIST SP 800-53 Rev. 5 control text to be loaded in
  Sprint 3, and are there licensing/attribution requirements to satisfy? (`SPRINTS.md` Sprint 3)
- C-4: Is cross-artifact contradiction analysis (e.g. "a policy claims MFA is required but a
  separate technical export shows accounts without MFA") a real V0.1-or-soon-after product
  requirement, given it's a genuinely valuable compliance use case? An architectural review found
  the Sprint 8 pipeline design (per-artifact-section evaluation, no cross-artifact prompt
  concatenation) does not support it, and `CONFLICTS_WITH` was narrowed to a single-section
  comparison against a control's stated requirement (`DECISIONS.md` D-014). If cross-artifact
  contradiction is wanted, it needs its own scoped design (a second retrieval/evaluation pass
  comparing artifacts pairwise or via a shared summary layer) — not something to retrofit silently
  into the existing per-section pipeline. (`COMPLIANCE_MODEL.md` §3, `AI_PIPELINE.md` §12)

## UX

- U-1: What is the exact first-launch privacy disclosure copy and flow, and is it skippable on
  return visits? (`USER_FLOWS.md` §1)
- U-2: How often should hardware re-detection run automatically, if at all, versus only on
  explicit user request? (`USER_FLOWS.md` §2)
- U-3: Can an analyst edit a Mapping Candidate's relationship type/confidence, or only
  approve/reject the AI's characterization as-is? (`USER_FLOWS.md` §8)
- U-4: Is bulk approve/reject required for V0.1, given potentially large numbers of candidates
  per assessment? (`USER_FLOWS.md` §8)
- U-5: What graph rendering library/approach should be used, and what are acceptable performance
  targets (node count, interaction latency)? (`USER_FLOWS.md` §10, `SPRINTS.md` Sprint 11)
- U-6: What deletion/retention semantics are expected for assessments and evidence — hard delete,
  soft delete, or a retention window — given the audit-trail requirements elsewhere in the
  product? (`USER_FLOWS.md` §5, `SECURITY.md` T-17)
- U-7: V0.1 no longer computes an automatic "evidence sufficiency" verdict at all — it reports
  independent factual Coverage Signals instead (`DECISIONS.md` D-019, `COMPLIANCE_MODEL.md`
  "Coverage Signals"), so there is no computed field left to override. The open question is now:
  should V0.1 (or a near-term follow-up) add an explicit, human-authored "this control is
  adequately addressed" decision — with its own rationale and append-only history, analogous to an
  Analyst Decision — or is showing the raw signals (support/partial/conflict/references/pending/
  incomplete) and leaving any holistic judgment entirely outside the tool sufficient for V0.1? Such
  a decision would need its own audit trail if added, so it's worth a deliberate product call rather
  than adding it opportunistically.

## Architecture

- A-1: **Resolved by D-018/D-025:** the child binds loopback port 0 and reports the bound port over
  private inherited pipes/handles. Startup verifies the endpoint before delivering the session
  token over that channel and releasing it to the frontend. S1-04/S1-09 validate the Windows
  implementation; fixed-vs-dynamic allocation and environment-variable IPC are no longer open.
- A-2: Where are models and the SQLite database stored on disk, and is either path
  user-configurable? (`USER_FLOWS.md` §3, §12)
- A-3: What is the packaging strategy for bundling the Python runtime inside the Tauri app for
  distribution (Sprint 16), and does the chosen approach affect earlier sprints' assumptions?
  Flagged as the top technical risk in `DECISIONS.md` D-001; a narrow validation spike is now
  scheduled in Sprint 1 (`SPRINT_1_BACKLOG.md` S1-09) rather than waiting until Sprint 16, but the
  final production packaging approach is still an open Sprint 16 decision.
- A-4: Should the local frontend↔service IPC transport eventually move from TCP-over-loopback to
  an OS-native local transport (named pipe on Windows, Unix domain socket elsewhere) for stronger
  isolation than a shared-secret token over HTTP, or is TCP-over-loopback plus the shared-secret
  token (`DECISIONS.md` D-009) sufficient for V0.1? The shared-secret approach was chosen as the
  minimal fix that closes the authentication gap without a bigger transport change; revisit if a
  future security review finds it insufficient.
- A-5: Should the Framework → Control hierarchy be generalized beyond the current "Family is
  optional, one level" fix (`DECISIONS.md` D-016) to support arbitrary-depth nested control
  groupings, once a second framework's real data is available to inform the actual shape needed?
  Deliberately not attempted in V0.1 with only one framework implemented — designing a general
  hierarchy against a single example risks guessing wrong.

## AI

- AI-1: What retrieval top-N and any confidence/similarity thresholds should Sprint 8 start with,
  and how will they be tuned? (`AI_PIPELINE.md` §5)
- AI-2: Should classification (`AI_PIPELINE.md` §4) be LLM-based, a lighter heuristic/rules
  classifier, or a small dedicated local classifier model? Affects Sprint 7 scope and possibly
  requires its own small model.
- AI-3: What is the required/target evaluation methodology and quality bar for Sprint 14's
  evaluation harness, given no existing benchmark? Who creates the hand-labeled evaluation set?

## Security

- S-1: Should file parsing run in a sandboxed subprocess or WASM sandbox rather than in-process,
  given the parser-exploit threat (T-01/T-02/T-03)? What's the acceptable performance/complexity
  tradeoff?
- S-2: What are the concrete maximum file size and file count limits for evidence upload? (T-04)
- S-3: Is at-rest encryption of the SQLite database required for V0.1 given the sensitivity of
  the data it may contain, or explicitly deferred with that risk accepted for V0.1? (T-12)
- S-4: What is the exact model checksum/manifest verification mechanism, and who maintains and
  hosts the pinned model catalog referenced in `MODEL_RUNTIME.md` §3/§6? (T-08)
- S-5: Is dependency/SCA (software composition analysis) scanning required in CI for V0.1? (T-13)

---

When a question is resolved, update the relevant doc(s) it's cross-referenced from, remove or
mark it Resolved here, and add an ADR to `docs/DECISIONS.md` if the resolution is architecturally
significant.
