# Servitor

Pure tool executor for the egregore network. Receives pre-planned tool calls from Familiar and executes them against scoped MCP servers, publishing signed attestations back to egregore.

## Architecture

**Direct execution model**: Servitor has no LLM. Familiar (the planner) decomposes user requests into concrete tool calls. Servitor executes them sequentially, validates scope and authority per call, and publishes signed results.

```
Familiar ──► egregore feed ──► Servitor ──► MCP Servers (tools)
                                   │
                                   ▼
                              egregore feed ──► task_result (signed)
```

### Two Planes

| Plane | Purpose | This Project |
|-------|---------|--------------|
| Communication | Message transport | Egregore (SSE subscribe + publish) |
| Tool | Execution capabilities | MCP pool (stdio/http) + A2A agents |

## Module Map

| Module | Location | Purpose |
|--------|----------|---------|
| a2a | `src/a2a/` | Agent-to-Agent protocol (client, server, pool) |
| agent | `src/agent/` | Direct execution, output defense, sanitization |
| authority | `src/authority/` | Person/Skill authorization |
| cli | `src/cli/` | Command implementations (daemon, exec, hook, info, init) |
| config | `src/config/` | TOML loading, validation, cron parsing |
| egregore | `src/egregore/` | Messages, publish, profile building, context fetching |
| events | `src/events/` | EventRouter, CronSource, SseSource, MCP notifications |
| identity | `src/identity/` | Ed25519 keys, signing |
| mcp | `src/mcp/` | McpClient trait, pool, circuit breaker |
| metrics | `src/metrics.rs` | Prometheus metrics for observability |
| runtime | `src/runtime/` | RuntimeContext, stats tracking, auth events |
| scope | `src/scope/` | Allow/block policy enforcement |
| session | `src/session/` | Session store, task watcher, pending task lifecycle |
| task | `src/task/` | Task filtering, handlers, state management, coordinator |

## Key Files

- `src/main.rs` — CLI entry point, argument parsing
- `src/cli/daemon.rs` — Daemon mode event loop (SSE + cron + heartbeat)
- `src/cli/daemon_handlers.rs` — Event handler functions (task execution, heartbeat)
- `src/agent/direct.rs` — Direct execution: validate scope, call MCP tools, build signed result
- `src/agent/output_defense.rs` — Output defense pipeline (size limits, credential redaction, instruction detection)
- `src/authority/mod.rs` — Authority struct, authorize(), skill checks
- `src/a2a/server/mod.rs` — A2A server (JSON-RPC 2.0 task delegation)
- `src/mcp/pool.rs` — MCP client pool with tool introspection
- `src/egregore/messages.rs` — All message types (Task, TaskResult, ServitorManifest, etc.)
- `src/egregore/publish.rs` — Publish methods for profiles, offers, results, manifests
- `src/egregore/profile.rs` — Build profile and manifest from runtime state
- `src/task/handlers.rs` — SSE message processing, offer/assignment flow
- `src/task/state.rs` — TaskCoordinator, offer tracking, assignment lifecycle
- `src/events/mod.rs` — EventSource trait, EventRouter, task_from_template
- `src/events/cron.rs` — Cron schedule evaluation
- `src/events/sse.rs` — SSE subscription and capability filtering
- `src/events/mcp.rs` — MCP notification routing to tasks
- `src/scope/policy.rs` — Scope enforcement logic
- `src/runtime/context.rs` — RuntimeContext initialization
- `src/metrics.rs` — Prometheus metrics (counters, histograms, gauges)
- `servitor.example.toml` — Configuration reference
- `authority.example.toml` — Authority configuration reference

## Commands

```bash
cargo build --release      # Build
cargo test                 # Run tests (166 total)
./target/release/servitor init    # Generate identity
./target/release/servitor info    # Show config + authority status
./target/release/servitor exec '[{"name":"shell__execute","arguments":{"command":"pwd"}}]'  # Execute directly
./target/release/servitor run     # Daemon mode
./target/release/servitor run --hook  # Egregore hook mode
```

## Configuration

Copy `servitor.example.toml` to `servitor.toml`. Key sections:

- `[mcp.*]` — MCP server definitions with scope.allow/scope.block
- `[a2a.*]` — A2A agent clients (delegate tasks to external agents)
- `[a2a_server]` — A2A server (receive tasks from external agents)
- `[agent]` — timeout_secs, publish_trace_spans (no LLM config)
- `[egregore]` — api_url, subscribe (SSE)
- `[heartbeat]` — interval_secs, include_runtime_monitoring
- `[profile]` — roles, labels, deployment targets
- `[metrics]` — enabled, bind address for Prometheus endpoint
- `[[schedule]]` — Cron tasks (name, cron, tool_calls, publish)

## Authority

Copy `authority.example.toml` to `~/.servitor/authority.toml` for access control.

**Person/Skill model**:

- **Keeper**: Identity across planes (egregore pubkey, discord ID, http token)
- **Skill**: Tool patterns (`shell:execute`, `docker:*`)

No `authority.toml` means daemon and hook modes refuse to start. Use `--insecure` only for local development when you intentionally want open mode.

## Task Execution Flow

1. Task arrives via SSE (or cron/hook/MCP notification)
2. Capability check: `task.required_caps` must match servitor capabilities
3. Authority check: requestor must be authorized for the skill pattern
4. If SSE: publish `task_offer`, wait for `task_assign` from Familiar
5. Execute `tool_calls` sequentially via `execute_direct()`
6. Output defense runs on every tool result
7. Build signed `task_result` with attestation (Ed25519 signature over result hash)
8. Publish result to egregore feed

Tasks without `tool_calls` are rejected with `invalid_task`. All planning happens in Familiar.

## Related Projects

- `egregore/` — Network daemon (signed feeds, gossip replication)
- `familiar/` — Conversational planner (decomposes requests into tool calls)
- `thallus-core/` — Shared library (identity, MCP, providers)

## Implementation Status

**Complete**: Direct execution, authority, scope, A2A, events (cron/SSE/MCP notifications), heartbeat, manifest/snapshot projection

**Deferred**: Consumer groups, capability challenges, reputation tracking

## Code Style

- Rust 2021 edition
- `cargo fmt` + `cargo clippy`
- Async with tokio
- Error handling via `thiserror`
