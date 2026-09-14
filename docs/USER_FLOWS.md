# EvidenceGraph — User Flows

Status: Sprint 0 draft. Describes intended flows for V0.1 and flags unresolved UX questions —
see `docs/OPEN_QUESTIONS.md` for the authoritative open-question list. Do not implement UI
behavior for an unresolved question by guessing; ask.

## 1. First Launch

1. App starts, no assessment exists yet.
2. App performs hardware detection (see §2) and shows a summary screen.
3. App checks whether a local model is already installed/imported.
   - If none: route to Model Selection (§3).
   - If one exists: route to a landing/dashboard screen listing assessments (empty state).
4. App explains, on first launch only, the local-first / privacy posture (what is and is not sent
   anywhere) before any evidence can be uploaded.

Open questions: exact copy/UX of the privacy disclosure; whether first-launch is skippable for
returning users; whether telemetry (even fully local/opt-in) exists at all in V0.1.

## 2. Hardware Detection

1. App detects CPU, RAM, GPU (if present), VRAM (if determinable), OS, and available disk space.
2. Detection is best-effort — some values (e.g. VRAM on unsupported GPUs) may be "unknown."
3. Results feed the Model Selection screen's recommendations; they do not block progress if
   detection partially fails.
4. Detected hardware is stored locally to avoid re-detecting on every launch (re-detect on
   explicit user request or app update).

Open questions: how often to re-run detection automatically; whether users can override detected
values manually.

## 3. Model Selection

1. User is shown model tiers: FAST / BALANCED / ACCURATE / CUSTOM (see `MODEL_RUNTIME.md`), with
   which tiers are feasible given detected hardware, and which are not recommended.
2. User selects a model. App either:
   - Downloads the GGUF model (with integrity verification), or
   - Lets the user import an existing local GGUF file.
3. App starts the model runtime locally and performs a health check.
4. User is routed to the dashboard once a healthy model is confirmed running.

Open questions: where models are stored on disk and whether that path is user-configurable;
behavior when download is interrupted; whether multiple models can be installed simultaneously
and swapped without re-downloading; exact download source(s) and how their integrity/trust is
established.

## 4. Assessment Creation

1. User creates a new assessment: name, description, target framework (NIST SP 800-53 Rev. 5 is
   the only option in V0.1).
2. User optionally scopes the assessment to a control baseline/subset (open question — see below).
3. Assessment is created locally; app routes to the assessment's evidence upload screen.

Open questions: whether V0.1 supports selecting a NIST baseline (Low/Moderate/High) or control
subset at creation time, or whether all controls are always in scope; whether assessments can be
cloned/templated.

## 5. Evidence Upload

1. User uploads one or more files (PDF, DOCX, TXT, CSV) via drag-drop or file picker.
2. Each file is hashed on ingestion; exact-duplicate files (by hash) are flagged rather than
   silently re-processed.
3. Files undergo the security handling described in `SECURITY.md` (size limits, type
   verification, sandboxed parsing) before any content is extracted.
4. User sees per-file ingestion status: queued → parsing → one of `parsed` / `partial` / `empty` /
   `unsupported_format` / `failed` (`DATABASE.md` §3) — not just a binary parsed/failed, so a
   scanned-image PDF with no extractable text or a partially-recovered document is visible as its
   own distinct state, not lumped in with either success or failure.

Open questions: max file size and count limits for V0.1; whether folder-based bulk upload is
required; how near-duplicate (not byte-identical) evidence is surfaced, if at all, in V0.1.

## 6. Artifact Review

