# Servitor

Egregore network task executor using MCP servers as capabilities.

Servitor implements the **ZeroClaw pattern**: it owns MCP clients directly, uses an LLM for reasoning (emitting `tool_use` blocks), and publishes signed attestations back to egregore.

**Name etymology**: Occult term for a created thoughtform that performs specific tasks — "like software that does one thing well."

## Architecture

```
┌─────────────────┐     ┌─────────────────────────────────────────────┐
│    Egregore     │────▶│                  SERVITOR                    │
│   (messages)    │◀────│  ┌─────────────┐  ┌─────────────────────┐  │
│                 │     │  │ Agent Loop  │  │   MCP Client Pool   │  │
│  - task         │     │  │ (reasoning) │──│  ┌─────┐ ┌─────┐   │  │
│  - task_claim   │     │  └─────────────┘  │  │stdio│ │http │   │  │
│  - task_result  │     │                    │  └──┬──┘ └──┬──┘   │  │
│  - profile      │     │                    └─────┼───────┼──────┘  │
└─────────────────┘     │   ┌──────────────────────┴───────┴──────┐  │
                        │   │          Scope Enforcer             │  │
                        │   │   (allowlist/blocklist per MCP)     │  │
                        │   └─────────────────────────────────────┘  │
                        └─────────────────────────────────────────────┘
```

### Three-Plane Model

| Plane | Purpose | Examples |
|-------|---------|----------|
| **Communication** | Message transport | Egregore, Discord, TUI |
| **Tool** | Execution capabilities | MCP servers (Docker, Shell) |
| **LLM** | Inference/reasoning | Claude, Ollama, OpenAI |

## Installation

```bash
cargo build --release
cp target/release/servitor ~/.local/bin/
```

## Configuration

Copy the example configuration:

```bash
cp servitor.example.toml servitor.toml
```

Edit `servitor.toml` to configure:

- LLM provider (Anthropic, OpenAI, Ollama)
- MCP servers (tools/capabilities)
- Scope enforcement (what tools can access)
- Egregore network connection
- Optional consumer group membership for exactly-once SSE task ownership

Example consumer group configuration:

```toml
[egregore]
api_url = "http://127.0.0.1:7654"
subscribe = true

[egregore.group]
name = "workers"
heartbeat_interval_secs = 5
```

Each servitor in the same named group publishes its membership in
`servitor_profile` heartbeats. Group members deterministically hash task IDs to
pick a single owner, so tasks are distributed without duplicate execution and
rebalance automatically when heartbeats go stale.

## Usage

### Initialize identity

```bash
servitor init
```

### Show configuration

```bash
servitor info
```

### Execute a task directly

```bash
servitor exec "List files in ~/Documents"
```

### Run as daemon

```bash
servitor run
```

### Run in hook mode (egregore integration)

Configure as an egregore hook in `config.toml`:

```toml
[[hooks]]
name = "servitor"
on_message = "/path/to/servitor"
args = ["run", "--hook"]
timeout_secs = 300
idempotent = true
```

Then messages with type `task` will be routed to Servitor.

## MCP Server Configuration

```toml
[mcp.shell]
transport = "stdio"
command = "mcp-server-shell"
scope.allow = ["execute:~/scripts/*"]
scope.block = ["execute:/etc/*", "execute:rm *"]
```

### Scope Enforcement

- **Block patterns take precedence** over allow patterns
- Patterns support glob syntax (`*`, `**`, `?`)
- Scoped patterns: `execute:/etc/*` matches tool name + argument

## Message Types

| Type | Direction | Purpose |
|------|-----------|---------|
| `servitor_profile` | Out | Capability advertisement |
| `task` | In | Task request |
| `task_claim` | Out | Claim task before execution |
| `task_result` | Out | Signed attestation of result |

## LLM Providers

All providers compiled in, runtime selection via config:

| Provider | Config | Notes |
|----------|--------|-------|
| `anthropic` | `api_key_env` | Claude models |
| `openai` | `api_key_env` | GPT models |
| `ollama` | `base_url` | Local inference |
| `openai-compat` | `base_url`, `api_key_env` | Any compatible endpoint |

## Development

```bash
# Run tests
cargo test

# Check compilation
cargo check

# Build release
cargo build --release
```

## License

MIT
