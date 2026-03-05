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
| config | `src/config/` | TOML loading, validation |
| identity | `src/identity/` | Ed25519 keys, signing |
| egregore | `src/egregore/` | Hook receiver, HTTP publish |
| mcp | `src/mcp/` | McpClient trait, pool |
| scope | `src/scope/` | Allow/block policy enforcement |
| agent | `src/agent/` | LLM providers, tool_use loop |

## Key Files

- `src/main.rs` — CLI entry point, daemon modes
- `src/agent/loop.rs` — Core execution loop (tool_use → execute → feed_back)
- `src/agent/provider.rs` — LLM abstraction (Anthropic, OpenAI-compat)
- `src/mcp/pool.rs` — MCP client pool with tool introspection
- `src/scope/policy.rs` — Scope enforcement logic
- `servitor.example.toml` — Configuration reference

## Commands

```bash
cargo build --release      # Build
cargo test                 # Run tests (37 total)
./target/release/servitor init    # Generate identity
./target/release/servitor info    # Show config
./target/release/servitor exec "task"  # Execute directly
./target/release/servitor run     # Daemon mode
./target/release/servitor run --hook  # Egregore hook mode
```

## Configuration

Copy `servitor.example.toml` to `servitor.toml`. Key sections:

- `[llm]` — Provider (anthropic/openai/ollama), model, API key env var
- `[mcp.*]` — MCP server definitions with scope.allow/scope.block
- `[agent]` — max_turns, timeout_secs

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

**Complete**: Phases 1-4 (foundation, MCP, agent loop, hardening)

**Deferred**: Scheduled tasks, event watchers, consumer groups, capability challenges

## Code Style

- Rust 2021 edition
- `cargo fmt` + `cargo clippy`
- Async with tokio
- Error handling via `thiserror`
