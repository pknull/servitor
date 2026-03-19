# Servitor Modes

Servitor can run in different modes depending on configuration. Each mode enables different capabilities by including or omitting config sections.

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────┐
│                        Servitor                              │
│                                                              │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐    │
│  │   LLM    │  │ Egregore │  │   A2A    │  │   MCP    │    │
│  │Reasoning │  │  Client  │  │Server/Cli│  │   Pool   │    │
│  └──────────┘  └──────────┘  └──────────┘  └──────────┘    │
│       ▲              ▲             ▲             ▲          │
│       │              │             │             │          │
│       └──────────────┴─────────────┴─────────────┘          │
│                    Config-driven                             │
└─────────────────────────────────────────────────────────────┘
```

Each component is optional. Configuration determines which are active.

## Mode Summary

| Mode | LLM | Egregore | A2A Server | A2A Client | MCP | Use Case |
|------|-----|----------|------------|------------|-----|----------|
| Full Agent | ✓ | ✓ | ✓ | ✓ | ✓ | Personal AI agent |
| Personal Agent | ✓ | ✗ | ✗ | ✗ | ✓ | Local assistant |
| Worker | ✗ | ✗ | ✓ | ✗ | ✓ | Execute tasks |
| Coordinator | ✗ | ✓ | ✓ | ✓ | ✗ | Route tasks |
| Gateway | ✗ | ✓ | ✓ | ✗ | ✗ | Egregore ↔ A2A bridge |

## Mode 1: Full Agent

The complete Servitor configuration. Can reason about tasks, subscribe to egregore feeds, delegate to A2A agents, and execute local tools.

**When to use:**

- Primary orchestration agent
- Needs to interpret ambiguous requests
- Participates in egregore network
- Coordinates multiple workers

**Data flow:**

```
Egregore ──SSE──► Servitor ──A2A──► Workers
    ▲                │
    └──attestation───┘
```

**Configuration:**

```toml
[identity]
data_dir = "/var/lib/servitor"

[llm]
provider = "anthropic"
model = "claude-sonnet-4-20250514"
api_key_env = "ANTHROPIC_API_KEY"

[egregore]
api_url = "http://localhost:7654"
subscribe = true

[agent]
max_turns = 10
timeout_secs = 300

[a2a_server]
enabled = true
bind = "0.0.0.0:8765"
name = "orchestrator"
description = "Primary orchestration agent"

[a2a.shell-worker]
url = "http://shell-worker:8765"
timeout_secs = 60

[a2a.browser-worker]
url = "http://browser-worker:8765"
timeout_secs = 120

[mcp.filesystem]
command = ["mcp-filesystem-server", "/data"]
```

## Mode 2: Personal Agent

Local assistant with LLM reasoning and tools. No network participation - just direct interaction via CLI or HTTP.

**When to use:**

- Local development assistant
- Single-user scenarios
- No need for distributed coordination

**Data flow:**

```
User ──CLI/HTTP──► Servitor ──MCP──► Tools
```

**Configuration:**

```toml
[identity]
data_dir = "~/.servitor"

[llm]
provider = "ollama"
base_url = "http://localhost:11434/v1"
model = "llama3.2:8b"

[agent]
max_turns = 20
timeout_secs = 600

[mcp.shell]
command = ["mcp-shell-server"]
env = { ALLOW_COMMANDS = "git,npm,cargo,docker" }

[mcp.filesystem]
command = ["mcp-filesystem-server", "."]
```

No `[egregore]`, `[a2a_server]`, or `[a2a.*]` sections.

## Mode 3: Worker

Headless executor. Receives structured tasks via A2A, executes using MCP tools, returns results. No reasoning - just execution.

**When to use:**

- Capability endpoint in agent cluster
- Tasks are already structured (no interpretation needed)
- Horizontal scaling of specific capabilities

**Data flow:**

```
A2A Request ──► Worker ──MCP──► Tool
                  │
A2A Response ◄───┘
```

**Configuration:**

```toml
[a2a_server]
enabled = true
bind = "0.0.0.0:8765"
name = "shell-worker"
description = "Executes shell commands"
task_timeout_secs = 120
max_concurrent_tasks = 10

