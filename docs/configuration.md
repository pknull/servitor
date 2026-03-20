# Servitor Configuration

Primary runtime configuration lives in `servitor.toml`. Keeper authorization
lives separately in `authority.toml`.

Start from:

```bash
cp servitor.example.toml servitor.toml
cp authority.example.toml ~/.servitor/authority.toml
```

## Top-Level Sections

| Section | Purpose |
|---------|---------|
| `identity` | Local key storage location |
| `egregore` | Egregore API endpoint and SSE subscription |
| `llm` | Reasoning provider selection and auth |
| `mcp.*` | Tool transports, timeouts, notification templates, and scopes |
| `a2a.*` | A2A agent clients for task delegation |
| `a2a_server` | A2A server for receiving external tasks |
| `metrics` | Prometheus metrics endpoint |
| `agent` | Turn budget, execution timeout, and trace publishing |
| `task` | Offer/assignment/start/ping timing |
| `heartbeat` | Profile cadence and runtime monitoring toggle |
| `comms` | Inbound operator transports |
| `schedule` | Cron-driven task generation |
| `watch` | MCP notification to task templates |

## `identity`

`identity.data_dir` points at the local servitor key material and authority
files. The default is `~/.servitor`.

## `egregore`

- `api_url`: egregore node base URL
- `subscribe`: subscribe to the egregore SSE feed in daemon mode

Current branch note:

- `egregore.group` is parsed by the config schema but is not consumed by the
  current runtime. Do not rely on it for task ownership or deduplication yet.

## `llm`

Supported providers:

| Provider | Required fields | Notes |
|----------|-----------------|-------|
| `anthropic` | `model`, `api_key_env` | Native Anthropic provider |
| `openai` | `model`, `api_key_env` | OpenAI via compatible client |
| `ollama` | `model` | `base_url` is optional |
| `openai-compat` | `model`, `base_url` | Compatible hosted or local endpoint |
| `codex` | `model`, `token_file` | OAuth-backed Codex integration |
| `claude-code` | `model` | Uses local Claude Code auth state |

Optional shared fields:

- `max_tokens`
- `temperature`
- `oauth_profile` for `codex`

## `mcp.*`

Each named MCP section declares one tool server.

Supported transports:

- `stdio`: requires `command`
- `http`: requires `url`

Shared fields:

- `args`
- `env`
- `timeout_secs`
- `scope.allow`
- `scope.block`
- `on_notification`

Scope rules are deny-first: block patterns always win over allow patterns.

## `a2a.*`

Each named A2A section declares an external agent to delegate tasks to.

Required fields:

- `url`: agent's A2A endpoint (e.g., `http://agent.local:8765/a2a`)

Optional fields:

- `card_url`: override `/.well-known/agent.json` discovery URL
- `auth.type`: `bearer` for token auth
- `auth.token_env`: environment variable containing the bearer token
- `timeout_secs`: per-request timeout
- `retry_attempts`: retry count on failure

Example:

```toml
[a2a.research-agent]
url = "http://localhost:8766/a2a"
auth.type = "bearer"
auth.token_env = "RESEARCH_AGENT_TOKEN"
```

## `a2a_server`

Enable servitor to receive tasks from external A2A agents.

- `enabled`: toggle the A2A server
- `bind`: listen address (e.g., `127.0.0.1:8765`)
- `name`: agent name in the published AgentCard
- `description`: agent description
- `task_timeout_secs`: max execution time per task
- `max_concurrent_tasks`: bounded in-flight tasks

Example:

```toml
[a2a_server]
enabled = true
bind = "127.0.0.1:8765"
name = "servitor"
description = "Task executor with shell and docker capabilities"
```

## `metrics`

Prometheus metrics endpoint configuration.

- `enabled`: toggle metrics endpoint
- `bind`: listen address (e.g., `127.0.0.1:9090`)

When enabled, exposes `/metrics` with counters, histograms, and gauges for
tool calls, task execution, and provider latency.

## `agent`

- `max_turns`: LLM round-trip budget per task
- `timeout_secs`: default execution timeout
- `system_prompt`: optional prefix injected into the agent prompt
- `publish_trace_spans`: opt-in tracing over egregore

## `task`

The coordinated SSE lifecycle is controlled by:

- `offer_ttl_secs`
- `offer_timeout_secs`
- `assign_timeout_secs`
- `start_timeout_secs`
- `eta_buffer_multiplier`
- `ping_timeout_secs`

These timeouts are relevant only for the offer/assign execution path.

## `heartbeat`

- `interval_secs`: heartbeat cadence
- `include_runtime_monitoring`: opt-in runtime fields in `servitor_profile`

Runtime monitoring is off by default to avoid extra feed noise.

## `comms`

Live today:

- `comms.discord`

Parsed but not wired on this branch:

- `comms.http`

That means the example HTTP webhook section is documentation of a reserved
schema surface, not an active inbound API.

## `schedule`

Each `[[schedule]]` entry creates a cron-triggered task with:

- `name`
- `cron`
- `task`
- `publish`
- `notify`

Cron expressions use a six-field form with seconds.

## `watch`

Each `[[watch]]` entry maps an MCP notification into a task template:

- `name`
- `mcp`
- `event`
- `filter`
- `task`
- `notify`

## `authority.toml`

Authorization is configured separately from `servitor.toml`.

Relevant concepts:

- Keepers map identities across egregore, Discord, and HTTP bearer tokens
- Permissions match `place` and `skill` patterns
- Request authorization uses `request:<task_type>`
- Assignment delegation uses `assign:<task_type>`

Without `authority.toml`, daemon and hook execution refuse work unless
`--insecure` is supplied explicitly.
