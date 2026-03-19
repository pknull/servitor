---
panel_id: "2026-03-18--servitor-daemon-refactor"
phase: 8
phase_name: "Decision"
started: "2026-03-18T19:55:00+10:00"
completed: "2026-03-18T20:00:00+10:00"
---

# Phase 8: Decision

## Panel Decision: Servitor Daemon Refactor

**Consensus: 75% (Moderate)** | **Decision Rule: Consensus**

---

## Summary

Extract servitor/src/main.rs (1,655 lines) into 2 new modules (`cli/`, `runtime/`) plus extensions to existing modules (`task/`, `egregore/`, `authority/`, `comms/`, `config/`). Use context structs to reduce parameter passing. Execute in 5 incremental phases with tests between each.

---

## Key Findings

| Area | Finding |
|------|---------|
| Current Size | 1,655 lines (2x hard limit of 800) |
| Bug Found | Duplicate `start_task()` at lines 665, 670 |
| Parameter Issue | `execute_assigned_task()` has 11 params |
| Risk Level | Medium - complex state coupling in event loop |
| Time Estimate | 2-4 hours across 5 phases |

---

## Approved Architecture

### New Modules

| Module | Files | Lines (est.) | Purpose |
|--------|-------|--------------|---------|
| `cli/` | 4 | ~200 | Subcommand implementations |
| `runtime/` | 5 | ~450 | Daemon/hook modes, stats, profile |

### Extended Modules

| Module | New File | Function(s) |
|--------|----------|-------------|
| `task/` | `execution.rs`, `handlers.rs`, `filter.rs` | Task execution, SSE handling |
| `egregore/` | `publish.rs` | Auth denied event publishing |
| `authority/` | `runtime.rs` | Runtime authority loading |
| `comms/` | `task.rs` | Comms-to-task conversion |
| `config/` | `default.rs` | Default config template |

### Context Structs

```rust
// Reduces execute_assigned_task from 11 params to 4
pub struct ExecutionContext<'a> { ... }
pub struct DaemonState { ... }
```

---

## Dissent (25%)

**The Challenger**: The current system works. No bugs have been reported by users. This refactor is preventive maintenance, not urgent need. The duplicate `start_task()` bug found validates the review but doesn't prove extraction is necessary - it could be fixed in place.

**Counter**: Panel majority accepts that timing is favorable (post-security-hardening) and the bug discovery validates that large files hide issues.

---

## Next Steps

| Phase | Action | Risk | Deliverable |
|-------|--------|------|-------------|
| 0 | Fix duplicate `start_task()` bug | Low | Bug fix commit |
| 1 | Extract helpers to existing modules | Low | 6 functions moved |
| 2 | Extract authority helpers | Low | 2 functions moved |
| 3 | Create `cli/` module | Medium | 3 subcommands |
| 4 | Extend `task/` with handlers | Medium | Context structs + handlers |
| 5 | Create `runtime/` module | High | Daemon/hook modes |

### Execution Protocol

1. **Single extraction per commit**
2. **Run `cargo test` after each extraction**
3. **Run `cargo clippy` after each extraction**
4. **Commit before next extraction**

### Success Criteria

- [ ] main.rs under 200 lines
- [ ] No module over 500 lines
- [ ] All 129 tests pass
- [ ] No circular dependencies
- [ ] `execute_assigned_task()` uses context struct

---

## Confidence

- **Relevance**: 0.95 (directly addresses panel topic)
- **Completeness**: 0.90 (clear phases, context structs defined)
- **Confidence**: 0.85 (High - established patterns, incremental approach)
