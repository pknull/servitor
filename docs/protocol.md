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

### Assignment command families

Bare `task_assign` remains supported as Familiar's automated assignment path.
The typed `assign_task/v1` family is the operator-command path used for an
explicit assignment decision. In the current wire representation these RFC
families use `type: "assign_task"` and `type: "assign_task_result"`, backed by
the `AssignTaskV1` and `AssignTaskResultV1` schemas.

An `assign_task/v1` command binds:

- `task_id`: stable task identifier
- `task_hash`: feed hash of the task message
- `offer_hash`: feed hash of the selected offer
- `executor`: target Servitor identity
- `assigner`: authenticated assigner identity, which must equal the command
  message's signing author
- `predecessor`: optional expected latest lifecycle hash
- `command_id`: stable command idempotency key
- `attempt_id`: identity of this delivery attempt
- `expires_at`: command expiry timestamp

Servitor owns the state transition and validates the command fail closed. Its
current rejection reasons are:

| Condition | `assign_task_result/v1.reason` |
|-----------|--------------------------------|
| `assigner` does not match the command message author | `assigner_mismatch` |
| `expires_at` is in the past | `expired` |
| The task is active, already accepted, completed, or failed | `task_resolved` |
| The pending task or offer is unknown/withdrawn, or `offer_hash` does not match | `offer_unavailable` |
| `task_hash` does not match the offered task | `task_mismatch` |
| A supplied `predecessor` is not the latest lifecycle hash | `stale_predecessor` |
| The assigner lacks assignment authority | `unauthorized` |

A command addressed to another executor is ignored rather than answered. For
an accepted command, Servitor durably publishes `assign_task_result/v1`
**before** it mutates the owner state or begins execution. The result contains
`command_id`, `task_id`, `status`, and optional `reason`, `lifecycle_hash`, and
`task_state`. The status domain is `accepted | rejected | superseded`; the
current owner path emits `accepted` or `rejected`.

`command_id` provides recorded-result idempotency. Before validation, Servitor
queries its owner-authored assignment results. If that command id already has
a recorded result, replay produces neither another decision record nor another
execution; the existing result remains authoritative.

When trace propagation is enabled, offer, start, status, failure, and result
messages reuse the inherited task trace identifier when present. An inherited
identifier is reused when it has identifier syntax: 8 to 64 hex digits or
hyphens, the shape of UUIDs and W3C trace ids.

## Task Failure Payload

`task_failed` carries `task_id`, `servitor`, `reason`
(`execution_error` or `timeout`), `timestamp`, a bounded `code`, and a
`details` sentence built from public metadata: the failing call's position in
the plan, the registered tool (`<server>:<tool>`), and a scope rule named by
its list and index. Policy rule indices refer to the server's configured
`scope.allow` and `scope.block` lists, which stay in the executor's local
configuration and are never published; override rule indices refer to the
task's own `scope_override` lists. The full error text stays in the executor's
local log.

| `code` | `details` |
|--------|-----------|
| `missing_tool_calls` | The task carried no pre-planned `tool_calls` |
| `unknown_tool` | `tool call <i> names no registered tool` |
| `unauthorized` | `tool call <i> (<tool>) is not authorized for the requesting keeper` |
| `scope_blocked` | `tool call <i> (<tool>) blocked by policy block rule <n>` (or `task scope override block rule <n>`) |
| `scope_not_allowed` | `tool call <i> (<tool>) not permitted by any policy allow rule` (or `task scope override allow rule`) |
| `invalid_scope_override` | `tool call <i> (<tool>) refused: task scope override <allow\|block> rule <n> is invalid` |
| `execution_timeout` | `task exceeded <n>s execution timeout` |
| `execution_error` | `task execution failed` |

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
| `assign_task/v1` (`type: "assign_task"`) | In | Typed operator assignment command |
| `assign_task_result/v1` (`type: "assign_task_result"`) | Out | Durable owner acceptance or rejection |
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

Each probe record in `environment_snapshot` names the registered tool and
whether the probe succeeded. A successful or tool-reported result carries the
probe output, reduced through the same output-defense pipeline used for direct
tool execution; a failed call carries a bounded `error_code` (`unknown_tool`,
`invalid_arguments`, `server_unavailable`, `timeout`, `call_failed`). Probe
arguments stay in configuration, and error text goes to the executor's local
log.

## Tracing

Detailed `trace_span` messages are disabled by default. Set `agent.publish_trace_spans = true` to emit task and tool spans.

Spans are built exclusively from approved public metadata. Every span carries
`trace_id`, `span_id`, `parent_span_id` when there is one, `name`, `service`
(`servitor`), `start_ts`, `end_ts`, `status` (`ok`, `error`, `timeout`), and
`attributes`:

| Span | `name` | `attributes` |
|------|--------|--------------|
| Tool call | `direct:<server>:<tool>` | `execution_id`, `provider`, `tool`, `call_index`, `duration_ms`, `outcome` (`ok`, `tool_error`, `call_error`) |
| Execution | `direct_execution` | `execution_id`, `mode` (`direct`), `tool_count`, `duration_ms` |

`execution_id` and span identifiers are generated per execution, and the
provider and tool are the names the MCP pool registered. Tool output belongs
to the task result, and detailed error text to the executor's local log.
