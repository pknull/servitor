---
panel_id: "2026-03-18--thallus-deployability-review"
phase: 8
phase_name: "Decision"
started: "2026-03-18T19:15:00+10:00"
completed: "2026-03-18T19:20:00+10:00"
---

# Phase 8: Decision

## Panel Decision: Thallus Deployability Review

**Consensus: 80% (Strong)** | **Decision Rule: Consensus**

---

## Summary

The Thallus project (egregore + servitor) remains deployable and suitable for lightweight hardware after recent security additions. Binary sizes (16-18 MB), memory footprint (~20-50 MB idle), and pre-built ARM64 release artifacts confirm Raspberry Pi 4 compatibility. Documentation is comprehensive but has minor drift. Security additions are largely proportional to the threat model.

---

## Key Findings

| Area | Status | Notes |
|------|--------|-------|
| Binary Size | ✅ Acceptable | 16 MB egregore, 18 MB servitor |
| Memory Footprint | ✅ Acceptable | 20-50 MB idle, fits 4GB RAM |
| ARM64 Support | ✅ Pre-built | Both ship aarch64-linux releases |
| Documentation | ⚠️ Minor drift | CLAUDE.md missing 3 modules, 1 wrong location |
| Security Measures | ✅ Proportional | Some consolidation opportunities |
| Code Complexity | ⚠️ Watch | servitor/main.rs at 1,655 lines |

---

## Dissent (20%)

**The Challenger**: No evidence of actual deployment failures or user complaints was presented. This review addresses hypothetical concerns, not proven problems.

---

## Next Steps

| Action | Priority | Effort |
|--------|----------|--------|
| Fix CLAUDE.md inaccuracies | High | 15 min |
| Add release profile to servitor Cargo.toml | High | 1 min |
| Consolidate sensitive patterns into shared module | Medium | 30 min |
| Document Memory/ directory structure | Medium | 10 min |
| Consider extracting servitor daemon loop | Low | 2-4 hours |

---

## Confidence

- **Relevance**: 0.9
- **Completeness**: 0.85
- **Confidence**: 0.82 (High)
