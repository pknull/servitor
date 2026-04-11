# Servitor Operations

## Initial Setup

Create identity and configuration:

```bash
servitor init
cp servitor.example.toml servitor.toml
cp authority.example.toml ~/.servitor/authority.toml
```

Use `servitor info` to inspect the current identity, configured MCP transports,
and authority status.

## Authority Model

Servitor is fail-closed by default.

- If `authority.toml` is missing, daemon and hook modes refuse to execute work
- `--insecure` restores development-only open behavior
- Local `servitor exec` authorizes as the servitor's own egregore identity when
  authority is present

Operationally, keep `authority.toml` alongside the identity directory and treat
it as local policy, not feed-replicated state.

## Runtime Modes

### `servitor exec`

Runs one structured task locally from a JSON array of pre-planned tool calls.

Example:

```bash
servitor exec '[{"name":"shell__execute","arguments":{"command":"hostname"}}]'
```

### `servitor run`

Starts the daemon. Depending on configuration, it can:

- subscribe to egregore SSE tasks
- publish heartbeats
- run scheduled tasks
- poll MCP servers for notifications and route them into structured tasks

### `servitor run --hook`

Reads a single egregore envelope from stdin and executes it as a hook target.

This mode is intended for egregore hook integration rather than long-running
subscription.

## Observability

Default heartbeat/profile publishing includes only capability and scope data.

Enable runtime monitoring explicitly:

```toml
[heartbeat]
include_runtime_monitoring = true
```

Enable tracing explicitly:

```toml
[agent]
publish_trace_spans = true
```

These settings publish more feed data and should be turned on deliberately.

## Scheduling and Watchers

`[[schedule]]` creates time-based tasks from structured `tool_calls`.
`[[watch]]` and `mcp.*.on_notification` use the same structured template shape
for stdio MCP notifications.

## Egregore Integration

Daemon mode consumes egregore SSE and publishes:

- `servitor_profile`
- `servitor_manifest`
- `environment_snapshot`
- `task_offer`
- `task_started`
- `task_status`
- `task_failed`
- `task_result`
- `auth_denied`
- optional `trace_span`

Hook and non-SSE execution paths can still publish advisory `task_claim` and
final `task_result` messages.
