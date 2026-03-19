---
panel_id: "2026-03-18--servitor-daemon-refactor"
phase: 4
phase_name: "Cross-Examination"
started: "2026-03-18T19:45:00+10:00"
completed: "2026-03-18T19:50:00+10:00"
---

# Phase 4: Cross-Examination

## The Challenger's Challenges

### Challenge 1: "Show me the bugs"

**The Challenger**: The 1,655-line file has 129 passing tests. Where's the evidence this causes problems?

**The Systems Designer**: The concern is future maintainability, not current bugs:

- Code review difficulty: Reviewing a monolith hides bugs
- Parallel work: Any change to main.rs creates merge conflicts
- Testability: Binary crate code can't have unit tests for daemon logic

**The Extraction Surgeon**: Valid point. I found one actual bug: duplicate `start_task()` at lines 665 and 670. This was hidden in the large file. Extraction would have made it obvious.

**Verdict**: Bug found during review validates that monolith hides issues.

---

### Challenge 2: "Why not defer?"

**The Challenger**: If no features are blocked, why spend 2-4 hours now?

**The Quality Auditor**: Three reasons:

1. Previous panel identified this as technical debt - accumulating further
2. The project just shipped security/observability features - good time to clean
3. The longer we wait, the more logic accretes in main.rs

**The Systems Designer**: Also, servitor is actively developed. Every PR touching main.rs is a merge conflict risk. Better to refactor before the next feature sprint.

**Verdict**: Timing is favorable post-security-hardening, before next feature work.

---

### Challenge 3: "Extraction adds complexity"

**The Challenger**: 1 file → 10+ files. That's more mental overhead.

**The Extraction Surgeon**: Counter-evidence:

- Existing modules (`agent/`, `task/`, `authority/`) already have 3-5 files each
- Developers already navigate 15 modules - 3 more doesn't change the cognitive load model
- SMALLER files are easier to understand than 1,655-line monsters

**The Quality Auditor**: The codebase already uses modular patterns. This extraction *aligns* with existing structure rather than adding a new paradigm.

**Verdict**: Aligns with existing patterns; cognitive load argument doesn't hold.

---

## Convergence Points

1. **ALL AGREE**: `execute_assigned_task()` needs context struct (11 params is too many)
2. **ALL AGREE**: Extract helpers to EXISTING modules where possible
3. **ALL AGREE**: Incremental extraction with tests between each step
4. **DISPUTE**: Whether to create `protocol/` or extend `task/`

## Remaining Disagreement

| Issue | The Systems Designer | Quality Auditor | Extraction Surgeon |
|-------|---------------------|-----------------|-------------------|
| `protocol/` module | Create new | Extend `task/` | Either works |
| RuntimeStats location | `runtime/` | `metrics.rs` | `runtime/` (cohesive with profile) |
| Extract daemon loop | Yes, eventually | Yes | Keep in main.rs longer |
