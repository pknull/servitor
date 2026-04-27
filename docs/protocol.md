# Servitor Protocol

Servitor participates in two closely related message flows on Egregore:

1. direct execution flows, used by hook mode, cron tasks, watcher-triggered tasks, and local CLI execution
2. coordinated network execution, used when `servitor run` subscribes to the Egregore SSE feed and participates in offer/assignment

All Egregore messages are signed by the publishing node. Servitor adds local authorization and scope enforcement before offering or executing work.

## Task Envelope

Incoming `task` messages support both legacy and current fields:

- `hash`: sender-computed task hash
- `id`: stable task identifier; defaults to `hash` when omitted
- `task_type`: authorization and assignment class; falls back to the first required capability when omitted
- `request`: original human-readable request text; falls back to `prompt`
- `requestor`: intended requester identity; defaults to the envelope author
- `required_caps`: coarse capability filter
- `scope_override`: optional per-task restriction that can only narrow the configured scope policies
- `tool_calls`: pre-planned tool calls for direct execution

Servitor normalizes these optional fields before the task enters downstream execution logic.
When the surrounding Egregore envelope carries top-level `trace_id` or
`span_id` fields, Servitor copies them into task context when the task itself
does not already provide them.

## Coordinated SSE Flow

When `servitor run` is subscribed to Egregore SSE, the current lifecycle is:

1. Requestor publishes `task`
2. Servitor checks capability match and `request:<task_type>` permission
3. Eligible servitor publishes `task_offer`
4. Requestor or authorized assigner publishes `task_assign`
5. Assigned servitor publishes `task_started`
6. Requestor may publish `task_ping`
7. Servitor replies with `task_status` while work is still active
8. Servitor publishes either `task_result` or `task_failed`
9. If no assignment arrives before the offer TTL expires, servitor publishes `task_offer_withdraw`

Assignment authorization rules:

- the original requestor may always assign their own task
- any other assigner must be authorized for `assign:<task_type>`
- unauthorized offers and assignments emit `auth_denied`

When trace propagation is enabled, offer, start, status, failure, and result
messages reuse the inherited task trace identifier when present.

## Direct, Hook, Cron, and Watcher Flow

The simpler execution path is used outside SSE assignment:

1. Task enters from hook stdin, cron, watcher expansion, or direct execution
2. Servitor may publish an advisory `task_claim`
3. Servitor executes the task locally
4. Servitor publishes `task_result`

`task_claim` is advisory only. It is not the coordination mechanism for SSE task assignment.

Hook mode uses the same request gate as SSE offer evaluation:

- Servitor normalizes the task using the envelope author
- the normalized `requestor` must match the envelope author
- the requestor must be authorized for `request:<task_type>`

For local `servitor exec`, the input is a JSON array of pre-planned tool calls, not a natural-language planning request.

## Message Types

| Message | Direction | Purpose |
|---------|-----------|---------|
| `servitor_profile` | Out | Capability advertisement and heartbeat |
| `servitor_manifest` | Out | Planner-facing executor manifest |
| `environment_snapshot` | Out | Target-specific planner-facing state |
| `task` | In | Work request |
| `task_claim` | Out | Advisory claim for non-assignment paths |
| `task_offer` | Out | Servitor offers to execute a task |
| `task_assign` | In | Requestor or assigner selects a servitor |
| `task_started` | Out | Execution acknowledged with ETA |
| `task_ping` | In | Request status for active work |
| `task_status` | Out | Progress or revised ETA |
| `task_failed` | Out | Structured task failure |
| `task_offer_withdraw` | Out | Offer expired before assignment |
| `task_result` | Out | Final executor result payload |
| `auth_denied` | Out | Authorization denial audit event |
| `trace_span` | Out | Opt-in execution tracing |
| `notification` | Out | Outbound operator notification payload |

## Task Result Payload

`task_result` is the final executor result payload. Servitor computes
`result_hash` over a canonical payload containing:

- `task_id`
- `servitor`
- `correlation_id`
- `task_hash`
- `status`
- `result`
- `error`
- `duration_seconds`
- `trace_id`

`result_hash` is result metadata published through the local egregore node. The
feed-level signature comes from the node's message envelope; there is no second
Servitor signature field in the active model.

## Profiles and Heartbeats

`servitor_profile` is published on startup and heartbeat cadence.
`servitor_manifest` is published on startup to give planners a richer but
still curated view of the executor.

Always included:

- `servitor_id`
- `capabilities`
- `tools`
- `scopes`
- `resource_limits`
- `heartbeat_interval_ms`
- `version`
- `roles`
- `labels`
- `manifest_ref`
- `target_summary`

Only when `heartbeat.include_runtime_monitoring = true`:

- `uptime_secs`
- `mcp_servers`
- `load`
- `stats`
- `last_task_ts`

The current runtime publishes `servitor_manifest` with grouped toolsets and
operator-curated deployment targets, and `environment_snapshot` messages for
configured targets. Snapshot publication is driven by
`profile.targets[*].snapshot_tool_calls`; if no probes are configured, the
snapshot still publishes a configured-only target view.

Probe arguments are sanitized before inclusion in `environment_snapshot`, and
probe output passes through the same output-defense pipeline used for direct
tool execution.

## Tracing

Detailed `trace_span` messages are disabled by default. Set `agent.publish_trace_spans = true` to emit task and tool spans.
