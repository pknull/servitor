# Servitor Protocol

Servitor participates in two closely related message flows on egregore:

1. Direct execution flows, used by hook mode, cron tasks, watcher-triggered
   tasks, and some local execution paths
2. Coordinated network execution, used when `servitor run` subscribes to the
   egregore SSE feed and participates in offer/assignment

All egregore messages are signed by the publisher. Servitor adds local
authorization on top of that signature layer before offering or executing work.

## Task Envelope

Incoming `task` messages support both legacy and current fields:

- `hash`: sender-computed task hash
- `id`: stable task identifier; defaults to `hash` when omitted
- `task_type`: authorization and assignment class; falls back to the first
  required capability when omitted
- `request`: original human-readable request text; falls back to `prompt`
- `requestor`: intended requester identity; defaults to the envelope author
- `required_caps`: coarse capability filter
- `scope_override`: optional per-task restriction that can only narrow the
  configured scope policies

Servitor normalizes these optional fields before the task enters downstream
execution logic.

## Coordinated SSE Flow

When `servitor run` is subscribed to egregore SSE, the current lifecycle is:

1. Requestor publishes `task`
2. Servitor checks capability match and `request:<task_type>` permission
3. Eligible servitor publishes `task_offer`
4. Requestor or authorized assigner publishes `task_assign`
5. Assigned servitor publishes `task_started`
6. Requestor may publish `task_ping`
7. Servitor replies with `task_status` while work is still active
8. Servitor publishes either `task_result` or `task_failed`
9. If no assignment arrives before the offer TTL expires, servitor publishes
   `task_offer_withdraw`

Assignment authorization rules:

- The original requestor may always assign their own task
- Any other assigner must be authorized for `assign:<task_type>`
- Unauthorized offers and assignments emit `auth_denied`

## Direct, Hook, Cron, and Watcher Flow

The simpler execution path is still used outside SSE assignment:

1. Task enters from hook stdin, cron, watcher expansion, or direct execution
2. Servitor may publish an advisory `task_claim`
3. If `--plan-first` is used, servitor publishes a `task_plan`
4. Servitor executes the task locally
5. Servitor publishes `task_result`

`task_claim` is still part of the runtime for these paths, but it is advisory
only. It is not the coordination mechanism for SSE task assignment.

## Message Types

| Message | Direction | Purpose |
|---------|-----------|---------|
| `servitor_profile` | Out | Capability advertisement and heartbeat |
| `task` | In | Work request |
| `task_claim` | Out | Advisory claim for non-assignment paths |
| `task_offer` | Out | Servitor offers to execute a task |
| `task_assign` | In | Requestor or assigner selects a servitor |
| `task_started` | Out | Execution acknowledged with ETA |
| `task_ping` | In | Request status for active work |
| `task_status` | Out | Progress or revised ETA |
| `task_failed` | Out | Structured task failure |
| `task_offer_withdraw` | Out | Offer expired before assignment |
| `task_result` | Out | Signed final attestation and result payload |
| `task_plan` | Out | Optional pre-execution plan artifact |
| `auth_denied` | Out | Authorization denial audit event |
| `trace_span` | Out | Opt-in execution tracing |
| `notification` | Out | Outbound operator notification payload |

## Profiles and Heartbeats

`servitor_profile` is published on startup and heartbeat cadence.

Always included:

- `servitor_id`
- `capabilities`
- `tools`
- `scopes`
- `resource_limits`
- `heartbeat_interval_ms`
- `version`

Only when `heartbeat.include_runtime_monitoring = true`:

- `uptime_secs`
- `mcp_servers`
- `load`
- `stats`
- `last_task_ts`

## Planning and Tracing

`servitor exec --dry-run` performs local planning and validation without
execution and does not publish the plan.

`servitor exec --plan-first` publishes a signed `task_plan`, then executes and
binds the resulting `task_result.plan_hash` to that plan.

Detailed `trace_span` messages are disabled by default. Set
`agent.publish_trace_spans = true` to emit task and tool spans.