1. Once parsed, each artifact is shown with its detected type/classification (e.g. "policy
   document," "access review export," "configuration report") and extracted sections.
2. User can correct a misclassification before AI mapping runs (open question: is this required,
   optional, or post-hoc only).
3. User triggers AI analysis for one artifact or the whole assessment.

Open questions: is classification correction mandatory before mapping, or can mapping run on
AI-classified artifacts without review; how sections are presented (raw text, ToC-style outline).

## 7. AI Analysis

1. For each artifact (or selected artifacts), the pipeline in `AI_PIPELINE.md` runs: retrieval of
   candidate controls, LLM evaluation, structured output generation.
2. Progress is shown per-artifact; analysis runs entirely against the local model runtime.
3. Failures (model unavailable, parse error, low-confidence-only results) are surfaced, not
   hidden — see `AI_PIPELINE.md` §"AI Failure Handling."

## 8. Mapping Review

1. User sees candidate mappings grouped by artifact or by control, each with: control ID,
   relationship type, confidence, evidence section(s) referenced, reasoning summary.
2. For each candidate, user can: **Approve**, **Reject**, or **Mark for further review**.
3. Approving/rejecting records an analyst decision with timestamp and (future: user identity —
   V0.1 is single-user, so this may just be "the local user").
4. Rejected mappings remain visible (not deleted) for audit trail purposes.

Open questions: bulk approve/reject UX; whether an analyst can edit the relationship type or
confidence, or only accept/reject the AI's characterization; whether comments/notes can be
attached to a decision.

## 9. Control Review

1. User can browse the framework tree (Framework → Family → Control → Enhancement) and, per
   control, see all approved/pending/rejected mappings pointing at it across the assessment.
2. Control detail shows official NIST text (clearly labeled as official) separately from any
   product-generated "expected evidence" guidance (clearly labeled as such).

## 10. Graph Exploration

1. User can view assessment relationships as an interactive graph: artifacts, controls, policies,
   and the relationships between them (approved mappings by default; option to include
   pending/rejected).
2. User can filter by relationship type, confidence threshold, review status, or control family.

Open questions: graph rendering technology/library; performance expectations at scale (hundreds
vs. thousands of nodes); whether graph state (layout, filters) persists per assessment.

## 11. Gap Review

1. User sees a list of controls **without** an approved `SUPPORTS` mapping ("Support present" —
   `COMPLIANCE_MODEL.md` §2 "Finding / Gap", "Coverage Signals"). This is not a single verdict per
   control but a set of independent, simultaneously-possible signals: Partial support present,
   Confirmed conflict, References only, Review pending, Analysis incomplete. A control with an
   approved `PARTIALLY_SUPPORTS` mapping and nothing else **stays in this view** as unresolved — it
   is not treated as closed out just because some evidence exists (`DECISIONS.md` D-019 corrected an
   earlier design that removed partial-only controls from the gap view entirely).
2. A confirmed `CONFLICTS_WITH` mapping is surfaced as its own signal, and can coexist with
   "Support present" on the same control (e.g. one section supports a control while another
   conflicts with it) — the UI must show both, never collapse them into a single status.
3. "Analysis incomplete" means relevant analysis failed, is pending, or only partially completed
   (`DECISIONS.md` D-020) — the UI must not present this as "no evidence found," since that would be
   an unreviewed conclusion the system isn't entitled to draw yet.
4. A mapping contributing to a control's coverage that originated from a since-superseded analysis
   run is visibly flagged as based on an older analysis (`DECISIONS.md` D-021), prompting the
   analyst to re-confirm, replace, or withdraw it rather than silently trusting or silently losing
   it.

Open questions: whether gap analysis considers control baseline/applicability, or naively expects
every control in the framework to have evidence.

## 12. Settings / Model Management

1. User can view/change the active model, re-run hardware detection, manage downloaded models
   (list, delete, re-verify), and view basic app/version/storage info.
2. Any network-capable operation (model download) is clearly distinguished from fully offline
   operations in the UI.

Open questions: whether settings include any privacy/logging controls beyond what's fixed by
`SECURITY.md`; update mechanism UX (in scope for V0.1 at all?).

---

See `docs/OPEN_QUESTIONS.md` for the consolidated, categorized list of unresolved items from this
document.
