# Servitor

Egregore network task executor using MCP servers as capabilities.

## Architecture

**ZeroClaw pattern**: Servitor owns MCP clients directly. An LLM provides reasoning, emitting `tool_use` blocks that Servitor executes against scoped MCP servers, publishing signed attestations back to egregore.

```
Egregore ──► Servitor ──► MCP Servers (tools)
    ▲            │
    │            ▼
    └─── LLM (reasoning)
```

### Three Planes

| Plane | Purpose | This Project |
|-------|---------|--------------|
| Communication | Message transport | Egregore (hook + publish) |
| Tool | Execution capabilities | MCP pool (stdio/http) |
| LLM | Inference/reasoning | Anthropic, OpenAI, Ollama |

## Module Map

| Module | Location | Purpose |
|--------|----------|---------|
| a2a | `src/a2a/` | Agent-to-Agent protocol (client, server, pool) |
| authority | `src/authority/` | Person/Skill authorization |
| cli | `src/cli/` | Command implementations (daemon, exec, hook, info, init) |
| config | `src/config/` | TOML loading, validation, cron parsing |
| identity | `src/identity/` | Ed25519 keys, signing |
| egregore | `src/egregore/` | Hook, publish, context fetching |
| mcp | `src/mcp/` | McpClient trait, pool, circuit breaker |
| metrics | `src/metrics.rs` | Prometheus metrics for observability |
| runtime | `src/runtime/` | RuntimeContext, stats tracking, auth events |
| scope | `src/scope/` | Allow/block policy enforcement |
| agent | `src/agent/` | LLM providers, tool_use loop |
| task | `src/task/` | Task filtering, handlers, state management |
| events | `src/events/` | EventRouter, CronSource, SseSource |

## Key Files

- `src/main.rs` — CLI entry point, argument parsing
- `src/cli/daemon.rs` — Daemon mode event loop
- `src/authority/mod.rs` — Authority struct, authorize(), skill checks
- `src/agent/loop.rs` — Core execution loop (tool_use → execute → feed_back)
- `src/agent/providers/mod.rs` — LLM abstraction (Anthropic, OpenAI, Claude CLI, Codex)
- `src/a2a/server/mod.rs` — A2A server (JSON-RPC 2.0 task delegation)
- `src/mcp/pool.rs` — MCP client pool with tool introspection
- `src/runtime/context.rs` — RuntimeContext initialization
- `src/metrics.rs` — Prometheus metrics (counters, histograms, gauges)
- `src/scope/policy.rs` — Scope enforcement logic
- `src/egregore/context.rs` — Feed query, thread fetching
- `src/events/mod.rs` — EventSource trait, EventRouter
- `servitor.example.toml` — Configuration reference
- `authority.example.toml` — Authority configuration reference

## Commands

```bash
cargo build --release      # Build
cargo test                 # Run tests (187 total)
./target/release/servitor init    # Generate identity
./target/release/servitor info    # Show config + authority status
./target/release/servitor exec "task"  # Execute directly
./target/release/servitor run     # Daemon mode
./target/release/servitor run --hook  # Egregore hook mode
```

## Configuration

Copy `servitor.example.toml` to `servitor.toml`. Key sections:

- `[llm]` — Provider (anthropic/openai/ollama/claude_cli/codex), model, API key env var
- `[mcp.*]` — MCP server definitions with scope.allow/scope.block
- `[a2a.*]` — A2A agent clients (delegate tasks to external agents)
- `[a2a_server]` — A2A server (receive tasks from external agents)
- `[agent]` — max_turns, timeout_secs
- `[egregore]` — api_url, subscribe (SSE)
- `[metrics]` — enabled, bind address for Prometheus endpoint
- `[[schedule]]` — Cron tasks (name, cron, task, publish)

## Authority

Copy `authority.example.toml` to `~/.servitor/authority.toml` for access control.

**Person/Skill model**:

- **Keeper**: Identity across planes (egregore pubkey, discord ID, http token)
- **Skill**: Tool patterns (`shell:execute`, `docker:*`)

No `authority.toml` means daemon and hook modes refuse to start. Use `--insecure` only for local development when you intentionally want open mode.

## Memory Bank

| File | Purpose |
|------|---------|
| `Memory/activeContext.md` | Current status, next steps |
| `Memory/projectbrief.md` | Architecture overview |
| `Memory/techEnvironment.md` | Tech stack, commands |

## Related Projects

- `~/Code/egregore` — Network daemon (identity patterns reused from `src/identity/keys.rs`)
- `~/Projects/threshold/Work/panels/2026-03-04--watcher-design/` — Design panel decision

## Implementation Status

**Complete**: v1 rebuild (foundation, egregore, MCP, scope, agent, events, authority)

**Deferred**: Consumer groups, capability challenges, reputation tracking

## Code Style

- Rust 2021 edition
- `cargo fmt` + `cargo clippy`
- Async with tokio
- Error handling via `thiserror`
