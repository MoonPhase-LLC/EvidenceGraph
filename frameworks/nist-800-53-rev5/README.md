# NIST SP 800-53 Rev. 5 — Framework Data

This directory is a placeholder. It intentionally contains **no control data yet**.

## Why empty

Per project instructions, official framework data must not be fabricated. Populating this
directory requires an authoritative source for NIST SP 800-53 Rev. 5 control text, which is a
dedicated, explicitly-assigned task (`docs/SPRINTS.md` Sprint 3), not something invented during
documentation/bootstrap work.

## Expected format (to be finalized in Sprint 3)

The framework engine (`docs/ARCHITECTURE.md` §8) expects framework data to express the generic
Framework → Family → Control → Enhancement hierarchy described in `docs/COMPLIANCE_MODEL.md`.
Anticipated shape, subject to change when Sprint 3 finalizes the actual loader:

```
frameworks/nist-800-53-rev5/
├── framework.json          # id, name, version, source_reference
├── families/
│   ├── AC.json              # family identifier, name, and its controls
│   ├── AU.json
│   └── ...
```

Each family file would contain its controls and their enhancements, with official text kept
verbatim and distinguishable from any product-generated "expected evidence" guidance (which, per
`COMPLIANCE_MODEL.md` §7, must never be presented as official NIST language).

## Do not

- Do not hand-author plausible-looking control text here as a placeholder.
- Do not hardcode application logic against assumptions about this directory's exact contents
  until Sprint 3 defines the real format — `ARCHITECTURE.md` §8's genericity rule still applies.

See `docs/SPRINTS.md` Sprint 3 and `docs/OPEN_QUESTIONS.md` C-3 for the source/licensing question
that must be resolved before this directory is populated.
