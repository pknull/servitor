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
| authority | `src/authority/` | Person/Place/Skill authorization |
| config | `src/config/` | TOML loading, validation, cron parsing |
| identity | `src/identity/` | Ed25519 keys, signing |
| egregore | `src/egregore/` | Hook, publish, context fetching |
| mcp | `src/mcp/` | McpClient trait, pool |
| scope | `src/scope/` | Allow/block policy enforcement |
| agent | `src/agent/` | LLM providers, tool_use loop |
| events | `src/events/` | EventRouter, CronSource, SseSource |
| comms | `src/comms/` | Discord transport with authorization |

## Key Files

- `src/main.rs` — CLI entry point, daemon modes, event loop
- `src/authority/mod.rs` — Authority struct, authorize(), skill checks
- `src/agent/loop.rs` — Core execution loop (tool_use → execute → feed_back)
- `src/agent/provider.rs` — LLM abstraction (Anthropic, OpenAI-compat)
- `src/mcp/pool.rs` — MCP client pool with tool introspection
- `src/scope/policy.rs` — Scope enforcement logic
- `src/egregore/context.rs` — Feed query, thread fetching
- `src/events/mod.rs` — EventSource trait, EventRouter
- `servitor.example.toml` — Configuration reference
- `authority.example.toml` — Authority configuration reference

## Commands

```bash
cargo build --release      # Build
cargo test                 # Run tests (70 total)
./target/release/servitor init    # Generate identity
./target/release/servitor info    # Show config + authority status
./target/release/servitor exec "task"  # Execute directly
./target/release/servitor run     # Daemon mode
./target/release/servitor run --hook  # Egregore hook mode
```

## Configuration

Copy `servitor.example.toml` to `servitor.toml`. Key sections:

- `[llm]` — Provider (anthropic/openai/ollama), model, API key env var
- `[mcp.*]` — MCP server definitions with scope.allow/scope.block
- `[agent]` — max_turns, timeout_secs
- `[egregore]` — api_url, subscribe (SSE)
- `[[schedule]]` — Cron tasks (name, cron, task, publish)

## Authority

Copy `authority.example.toml` to `~/.servitor/authority.toml` for access control.

**Person/Place/Skill model**:

- **Keeper**: Identity across planes (egregore pubkey, discord ID, http token)
- **Place**: Hierarchical patterns (`discord:guild:channel`, `egregore:local`)
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
