---
panel_id: "2026-03-19--authority-place-analysis"
phase: 7
phase_name: "Synthesis"
started: "2026-03-19T21:30:00+10:00"
---

# Phase 7: Synthesis

## Options Summary

### Option A: Keep Place (Status Quo)

**Description**: Place remains a required authorization dimension. All permissions must specify a place pattern.

**Proponents**: The Architect (partial)

**Tradeoffs**:

| Aspect | Assessment |
|--------|------------|
| Security benefit | Marginal. One valid scenario (credential theft + transport binding) |
| Configuration burden | High. All permissions require place, most will use `"*"` |
| Cognitive load | Moderate. Admins must understand Person AND Place distinctions |
| Audit value | Unchanged. Place logged regardless |
| Backward compatibility | Full. No migration required |

**When to choose**: Only if credential-theft-with-transport-binding is a realistic threat model.

---

### Option B: Simplify (Audit-Only)

**Description**: Place becomes optional audit metadata. Authorization checks Person + Skill only. Place recorded in logs but not evaluated for access decisions.

**Proponents**: The Operator, The Scholar

**Tradeoffs**:

| Aspect | Assessment |
|--------|------------|
| Security benefit | Equivalent to Option A for 80%+ of deployments |
| Configuration burden | Low. Place optional, defaults to `"*"` |
| Cognitive load | Low. Admins focus on Person + Skill |
| Audit value | Preserved. Place still logged |
| Backward compatibility | High. Existing configs work, place ignored for authz |

**Implementation**:

```rust
// Before (authorization)
fn matches(&self, request: &AuthRequest) -> bool {
    pattern_matches(&self.keeper, &request.keeper) &&
    pattern_matches(&self.place, &request.place) &&    // <-- Remove
    self.skill_matches(&request.skill)
}

// After (audit only)
fn matches(&self, request: &AuthRequest) -> bool {
    pattern_matches(&self.keeper, &request.keeper) &&
    self.skill_matches(&request.skill)
}
// Place still captured in request, logged, but not checked
```

**Migration path**:

1. Deprecation warning when `place != "*"` is configured
2. Document that place is audit-only
3. Remove from authorization checks
4. Keep place field in config for backward compatibility (ignored)

---

### Option C: Opt-In Origin Filter

**Description**: Replace `place` with `origin_filter` as an optional advanced field. Defaults to `"*"`. Operators who want transport-based restrictions explicitly opt-in.

**Proponents**: The Operator (alternative proposal)

**Tradeoffs**:

| Aspect | Assessment |
|--------|------------|
| Security benefit | Same as Option A for those who opt-in |
| Configuration burden | Minimal. Field optional, defaults permissive |
| Cognitive load | Low for most. Advanced feature for specialists |
| Audit value | Preserved. Origin always logged |
| Backward compatibility | Moderate. Field rename requires migration |

**Implementation**:

```toml
# Before
[[permission]]
keeper = "alice"
place = "discord:*"       # Required
skills = ["shell:*"]

# After
[[permission]]
keeper = "alice"
skills = ["shell:*"]
# origin_filter = "discord:*"  # Optional, defaults to "*"
```

**Migration path**:

1. Add `origin_filter` as optional field
2. Deprecate `place` field (map to `origin_filter`)
3. Default `origin_filter` to `"*"` when absent
4. Eventually remove `place` field

---

### Option D: Remove Place Entirely

**Description**: Eliminate Place from the authorization model. Person + Skill only.

**Proponents**: None explicitly, but logical extension of Option B

**Tradeoffs**:

| Aspect | Assessment |
|--------|------------|
| Security benefit | No loss for 80%+ of deployments |
| Configuration burden | Minimal. Schema simplified |
| Cognitive load | Lowest. Two dimensions instead of three |
| Audit value | Reduced. Would need separate logging mechanism |
| Backward compatibility | Low. Breaking change to config schema |

**Why not recommended**: Audit value is real. Removing Place entirely loses the ability to see request origin in logs. Options B and C preserve audit value.

---

## Option Comparison Matrix

| Criterion | A (Keep) | B (Audit-Only) | C (Opt-In Filter) | D (Remove) |
|-----------|----------|----------------|-------------------|------------|
| Security for most users | Equivalent | Equivalent | Equivalent | Equivalent |
| Security for power users | Best | Reduced | Best | Reduced |
| Configuration simplicity | Low | High | Highest | Highest |
| Cognitive load | High | Low | Low | Lowest |
| Audit capability | Full | Full | Full | Reduced |
| Backward compatibility | Full | High | Moderate | Low |
| Implementation effort | None | Low | Moderate | High |

## Recommendation Alignment

| Specialist | Preferred Option | Acceptable Options |
|------------|------------------|-------------------|
| The Architect | A | B, C |
| The Operator | B or C | B, C |
| The Scholar | B | B, C |
| The Challenger | N/A (adversarial) | Any with rationale |

## Consensus Trajectory

Options B and C emerge as the consensus candidates:

- Both remove Place from authorization checks (addressing 80% complexity tax)
- Both preserve audit value (Place/origin logged)
- Both allow power users to opt-in to origin restrictions (C explicitly, B via future extension)

**Key distinction**: Option B simplifies immediately; Option C renames and restructures.

Given the user's prior statement ("I cannot see a reason to allow such distinction"), the simpler path (Option B) appears aligned with project philosophy.

## Risk Assessment

### Risks of Option B (Audit-Only)

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Credential theft not mitigated | Low | Medium | Document in threat model; security-conscious users can fork |
| Config migration confusion | Low | Low | Deprecation warnings; backward compat |
| Future regret | Low | Medium | Can re-add authorization dimension if concrete use case emerges |

### Risks of Option A (Keep)

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Unnecessary complexity | High | Low | Documentation and examples |
| Admin misconfiguration | Medium | Low | Better defaults and warnings |
| Cognitive overload | Medium | Low | Training materials |

## Synthesis Conclusion

**Options B and C both satisfy the panel's constraints**:

1. No regression in security posture for realistic threat models
2. Audit capability preserved
3. Backward compatibility maintained
4. Simpler than status quo

The decision between B and C is implementation preference, not security-critical. Option B is simpler (less code change); Option C is more explicit (clearer intent).

Panel recommends proceeding to decision with Option B as primary recommendation, Option C as acceptable alternative.
