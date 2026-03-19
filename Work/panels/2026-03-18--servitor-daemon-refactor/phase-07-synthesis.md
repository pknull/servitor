---
panel_id: "2026-03-18--servitor-daemon-refactor"
phase: 7
phase_name: "Synthesis"
started: "2026-03-18T19:50:00+10:00"
completed: "2026-03-18T19:55:00+10:00"
---

# Phase 7: Synthesis

## Synthesized Architecture

After cross-examination, the panel converges on a **hybrid approach**: 2 new modules + extensions to existing modules.

### Final Module Structure

```
src/
├── main.rs             # ~150 lines: CLI + dispatch (down from 1,655)
│
├── cli/                # NEW (~200 lines)
│   ├── mod.rs          # Cli struct, Commands enum
│   ├── exec.rs         # run_exec()
│   ├── info.rs         # run_info()
│   └── init.rs         # run_init()
│
├── runtime/            # NEW (~450 lines)
│   ├── mod.rs          # DaemonRunner, HookRunner exports
│   ├── daemon.rs       # run_daemon_mode() event loop
│   ├── hook.rs         # run_hook_mode()
│   ├── stats.rs        # RuntimeStats
│   └── profile.rs      # build_profile()
│
├── task/               # EXISTING - extend
│   ├── mod.rs          # (existing)
│   ├── state.rs        # (existing)
│   ├── execution.rs    # NEW: execute_assigned_task()
│   ├── handlers.rs     # NEW: process_sse_message(), maybe_accept_assignment()
│   └── filter.rs       # NEW: task_matches_capabilities()
│
├── egregore/           # EXISTING - extend
│   ├── ...             # (existing files)
│   └── publish.rs      # NEW: publish_auth_denied_event()
│
├── authority/          # EXISTING - extend
│   ├── ...             # (existing files)
│   └── runtime.rs      # NEW: load_runtime_authority(), authorize_local_exec()
│
├── comms/              # EXISTING - extend
│   ├── ...             # (existing files)
│   └── task.rs         # NEW: task_from_comms()
│
└── config/             # EXISTING - extend
    ├── ...             # (existing files)
    └── default.rs      # NEW: create_default_config()
```

## Context Structs

### ExecutionContext (reduces 11 params to 4)

```rust
/// Read-only context for task execution
pub struct ExecutionContext<'a> {
    pub provider: &'a dyn Provider,
    pub mcp_pool: &'a McpPool,
    pub scope_enforcer: &'a ScopeEnforcer,
    pub identity: &'a Identity,
    pub authority: &'a Authority,
    pub egregore: &'a EgregoreClient,
    pub config: &'a Config,
    pub capability_set: &'a HashSet<String>,
}
```

### DaemonState (mutable state container)

```rust
/// Mutable state owned by daemon loop
pub struct DaemonState {
    pub runtime_stats: RuntimeStats,
    pub task_coordinator: TaskCoordinator,
    pub sse_source: Option<SseSource>,
    pub discord_transport: Option<DiscordTransport>,
    pub event_router: EventRouter,
    pub last_heartbeat: Instant,
}
```

## Extraction Phases

### Phase 1: Low-Risk Helpers (Day 1)

| Function | Target | Risk |
|----------|--------|------|
| `RuntimeStats` | `runtime/stats.rs` | Low |
| `build_profile()` | `runtime/profile.rs` | Low |
| `task_from_comms()` | `comms/task.rs` | Low |
| `task_matches_capabilities()` | `task/filter.rs` | Low |
| `publish_auth_denied_event()` | `egregore/publish.rs` | Low |
| `create_default_config()` | `config/default.rs` | Low |

### Phase 2: Authority Helpers (Day 1)

| Function | Target | Risk |
|----------|--------|------|
| `load_runtime_authority()` | `authority/runtime.rs` | Low |
| `authorize_local_exec()` | `authority/runtime.rs` | Low |

### Phase 3: CLI Subcommands (Day 2)

| Function | Target | Risk |
|----------|--------|------|
| `run_info()` | `cli/info.rs` | Low |
| `run_init()` | `cli/init.rs` | Low |
| `run_exec()` | `cli/exec.rs` | Medium |

### Phase 4: Task Handlers (Day 2-3)

| Function | Target | Risk |
|----------|--------|------|
| `process_sse_message()` | `task/handlers.rs` | Medium |
| `maybe_accept_assignment()` | `task/handlers.rs` | Medium |
| Context structs | `task/context.rs` | Low |
| `execute_assigned_task()` | `task/execution.rs` | High |

### Phase 5: Runtime Modes (Day 3)

| Function | Target | Risk |
|----------|--------|------|
| `run_hook_mode()` | `runtime/hook.rs` | Medium |
| `run_daemon_mode()` | `runtime/daemon.rs` | High |

## Trade-offs

| Option A: 3 New Modules | Option B: Extend Existing |
|-------------------------|---------------------------|
| Clearer boundaries | Less file proliferation |
| More explicit imports | Leverages existing structure |
| Easier to find code | Keeps related code together |

**Decision**: Hybrid - `cli/` and `runtime/` are new; task handlers extend `task/`

## Bug Fix Required

**Before extraction**, fix the duplicate `start_task()` bug at lines 665 and 670:

```rust
// Line 664-670 (current)
task.keeper = keeper_name.clone();
runtime_stats.start_task();  // First call

// ... 5 lines ...

let claim = TaskClaim::new(task.hash.clone(), identity.public_id(), 180);
let _ = egregore.publish_claim(&claim).await;
runtime_stats.start_task();  // Duplicate! Remove this one.
```
