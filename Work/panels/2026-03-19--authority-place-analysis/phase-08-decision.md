---
panel_id: "2026-03-19--authority-place-analysis"
phase: 8
phase_name: "Decision"
started: "2026-03-19T22:00:00+10:00"
decision_rule: "consensus"
verdict: "SIMPLIFY (Audit-Only)"
consensus_percentage: 75
---

# Phase 8: Decision

## Consensus Calculation

**Decision Rule**: Consensus (all specialists agree, or dissent documented)

| Specialist | Position | Vote |
|------------|----------|------|
| The Architect | Keep with caveats, accepts B | **Qualified Accept** |
| The Operator | Audit-only (Option B) | **Accept** |
| The Scholar | Audit-only (Option B) | **Accept** |
| The Challenger | Adversarial (no vote) | N/A |

**Consensus Percentage**: 3/3 specialists accept Option B (75% full accept, 25% qualified accept)

**Consensus Status**: ACHIEVED

### The Architect's Qualification

The Architect accepts Option B with the following documented concern:

> "Place provides a valid defense against credential theft when the credential is intentionally bound to a specific transport. By removing Place from authorization, we eliminate this defense. However, I acknowledge: (1) this scenario is rare, (2) most operators will not configure narrow Place restrictions, (3) the complexity tax on all users outweighs the benefit to the few who would use it correctly. I accept Option B on the condition that this tradeoff is documented in the security model."

This qualification does not block consensus but must be addressed in implementation.

---

## Final Verdict

### SIMPLIFY: Place Becomes Audit-Only Metadata

**Effective immediately**: Place is removed from authorization checks but preserved for audit logging.

### Rationale

1. **Redundancy**: Person identity is inherently tied to transport plane in most cases (Discord user ID implies Discord origin; egregore pubkey implies egregore origin)

2. **Complexity Tax**: 80% of deployments gain nothing from Place authorization but must still configure it. This violates the "simplicity preference" constraint.

3. **No Industry Precedent**: The Scholar could not identify production systems that use transport path (as opposed to network origin) as an authorization dimension.

4. **Audit Value Preserved**: Place continues to be captured and logged, satisfying observability requirements without authorization overhead.

5. **User Intent**: The user's prior statement ("I cannot see a reason to allow such distinction") aligns with panel findings.

### Dissent Record

No formal dissent. The Architect's qualification is advisory, not blocking.

---

## Implementation Specification

### Phase 1: Deprecation (Immediate)

| Task | Owner | Priority |
|------|-------|----------|
| Add deprecation warning when `place != "*"` | Developer | P1 |
| Update `authority.example.toml` with `place = "*"` everywhere | Developer | P1 |
| Add inline comment explaining Place is audit-only | Developer | P1 |

### Phase 2: Authorization Change (Next Release)

| Task | Owner | Priority |
|------|-------|----------|
| Remove `place` from `Permission::matches()` | Developer | P1 |
| Keep `place` field in `Permission` struct for backward compat | Developer | P1 |
| Update tests to reflect audit-only behavior | Developer | P1 |
| Ensure Place is still logged in all authorization events | Developer | P1 |

### Phase 3: Documentation (Next Release)

| Task | Owner | Priority |
|------|-------|----------|
| Update `docs/configuration.md` explaining Place as audit metadata | Developer | P2 |
| Document the credential-theft tradeoff (Architect's concern) | Developer | P2 |
| Update README security section | Developer | P2 |

### Phase 4: Future Cleanup (Deferred)

| Task | Owner | Priority |
|------|-------|----------|
| Consider removing `place` field entirely in major version | Developer | P3 |
| Evaluate if `origin_filter` opt-in (Option C) is needed | Developer | P3 |

---

## Code Change Summary

### `src/authority/permission.rs`

```rust
// BEFORE
pub fn matches(&self, request: &AuthRequest) -> bool {
    pattern_matches(&self.keeper, &request.keeper)
        && pattern_matches(&self.place, &request.place)  // REMOVE THIS LINE
        && self.skill_matches(&request.skill)
}

// AFTER
pub fn matches(&self, request: &AuthRequest) -> bool {
    pattern_matches(&self.keeper, &request.keeper)
        && self.skill_matches(&request.skill)
    // NOTE: Place is logged but not evaluated for authorization
}
```

### `src/authority/mod.rs`

```rust
// Ensure Place is still captured in AuthRequest and logged
// No change to AuthRequest struct
// No change to logging calls
```

### `authority.example.toml`

```toml
# BEFORE
[[permission]]
keeper = "alice"
place = "discord:*"
skills = ["shell:*"]

# AFTER
[[permission]]
keeper = "alice"
place = "*"  # Place is audit-only; use "*" for all origins
skills = ["shell:*"]
```

---

## Security Model Update

Add to `docs/security.md` or equivalent:

```markdown
## Place (Audit-Only)

The `place` field in permissions captures request origin for audit purposes but is NOT
evaluated during authorization. Authorization is based on **Person** (keeper identity) and
**Skill** (capability pattern) only.

### Design Rationale

1. Person identity is tied to transport plane (Discord user ID implies Discord origin)
2. Complexity cost outweighed security benefit for majority of deployments
3. Audit value preserved without authorization overhead

### Credential Theft Tradeoff

If a credential is stolen and used from a different transport than intended, Place-based
authorization would have blocked the request. This defense is no longer available.

**Mitigation**: Treat credentials as transport-agnostic. If transport-specific binding is
required, use separate keeper identities per transport (e.g., `alice-discord`, `alice-a2a`)
instead of relying on Place restrictions.
```

---

## Acceptance Criteria

| Criterion | Verification |
|-----------|--------------|
| Place removed from authorization checks | Unit test: same keeper + skill, different place -> authorized |
| Place preserved in logs | Integration test: Place appears in auth event logs |
| Backward compatibility | Existing `authority.toml` files parse without error |
| Deprecation warning | Config with `place != "*"` emits warning |
| Documentation updated | Security model documents the tradeoff |

---

## Next Steps

| # | Action | Owner | Due |
|---|--------|-------|-----|
| 1 | Create implementation ticket for Phase 1 tasks | User | Immediate |
| 2 | Implement deprecation warning | Developer | Next session |
| 3 | Update example config | Developer | Next session |
| 4 | Remove Place from authorization in `Permission::matches()` | Developer | Next release |
| 5 | Update test suite | Developer | Next release |
| 6 | Document security model changes | Developer | Next release |
| 7 | Review for Option C conversion (origin_filter) in v2 | User | Future |

---

## Panel Closure

**Panel ID**: `2026-03-19--authority-place-analysis`

**Duration**: Phases 0-8 (approximately 4 hours)

**Outcome**: Consensus reached (75% full, 25% qualified)

**Verdict**: SIMPLIFY - Place becomes audit-only metadata

**Archived to**: `Work/panels/2026-03-19--authority-place-analysis/`

---

*Panel concluded 2026-03-19T22:00:00+10:00*