[mcp.shell]
command = ["mcp-shell-server"]
env = { ALLOW_COMMANDS = "echo,ls,pwd,date,hostname,git,npm" }

[mcp.shell.scope]
allow = ["*"]
```

No `[llm]`, `[egregore]`, or `[a2a.*]` client sections.

**Authority (optional):**

```toml
# authority.toml - if access control needed
[[keeper]]
name = "internal"
http_token = "*"  # Accept any bearer token (internal cluster)
allow_skills = ["shell_*"]
```

## Mode 4: Coordinator

Routes tasks to capable workers. Subscribes to egregore for task sourcing, delegates via A2A, publishes attestations. No local execution.

**When to use:**

- Central routing for worker cluster
- Egregore integration without local tools
- Task distribution based on capabilities

**Data flow:**

```
Egregore ──SSE──► Coordinator ──A2A──► Worker A
    ▲                   │
    │                   ├──A2A──► Worker B
    │                   │
    └───attestation─────┘
```

**Configuration:**

```toml
[identity]
data_dir = "/var/lib/servitor"

[egregore]
api_url = "http://egregore:7654"
subscribe = true

[a2a_server]
enabled = true
bind = "0.0.0.0:8765"
name = "coordinator"
description = "Task router for worker cluster"

[a2a.shell-worker]
url = "http://shell-worker:8765"
timeout_secs = 60
[a2a.shell-worker.auth]
type = "bearer"
token_env = "SHELL_WORKER_TOKEN"

[a2a.docker-worker]
url = "http://docker-worker:8765"
timeout_secs = 120
[a2a.docker-worker.auth]
type = "bearer"
token_env = "DOCKER_WORKER_TOKEN"

[a2a.browser-worker]
url = "http://browser-worker:8765"
timeout_secs = 300
[a2a.browser-worker.auth]
type = "bearer"
token_env = "BROWSER_WORKER_TOKEN"
```

No `[llm]` or `[mcp.*]` sections.

## Mode 5: Gateway

Bridges egregore network to A2A endpoint. Receives tasks from egregore, exposes them via A2A server for external agents to claim.

**When to use:**

- Expose egregore tasks to A2A-only agents
- Bridge between networks
- Protocol translation layer

**Data flow:**

```
Egregore ──SSE──► Gateway ◄──A2A──► External Agent
    ▲                │
    └──attestation───┘
```

**Configuration:**

```toml
[identity]
data_dir = "/var/lib/servitor"

[egregore]
api_url = "http://egregore:7654"
subscribe = true

[a2a_server]
enabled = true
bind = "0.0.0.0:8765"
name = "egregore-gateway"
description = "Egregore to A2A bridge"
```

No `[llm]`, `[mcp.*]`, or `[a2a.*]` client sections.

## Deployment Patterns

### Pattern A: Single Agent

```
User ──► Servitor (Personal Agent mode)
```

### Pattern B: Agent + Workers

```
User ──► Servitor (Full Agent) ──A2A──► Workers
```

### Pattern C: Egregore Network

```
                    ┌──► Worker A
Egregore ──► Coordinator ──► Worker B
                    └──► Worker C
```

### Pattern D: Federated

```
Egregore A ◄──gossip──► Egregore B
    │                       │
    ▼                       ▼
Servitor A              Servitor B
    │                       │
    ▼                       ▼
Workers A               Workers B
```

## Implementation Status

| Mode | Config Support | Runtime Support | Tested |
|------|----------------|-----------------|--------|
| Full Agent | ✓ | ✓ | ✓ |
| Personal Agent | ✓ | ✓ | ✓ |
| Worker | ✓ | ✓ | ✓ |
| Coordinator | ✓ | Partial | ✗ |
| Gateway | ✓ | Partial | ✗ |

All config sections are now optional:

- `[llm]` - Only required for modes that use LLM reasoning (exec, hook, daemon with SSE/Discord/cron)
- `[identity]` - Defaults to `~/.servitor`, auto-generates keys if missing
- All other sections have sensible defaults

## Next Steps

To fully support all modes:

1. ~~Make `[llm]` section optional in config schema~~ ✓ Done
2. Make `[identity]` optional (auto-generate transient keys in memory)
3. Add mode detection based on which sections are present
4. Add health checks appropriate to each mode
5. Document authority patterns for each mode
