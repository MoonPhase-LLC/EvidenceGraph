# EvidenceGraph — Product Definition

Status: Sprint 0 draft. Subject to product-owner approval.

## 1. Problem

Cybersecurity compliance analysts (internal GRC teams, MSSPs, auditors, consultants) spend enormous
manual effort reading client evidence — policies, procedures, screenshots, exports, configuration
reports, spreadsheets, logs — and mentally mapping that evidence against framework controls (e.g.
NIST SP 800-53 Rev. 5). This work is:

- repetitive and time-consuming across large evidence sets
- inconsistent between analysts and across engagements
- difficult to trace after the fact ("why did we say this control was supported?")
- high-stakes: mistakes affect audit outcomes, certifications, and security posture claims
- frequently blocked by confidentiality concerns that make it hard to use cloud AI tools at all,
  because the evidence itself (network diagrams, account inventories, incident reports) is
  sensitive client data.

There is no widely available tool that helps analysts triage and pre-map evidence against controls
**without** requiring that evidence leave the analyst's machine or the client's environment.

## 2. Target Users

Primary:

- **Compliance analysts / GRC consultants** performing NIST 800-53-based assessments for clients
  or their own organization.

Secondary (not designed for in V0.1, but should not be architecturally precluded):

- Internal security teams preparing for an audit.
- MSSPs running assessments across many clients (multi-tenant, out of scope for V0.1 — see
  [[Non-Goals]]).
- Auditors reviewing analyst-produced mappings.

## 3. Value Proposition

EvidenceGraph lets an analyst upload a pile of unstructured client evidence and a supported
compliance framework, and get back a **reviewable, provenance-backed set of candidate
relationships** between that evidence and framework controls — without the evidence ever leaving
the analyst's computer, and without the tool ever pretending to make the compliance
determination itself.

The product's edge is not "AI reads your documents." It is:

1. **Local-first**: nothing evidence-bearing crosses the network by default.
2. **Structured, not conversational**: output is a reviewable graph/list of candidate mappings
   with confidence and reasoning, not a chat transcript.
3. **Provenance-preserving**: every suggestion can be traced back to the artifact, section, model,
   and reasoning that produced it.
4. **Human-authoritative**: the AI proposes; the analyst disposes. Nothing is marked "compliant"
   by the system.

## 4. Product Principles

These are non-negotiable design constraints, not aspirations. See `CLAUDE.md` and
`AGENT_INSTRUCTIONS.md` for enforcement.

1. **Local First** — evidence processing, embeddings, retrieval, and inference must be capable of
   running fully offline after setup.
2. **Privacy First** — no silent transmission of evidence, extracted text, embeddings, prompts
   containing evidence, policies, or assessment data to external services. No cloud AI in V0.1.
   No automatic cloud fallback, ever.
3. **Human in the Loop** — AI produces suggestions with confidence scores and reasoning. AI never
   issues a final compliance determination. Only a human analyst decision can mark something
   approved/rejected.
4. **Evidence First** — the core product loop is Evidence → Analysis → Relationships → Review →
   Knowledge Graph, not a chatbot.
5. **Traceability** — every AI output must be attributable to its source artifact, section, model,
   confidence, and reasoning, and must carry a review status.
6. **Model Independence** — the compliance engine talks to a `ModelProvider` abstraction, not to a
   specific LLM runtime.

## 5. V0.1 Features (In Scope)

- Local hardware detection (CPU/RAM/GPU/VRAM/storage, best-effort).
- Local LLM selection, download/import (GGUF), and lifecycle management (start/stop/health).
- Assessment creation against a single framework: NIST SP 800-53 Rev. 5.
- Evidence upload: PDF, DOCX, TXT, CSV.
- Local evidence parsing, hashing, and duplicate detection.
- Artifact classification (what kind of document is this).
- Candidate control retrieval (narrowing search space before LLM evaluation).
- AI-assisted evidence-to-control mapping suggestions with structured output, confidence, and
  reasoning summary.
- AI-assisted policy-to-control mapping suggestions.
- Analyst review workflow: approve / reject / mark for further review.
- Evidence/control/policy relationship visualization (graph exploration).
- Basic gap identification (controls with no supporting evidence, low-confidence-only coverage).
- Model and provenance metadata retained on every AI-generated mapping.

## 6. Non-Goals (Explicitly Out of Scope for V0.1)

- Additional frameworks (SOC 2, CMMC, ISO 27001, NIST 800-171, CIS Controls) — architecture must
  not preclude them, but none are implemented.
- Cloud inference / OpenAI / Anthropic / Gemini APIs.
- Billing, subscriptions, organization accounts, multi-tenant SaaS, multi-user collaboration.
- Cloud evidence collection (AWS, Azure, Google Workspace, SharePoint).
- Continuous compliance monitoring, automated remediation.
- Autonomous compliance determinations of any kind.
- Chatbot / general conversational AI functionality.

## 7. Future Possibilities (Not Committed, Architecture Should Not Preclude)

- Additional frameworks via the generic Framework → Family → Control → Enhancement model.
- Multi-user / team review workflows.
- Optional, explicitly-opt-in cloud AI assist for non-sensitive tasks.
- Evidence staleness tracking and scheduled re-review.
- Report generation for audit packages.
- Conversational Q&A over an assessment's knowledge graph (post-V0.1).

## 8. Success Criteria for V0.1

Success is a working, locally-run desktop app that a single analyst can use end to end, on
Windows, with no Internet access after setup, to:

1. Detect hardware and select/run a local LLM.
2. Create an assessment against NIST 800-53 Rev. 5.
3. Upload PDF/DOCX/TXT/CSV evidence and have it parsed and classified.
4. Receive AI-suggested evidence-to-control mappings with confidence and reasoning.
5. Approve, reject, or flag each suggestion, with the decision and its author preserved.
6. Visualize the resulting relationships as a graph.
7. See a basic list of controls with no or weak evidence coverage.

V0.1 is not judged on mapping accuracy against a gold-standard benchmark (no such benchmark exists
yet — see `docs/OPEN_QUESTIONS.md`), but on whether the full loop functions, stays local, and
preserves provenance and human authority at every step.

See also: [[ARCHITECTURE]], [[COMPLIANCE_MODEL]], [[AI_PIPELINE]], [[SECURITY]].
