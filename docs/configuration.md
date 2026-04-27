# Servitor Configuration

Primary runtime configuration lives in `servitor.toml`. Keeper authorization
lives separately in `authority.toml`.

Start from:

```bash
cp servitor.example.toml servitor.toml
cp authority.example.toml ~/.servitor/authority.toml
```

This document describes Servitor's current role as a headless executor. Its
job is to receive pre-planned tasks, enforce authority and scope, execute tool
calls, and publish results through the local egregore node.

## Top-Level Sections

| Section | Purpose |
|---------|---------|
| `identity` | Local key storage location |
| `egregore` | Egregore API endpoint and SSE subscription |
| `mcp.*` | Tool transports, timeouts, notification templates, and scopes |
| `a2a.*` | External A2A endpoints used as executable tool backends |
| `a2a_server` | A2A server for receiving external structured tasks |
| `metrics` | Prometheus metrics endpoint |
| `agent` | Execution timeout and trace publishing |
| `task` | Offer/assignment/start/ping timing |
| `heartbeat` | Profile cadence and runtime monitoring toggle |
| `schedule` | Cron-driven structured task generation |
| `watch` | MCP notification task templates |

## `identity`

`identity.data_dir` points at the local Servitor key material and authority
files. The default is `~/.servitor`.

## `egregore`

- `api_url`: local egregore node base URL
- `subscribe`: subscribe to the egregore SSE feed in daemon mode

Current branch note:

- `egregore.group` is parsed by the config schema but is not consumed by the
  current runtime. Do not rely on it for task ownership or deduplication yet.

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

`on_notification` is optional. When present, it must be an array of structured
tool calls. String prompt templates are no longer part of the active runtime
contract.

Daemon mode polls MCP servers for stdio notifications and routes matching
events through these templates.

## `a2a.*`

Each named A2A section declares an external agent endpoint that can be invoked
as part of execution. Servitor does not plan when to delegate; it only runs the
pre-planned call it was given.

Required fields:

- `url`: agent's A2A endpoint (for example `http://agent.local:8765/a2a`)

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

Enable Servitor to receive structured tasks from external A2A agents.

- `enabled`: toggle the A2A server
- `bind`: listen address (for example `127.0.0.1:8765`)
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
- `bind`: listen address (for example `127.0.0.1:9090`)

When enabled, exposes `/metrics` with counters, histograms, and gauges for
tool calls, task execution, and task lifecycle events.

## `agent`

- `timeout_secs`: default execution timeout
- `publish_trace_spans`: opt-in tracing over egregore

Servitor no longer owns planning or multi-turn reasoning. Any older
planner-oriented fields should be treated as legacy residue, not part of the
active contract.

When `publish_trace_spans = true`, Servitor emits task-level and per-tool
`trace_span` messages. If inbound work already carries top-level Egregore
`trace_id` / `span_id` fields, Servitor reuses that trace context instead of
starting an unrelated trace tree.

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

## `profile`

Planner-facing executor metadata:

- `roles`: low-cardinality placement roles such as `docker-host` or `staging`
- `labels`: small stable key/value metadata used for filtering or placement
- `targets`: operator-curated deployment targets published in
  `servitor_manifest`

Each `[[profile.targets]]` entry supports:

- `target_id`
- `kind`
- `summary`
- `roles`
- `snapshot_ttl_secs`
- `snapshot_tool_calls`

`snapshot_tool_calls` are structured probe calls the servitor runs locally to
publish `environment_snapshot` state for that target.

Probe arguments are sanitized before publication. Probe output is reduced
through the same output-defense pipeline used for direct tool execution so the
published snapshot remains planner-facing rather than raw tool output.

## `schedule`

Each `[[schedule]]` entry creates a cron-triggered structured task. The active
contract is executor-oriented: scheduled work should resolve to explicit tool
calls rather than free-form natural-language prompts.

Common fields:

- `name`
- `cron`
- `tool_calls`
- `publish`
- `notify`

Cron expressions use a six-field form with seconds.

## `watch`

Each `[[watch]]` entry maps an MCP notification into a task template:

- `name`
- `mcp`
- `event`
- `filter`
- `prompt`
- `tool_calls`
- `notify`

Watcher-generated tasks should follow the same rule as all other Servitor
inputs: execution happens from pre-planned tool calls.

Daemon mode evaluates watcher routes against incoming MCP notifications and
emits structured execution tasks for matching events.

## `authority.toml`

Authorization is configured separately from `servitor.toml`.

Relevant concepts:

- Keepers map identities across egregore, Discord, and HTTP bearer tokens
- Permissions match `place` and `skill` patterns
- Request authorization uses `request:<task_type>` in both SSE and hook mode
- Assignment delegation uses `assign:<task_type>`

Hook mode also requires the normalized task requestor to match the Egregore
envelope author before execution proceeds.

Without `authority.toml`, daemon and hook execution refuse work unless
`--insecure` is supplied explicitly.
