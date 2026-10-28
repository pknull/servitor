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

## `mcp.*`

Each named MCP section declares one tool server.

Section names under `[mcp.*]` and `[a2a.*]` are published as provider names,
so they must be 1 to 64 ASCII letters, digits, `_`, `.` or `-`, starting with
a letter or digit. A tool or skill whose advertised name breaks the same rule
is not registered, and the manifest publishes each description as one line of
at most 200 characters.

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

- `url`: agent's A2A base URL (for example `http://agent.local:8765`); the
  client appends `/a2a` for JSON-RPC calls

Optional fields:

- `card`: override `/.well-known/agent.json` discovery URL
- `auth.type`: `bearer` for token auth
- `auth.token_env`: environment variable containing the bearer token
- `timeout_secs`: per-request timeout
- `poll_interval_ms`: task-status polling interval
- `scope.allow`: remote skills to expose; an empty list exposes all discovered skills
- `scope.block`: parsed block patterns; the current A2A pool does not consult
  this field

Example:

```toml
[a2a.research-agent]
url = "http://localhost:8766"
card = "http://localhost:8766/.well-known/agent.json"
timeout_secs = 300
poll_interval_ms = 2000
auth.type = "bearer"
auth.token_env = "RESEARCH_AGENT_TOKEN"
scope.allow = ["web_search", "summarize"]
scope.block = []
```

## `a2a_server`

Enable Servitor to receive structured tasks from external A2A agents.

- `enabled`: toggle the A2A server
- `bind`: listen address (for example `127.0.0.1:8765`)
- `name`: agent name in the published AgentCard
- `description`: agent description
- `task_timeout_secs`: max execution time per task
- `max_concurrent_tasks`: bounded in-flight tasks

Outside open mode, `tasks/send`, `tasks/get` and `tasks/cancel` carry a
keeper's HTTP bearer token (`http_token` in `authority.toml`); the AgentCard
at `/.well-known/agent.json` stays public. For `tasks/send`, the keeper's
`skills` patterns match the registered `provider:tool`, as on every other
execution path, so `shell:*` authorizes the skill the AgentCard advertises as
`shell_execute`. `tasks/get` and `tasks/cancel` reach only the keeper's own
tasks; any other task reads as not found.

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
- `trace_span_arguments`: accepts only `false` (the default); `true` fails at
  load, because published spans carry public metadata only

Servitor no longer owns planning or multi-turn reasoning. Any older
planner-oriented fields should be treated as legacy residue, not part of the
active contract.

When `publish_trace_spans = true`, Servitor emits task-level and per-tool
`trace_span` messages. When an authorized SSE request's signed envelope
carries top-level `trace_id` / `span_id` fields with identifier syntax (8 to
64 hex digits or hyphens), Servitor joins that trace instead of starting an
unrelated one. Trace keys inside task content are ignored, and every other
task starts a fresh trace.

Spans are built exclusively from approved public metadata: generated span and
execution identifiers, the registered provider and tool, the call's position
in the plan, timing, and an outcome. Tool output belongs to the task result
and detailed error text to the local log; the protocol reference lists every
span field. Capturing arguments for debugging is
planned as a local-only diagnostics mode that writes to this machine and never
to the feed.

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

Each published probe record names the tool and whether the probe succeeded.
Probe output is reduced through the same output-defense pipeline used for
direct tool execution so the published snapshot remains planner-facing; a
failed probe call publishes a bounded `error_code`. Probe arguments stay in
this configuration, and error text goes to the local log.

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
- Request authorization uses `request:<task_type>` for SSE tasks
- Assignment delegation uses `assign:<task_type>`

Without `authority.toml`, daemon execution refuses work unless
`--insecure` is supplied explicitly.
