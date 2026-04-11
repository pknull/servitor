# Servitor

Servitor is an egregore-connected task executor. It owns MCP clients directly, can delegate to external A2A agents, and publishes signed attestations back to the feed.

Servitor is the **hands** of Thallus:

- it does not own user-facing conversation
- it does not own task decomposition or planning
- it executes pre-planned work, enforces authority and scope, and reports results

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

Run the daemon:

```bash
servitor run
```

Execute a structured direct task locally:

```bash
servitor exec '[{"name":"shell__execute","arguments":{"command":"pwd"}}]'
```

Daemon and hook modes refuse work unless `authority.toml` is present or `--insecure` is set explicitly for local development. Local `servitor exec` runs authorize as the servitor's own egregore identity when authority is configured.

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
│   (messages)    │◀────│                                              │
│                 │     │  ┌─────────────────────┐                    │
│  - task         │     │  │   MCP Client Pool   │                    │
│  - task_offer   │     │  │  ┌─────┐ ┌─────┐   │                    │
│  - task_assign  │     │  │  │stdio│ │http │   │                    │
│  - task_result  │     │  │  └──┬──┘ └──┬──┘   │                    │
│  - profile      │     │  └─────┼───────┼──────┘                    │
└─────────────────┘     │   ┌────┴───────┴────────────────────────┐  │
                        │   │          Scope Enforcer             │  │
                        │   │   (allowlist/blocklist per MCP)     │  │
                        │   └─────────────────────────────────────┘  │
                        └─────────────────────────────────────────────┘
```

### Two-Plane Model

| Plane | Purpose | Examples |
|-------|---------|----------|
| **Communication** | Task sourcing and result publication | Egregore, A2A |
| **Tool** | Execution capabilities | MCP servers, A2A agents |

Planning and user interaction belong to Familiar, not Servitor.

## Operating Modes

| Mode | Command | Purpose |
|------|---------|---------|
| Direct exec | `servitor exec '<json tool calls>'` | Local one-shot execution of pre-planned tool calls |
| Daemon | `servitor run` | SSE subscription, cron, MCP notification routing, and profile publishing |
| Hook | `servitor run --hook` | Read one egregore `task` envelope from stdin |
| Info | `servitor info` | Show identity, capabilities, and authority state |

## Protocol Overview

Servitor currently uses two related task paths:

1. Direct, hook, cron, and MCP notification execution:
   `task -> optional task_claim -> task_result`
2. SSE-coordinated network execution:
   `task -> task_offer -> task_assign -> task_started -> task_status/task_failed -> task_result`

Additional audit and observability messages:

- `servitor_profile`: capability advertisement and heartbeat
- `servitor_manifest`: planner-facing executor manifest derived from local tool discovery
- `environment_snapshot`: target-specific planner context, optionally enriched by configured probe tool calls
- `task_offer_withdraw`: offer TTL expired before assignment
- `task_ping`: request a status update from an active execution
- `auth_denied`: published when offer or assignment authorization fails
- `trace_span`: opt-in distributed execution tracing
- `notification`: outbound notification payloads

See [docs/protocol.md](docs/protocol.md) for the lifecycle, auth gates, and message semantics.

## Configuration Highlights

Key config surfaces live in [servitor.example.toml](servitor.example.toml):

- `[mcp.*]`: tool transports, timeouts, and scope enforcement
- `[a2a.*]`: external agent clients
- `[a2a_server]`: inbound A2A server
- `[task]`: offer, assignment, start, ETA, and ping timeouts
- `[agent]`: execution timeout plus opt-in `trace_span` publishing
- `[heartbeat]`: profile cadence plus opt-in runtime monitoring fields
- `[profile]`: planner-facing roles, labels, and deployment target summaries
- `[egregore]`: egregore endpoint and SSE subscription

Configured `profile.targets[*].snapshot_tool_calls` let a servitor publish live
target snapshots without embedding planner logic in the executor. The probe
calls are operator-curated structured tool calls, not ad hoc prompts.

The `[egregore.group]` and `[comms.http]` sections are reserved configuration on this branch. They are parsed by the config schema but are not wired into the current runtime.

## Scope Enforcement

- Block patterns take precedence over allow patterns
- Patterns support glob syntax: `*`, `**`, `?`
- Scoped patterns bind a tool name to an argument pattern, for example `execute:/etc/*`
- Per-task `scope_override` can only further restrict access; it cannot widen a tool's configured scope

## Deployment

- Sandboxed sidecar deployment guide: [docs/deployment/containerization.md](docs/deployment/containerization.md)
- Example systemd units: [examples/systemd/](examples/systemd/)
- Example A2A mesh: [examples/a2a-mesh/](examples/a2a-mesh/)

## HTTP and OpenAPI

Servitor does not expose a stable inbound HTTP control API on this branch, so it does not currently ship its own OpenAPI document. It talks to Egregore's HTTP API and to MCP servers over HTTP as a client. See [docs/api/README.md](docs/api/README.md) for the exact boundary.

## Development

```bash
cargo test
cargo check
cargo build --release
```

## License

MIT
