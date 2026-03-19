---
panel_id: "2026-03-18--servitor-daemon-refactor"
phase: 3
phase_name: "Initial Positions"
started: "2026-03-18T19:36:00+10:00"
completed: "2026-03-18T19:45:00+10:00"
---

# Phase 3: Initial Positions

## The Systems Designer (code:architect)

**Position**: Full extraction into 3 new modules (`runtime/`, `protocol/`, `cli/`)

**Evidence**:

- main.rs at 1,655 lines violates soft limit (400) and hard limit (800)
- Five distinct functional clusters identified with clear boundaries
- Existing codebase has good modular structure to follow

**Risks**:

- `execute_assigned_task()` has 11 parameters - needs context struct
- Event loop has complex lifetime interactions with `tokio::select!`

**Unknowns**:

- Whether `run_daemon_mode()` event loop itself should be extracted

**Recommendation**: Full extraction with context structs, incremental 4-phase approach

---

## The Extraction Surgeon (refactoring-specialist)

**Position**: Cautious extraction with daemon loop remaining in main.rs initially

**Evidence**:

- State coupling is HIGH RISK: `runtime_stats`, `task_coordinator`, `sse_source` all mutated in event loop
- Tests reference private functions that need `pub(crate)` visibility
- Nested `tokio::select!` in `execute_assigned_task()` is riskiest part

**Risks**:

- Circular dependencies if `profile.rs` imports from `daemon.rs`
- Test breakage from visibility changes
- Lifetime issues with mutable borrows across async boundaries

**Unknowns**:

- Integration test coverage for daemon mode

**Recommendation**:

1. Extract leaf functions first (RuntimeStats, helpers)
2. Create context structs BEFORE extracting execute_assigned_task()
3. Keep `run_daemon_mode()` event loop in main.rs until all helpers extracted
4. Single extraction per commit with `cargo test` between each

---

## The Quality Auditor (code:code-reviewer)

**Position**: Approved with naming concerns about `protocol/` module

**Evidence**:

- `protocol/` overlaps with existing `events/sse.rs` and `task/`
- RuntimeStats belongs in `metrics.rs` not new `runtime/`
- **BUG FOUND**: Duplicate `start_task()` calls at lines 665 and 670 in main.rs
- `md5_hash` function uses DefaultHasher, not MD5 - misnomer

**Risks**:

- `protocol/` naming creates ambiguity with existing modules
- Adding 3 new modules when functions could extend existing ones

**Unknowns**:

- Whether duplicate `start_task()` is bug or intentional

**Recommendation**:

- Rename `protocol/` to extend existing modules (`task/`, `events/`)
- Move RuntimeStats to `metrics.rs`
- Move helpers to existing modules where natural homes exist
- Final main.rs should be ~50 lines

---

## The Challenger

**Position**: OPPOSE until necessity proven

**Evidence**:

- The current system WORKS. Where are the bug reports from 1,655 lines?
- No user complaints cited
- 129 tests pass - system is stable
- Extraction adds complexity (more files, more imports, more mental overhead)

**Risks**:

- Extraction for its own sake wastes effort
- May introduce regressions in working code
- "Cleaner" code doesn't mean better if it works

**Unknowns**:

- Actual pain points from current structure
- Developer time cost vs benefit

**Recommendation**:

- Show me the bugs this extraction will prevent
- Show me the features blocked by current structure
- If answer is "none", consider deferring until an actual problem exists
