---
panel_id: "2026-03-19--authority-place-analysis"
phase: 2
phase_name: "Infrastructure Check"
started: "2026-03-19T19:00:00+10:00"
---

# Phase 2: Infrastructure Check

## Existing Documentation

### Configuration Reference

| File | Relevance |
|------|-----------|
| `authority.example.toml` | Primary example showing Place patterns |
| `docs/configuration.md` | Documents Place as part of permission schema |
| `src/authority/mod.rs` | Module-level documentation on Person/Place/Skill |

### Related Panel Decisions

No prior panels have directly addressed the Place authorization question. Related panels:

| Panel | Relevance |
|-------|-----------|
| `2026-01-29--discord-claude-bot-security` | Discord security context, no Place-specific decision |
| `2026-03-18--servitor-daemon-refactor` | Daemon architecture, uses Place but doesn't question it |

## Codebase Infrastructure

### Authority Module Structure

```
src/authority/
  mod.rs          # Authority struct, authorize(), authorize_skill()
  config.rs       # TOML parsing for authority.toml
  keeper.rs       # Keeper and PersonId types
  permission.rs   # Permission struct, pattern_matches(), skill_pattern_matches()
```

### Place Usage Sites

| Location | Function | Place Value |
|----------|----------|-------------|
| `cli/daemon_handlers.rs:58` | `handle_discord_message()` | `discord:{guild}:{channel}` |
| `cli/daemon_handlers.rs:179` | `handle_event_router_task()` | `egregore:local` |
| `a2a/server/handlers.rs:245` | A2A request handler | `a2a:server` |
| `cli/hook.rs:45` | Egregore hook mode | `egregore:local` |
| `authority/mod.rs:303` | `authorize_local_exec()` | `egregore:local` |

### Pattern Matching Implementation

```rust
// permission.rs:94-125
pub fn pattern_matches(pattern: &str, value: &str) -> bool {
    // Wildcard matches everything
    if pattern == "*" { return true; }

    // Split on colons, match segments
    // Trailing * matches any remaining depth
    // Segment * matches any single segment
}
```

### Test Coverage

| Test | File | Coverage |
|------|------|----------|
| `test_pattern_wildcard` | `permission.rs` | `*` matches anything |
| `test_pattern_exact` | `permission.rs` | Exact segment matching |
| `test_pattern_trailing_wildcard` | `permission.rs` | `discord:guild:*` patterns |
| `test_pattern_segment_wildcard` | `permission.rs` | `discord:*:general` patterns |
| `test_authorize_limited_access` | `mod.rs` | Place restriction enforcement |

## Configuration Schema

### Permission Structure

```toml
[[permission]]
keeper = "name"           # Required: keeper name
place = "pattern"         # Required: place pattern
skills = ["pattern", ...] # Required: skill patterns
```

### Place Pattern Grammar (Informal)

```
place_pattern := "*" | segment (":" segment)*
segment       := "*" | identifier
identifier    := [a-zA-Z0-9_-]+
```

### Valid Place Examples

```
*                              # Anywhere
egregore:*                     # Any egregore
egregore:local                 # This node's egregore
discord:*                      # Any Discord
discord:187489110150086656     # Specific guild
discord:187489110150086656:*   # Any channel in guild
discord:187489110150086656:123 # Specific channel
a2a:server                     # A2A JSON-RPC endpoint
a2a:*                          # Any A2A context
http:*                         # HTTP context (future)
```

## Implementation Complexity Assessment

### If Place Removed

| Component | Change Required |
|-----------|-----------------|
| `AuthRequest` | Remove `place` field |
| `Permission` | Remove `place` field |
| `Permission::matches()` | Remove place pattern matching |
| `authority.toml` schema | Remove `place` from `[[permission]]` |
| `daemon_handlers.rs` | Remove place construction |
| `a2a/server/handlers.rs` | Remove place construction |
| `cli/hook.rs` | Remove place construction |
| Tests | Update all authorization tests |

**Estimated LOC change**: ~100-150 lines removal, ~50 lines modification

### If Place Simplified (Audit Only)

| Component | Change Required |
|-----------|-----------------|
| `Permission::matches()` | Remove place pattern matching |
| Logging sites | Keep place construction for logs |
| `authority.toml` schema | Mark `place` as optional/ignored |

**Estimated LOC change**: ~30 lines modification, deprecation warnings

### If Place Kept

No code changes. Document rationale in `CLAUDE.md` or design doc.

## Gaps Identified

### Missing Documentation

1. **Threat model document**: No formal threat model documenting what Place is intended to prevent
2. **Design rationale**: No ADR or design doc explaining why Place was included

### Missing Tests

1. **Cross-channel scenario**: No test for "same keeper, different place, different permissions"
2. **Place redundancy**: No test demonstrating Place provides value beyond Person

### Questions Requiring Research

1. **Industry comparison**: How do Boundary, Teleport, SPIFFE handle origin context?
2. **ABAC literature**: What is theoretical role of environment/context attributes?
3. **Zero-trust models**: Does Place align with zero-trust "never trust, always verify" principles?

## Recommendation for Phase 3

Panel is ready to proceed. Infrastructure check reveals:

1. **Implementation is clean** - Place is well-encapsulated in authority module
2. **Change is low-risk** - All Place usage sites are identified
3. **Evidence is available** - Codebase provides concrete examples to analyze
4. **Gaps are addressable** - Missing threat model can be constructed during panel

**Proceed to Phase 3: Initial Positions**

Each specialist should prepare a 5-bullet position statement addressing:

1. Does Place provide authorization value?
2. Concrete scenario where Place matters (or doesn't)
3. Implementation cost assessment
4. Recommendation (keep/simplify/remove)
5. Key risk or concern
