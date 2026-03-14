# Servitor

Servitor is an egregore-connected task executor that owns MCP clients directly,
uses an LLM for planning and execution, and publishes signed attestations back
to the feed.

It implements the ZeroClaw pattern: communication arrives over egregore, hook
stdin, cron, watcher events, or Discord; execution happens through scoped MCP
tools; results and audit events are published as signed messages.

## Quick Start

Build and install:

```bash
cargo build --release
cp target/release/servitor ~/.local/bin/
```

Create local config and authority files:

```bash
cp servitor.example.toml servitor.toml
servitor init
cp authority.example.toml ~/.servitor/authority.toml
```

Then either execute directly:

```bash
servitor exec "List files in ~/Documents"
```

or run the daemon:

```bash
servitor run
```

Daemon and hook modes refuse work unless `authority.toml` is present or
`--insecure` is set explicitly for local development. Local `servitor exec`
runs authorize as the servitor's own egregore identity when authority is
configured.

## Documentation

- Docs index: [docs/README.md](docs/README.md)
- Protocol and message lifecycle: [docs/protocol.md](docs/protocol.md)
- Configuration reference: [docs/configuration.md](docs/configuration.md)
- Operational guidance: [docs/operations.md](docs/operations.md)
- HTTP and OpenAPI boundary: [docs/api/README.md](docs/api/README.md)

## Architecture

```text
┌─────────────────┐     ┌─────────────────────────────────────────────┐
│    Egregore     │────▶│                  SERVITOR                    │
│   (messages)    │◀────│  ┌─────────────┐  ┌─────────────────────┐  │
│                 │     │  │ Task State  │  │   MCP Client Pool   │  │
│  - task         │     │  │ (reasoning) │──│  ┌─────┐ ┌─────┐   │  │
│  - task_offer   │     │  └─────────────┘  │  │stdio│ │http │   │  │
│  - task_assign  │     │                    │  └──┬──┘ └──┬──┘   │  │
│  - task_result  │     │                    └─────┼───────┼──────┘  │
│  - profile      │     │   ┌──────────────────────┴───────┴──────┐  │
└─────────────────┘     │   │          Scope Enforcer             │  │
                        │   │   (allowlist/blocklist per MCP)     │  │
                        │   └─────────────────────────────────────┘  │
                        └─────────────────────────────────────────────┘
```

### Three-Plane Model

| Plane | Purpose | Examples |
|-------|---------|----------|
| **Communication** | Task and operator interaction | Egregore, Discord, hook stdin |
| **Tool** | Execution capabilities | MCP servers over stdio or HTTP |
| **LLM** | Planning and reasoning | Anthropic, OpenAI-compatible, Codex, Claude Code |

## Operating Modes

| Mode | Command | Purpose |
|------|---------|---------|
| Direct exec | `servitor exec ...` | Local one-shot execution |
| Planning only | `servitor exec --dry-run ...` | Produce a local validated `task_plan` and stop |
| Plan-first exec | `servitor exec --plan-first ...` | Publish a `task_plan`, then execute |
| Daemon | `servitor run` | SSE subscription, cron, watchers, Discord |
| Hook | `servitor run --hook` | Read one egregore `task` envelope from stdin |

## Protocol Overview

Servitor currently uses two related task paths:

1. Direct, hook, cron, and watcher execution:
   `task -> task_claim (advisory) -> optional task_plan -> task_result`
2. SSE-coordinated network execution:
   `task -> task_offer -> task_assign -> task_started -> task_status/task_failed -> task_result`

Additional audit and observability messages:

- `servitor_profile`: capability advertisement and heartbeat
- `task_offer_withdraw`: offer TTL expired before assignment
- `task_ping`: request a status update from an active execution
- `auth_denied`: published when offer or assignment authorization fails
- `trace_span`: opt-in distributed execution tracing
- `notification`: outbound notification payloads

See [docs/protocol.md](docs/protocol.md) for the lifecycle, auth gates, and
message semantics.

## Configuration Highlights

Key config surfaces live in [servitor.example.toml](servitor.example.toml):

- `[llm]`: provider, model, credentials, and provider-specific auth
- `[mcp.*]`: tool transports, timeouts, and scope enforcement
- `[task]`: offer, assignment, start, ETA, and ping timeouts
- `[agent]`: execution budget plus opt-in `trace_span` publishing
- `[heartbeat]`: profile cadence plus opt-in runtime monitoring fields
- `[egregore]`: egregore endpoint and SSE subscription
- `[comms.discord]`: live inbound operator transport

The `[egregore.group]` and `[comms.http]` sections are documented as reserved
configuration on this branch. They are parsed by the config schema but are not
wired into the current runtime.

## Scope Enforcement

- Block patterns take precedence over allow patterns
- Patterns support glob syntax: `*`, `**`, `?`
- Scoped patterns bind a tool name to an argument pattern, for example `execute:/etc/*`
- Per-task `scope_override` can only further restrict access; it cannot widen a tool's configured scope

## Deployment

- Sandboxed sidecar deployment guide: [docs/deployment/containerization.md](docs/deployment/containerization.md)
- Example compose stack: [examples/containerized/docker-compose.yml](examples/containerized/docker-compose.yml)
- Example systemd units: [examples/systemd/](examples/systemd/)

## LLM Providers

| Provider | Config | Notes |
|----------|--------|-------|
| `anthropic` | `api_key_env` | Claude API models |
| `openai` | `api_key_env` | OpenAI API via OpenAI-compatible client |
| `ollama` | `base_url` optional | Local inference via OpenAI-compatible client |
| `openai-compat` | `base_url`, optional `api_key_env` | Any compatible endpoint |
| `codex` | `token_file`, optional `oauth_profile` | OAuth-backed Codex provider |
| `claude-code` | none required | Uses local Claude Code authentication |

## HTTP and OpenAPI

Servitor does not expose a stable inbound HTTP control API on this branch, so
it does not currently ship its own OpenAPI document. It talks to egregore's
HTTP API and to MCP servers over HTTP as a client. See
[docs/api/README.md](docs/api/README.md) for the exact boundary.

## Development

```bash
cargo test
cargo check
cargo build --release
```

## License

MIT
