# Changelog

All notable changes to servitor are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and this crate's pre-1.0 versioning treats minor bumps as the breaking-change signal.

## [Unreleased] - 2026-08-14

### Added

- Startup recovery queries a bounded feed window and replays identity-bound,
  offered-but-unresolved task chains through the normal authority path,
  including accepted-but-unexecuted typed assignments.
- Servitor now owns the RFC 0003 `AssignTaskV1` path: it validates typed
  operator assignments fail closed, durably publishes `AssignTaskResultV1`
  before execution, and suppresses replayed `command_id` mutations using
  recorded owner results.
- `task_failed` carries a bounded `code` (`missing_tool_calls`,
  `unknown_tool`, `unauthorized`, `scope_blocked`, `scope_not_allowed`,
  `invalid_scope_override`, `execution_timeout`, `execution_error`) alongside
  a `details` sentence naming the call position, registered tool and scope
  rule by list and index.
- Tool spans carry `execution_id`, `provider`, `tool`, `call_index`,
  `duration_ms` and `outcome` (`ok`, `tool_error`, `call_error`); the
  execution span carries `execution_id`, `mode`, `tool_count` and
  `duration_ms`.
- `environment_snapshot` probe failures carry a bounded `error_code`.
- Argument sanitization scans string values for credential-shaped content in
  addition to field names: authorization headers, bearer tokens, URL
  passwords, secret query parameters, `name=value` and `--flag value`
  secrets, known token formats, PEM private keys, and high-entropy tokens.
  Values are scanned before the 1KB truncation.

### Changed

- Exported execution telemetry is built exclusively from approved public
  metadata. `trace_span` messages are built from a closed record of generated
  identifiers, the registered tool, timing and outcome; arguments, output and
  error text have no field on a span. `agent.trace_span_arguments = true` now
  fails at config load; `false` or absent still load. Argument capture for
  debugging is planned as a local-only diagnostics mode.
- The execution span drops `task_hash`, which `servitor exec` and event
  templates derive from argument values, and the always-empty `events` list.
- Inherited trace and span identifiers are reused only with identifier syntax
  (8 to 64 hex digits or hyphens); other values are dropped rather than
  published.
- A malformed task `scope_override` rule refuses the call as
  `invalid_scope_override` instead of a configuration error.
- `environment_snapshot` probe records drop their `arguments` echo and their
  free-text `error`; an unregistered probe tool is reported as
  `unknown_tool` without its configured name.

- Cross-component reference only: Familiar now binds `task_offer.servitor` to
  the offer's signing author and fails closed unless assignment trust is
  explicitly configured; this does not add a Familiar-side execution path to
  Servitor.

### Fixed

- Scope rejections published the denied call's whole `command` argument in
  `task_failed.details`, so a denied command carrying a credential reached
  the feed. `task_failed` now publishes bounded reason codes, and the full
  error text goes to the local log through the credential scanner. Unknown
  tool names, raw snapshot probe errors and input-schema rejections (which
  quote argument values) are reported by code the same way.

- Feed history queries use Egregore's current `/v1/feed` endpoint with
  `include_self=true` and bounded pagination, replacing the obsolete
  `/v1/query` path so startup recovery and assignment-result checks can see
  local owner records.
- Output defense no longer panics when an oversized tool output has a
  multibyte character at the 8KB cut, and a task scope override pattern over
  the length limit no longer panics when a multibyte character sits at the
  error preview cut. Both truncate on a character boundary.

## [0.3.0] - 2026-04-27

### ⚠ Breaking

- **`Attestation` struct removed.** `egregore::messages::TaskResult` no longer carries an `attestation: Attestation` field, and the `Attestation` struct itself is gone from the public API. Servitor publishes execution metadata through the local Egregore node, which is now the sole network signing principal in the post-reconciliation contract; feed-level signing comes from the node's message envelope. Downstream code parsing `task_result` payloads must drop the `attestation` field.
- **Hook + daemon authorization gate now uses `request:<task_type>`.** The legacy `*` wildcard skill check has been replaced by the same `request:<task_type>` skill string SSE mode already used. Hook mode also now requires the normalized task `requestor` to match the Egregore envelope `author` before execution proceeds. Existing `authority.toml` keepers granted only the wildcard `*` skill will need an explicit `request:*` (or per-task-type) entry.
- **`build_signed_result` renamed to `build_result`** (internal but exposed in error paths via `build_error_result`).

### Added

- `task::inherit_trace_context()` copies top-level Egregore envelope `trace_id` and `span_id` into task context when absent. `Task::context_trace_id()`, `context_span_id()`, `context_parent_span_id()` helpers.
- `EgregoreClient::publish_*_with_trace()` variants for offer / started / status / failed / result / withdraw.
- `execute_direct` honours inherited `trace_id` and `parent_span_id`, parenting Servitor's root span to the upstream Familiar trace.
- `compute_result_hash` payload widened to cover `task_id`, `servitor`, `correlation_id`, `task_hash`, `status`, `result`, `error`, `duration_seconds`, and `trace_id`. `result_hash` is now a content-addressed identifier for the full task-result lifecycle.
- Output defense extended to error paths: `defense_pipeline` + `sanitize_tool_result` are applied to direct-call tool errors so credentials don't leak into `task_failed` payloads.
- Output defense extended to `environment_snapshot` probes: `sanitize_arguments` + `defense_pipeline` are applied to probe outputs before publication.
- `CONTRIBUTING.md`, dual `LICENSE-APACHE` / `LICENSE-MIT`.

### Changed

- `docs/README.md` restructured as the umbrella mdBook stub for Servitor, matching the egregore + familiar pattern. Detailed protocol, configuration, operations, and HTTP API material continues to ship from the subproject's `docs/`.

## [0.2.0] - prior

Earlier history is preserved in `git log`. Highlights: direct-only execution with LLM removal, MCP notifications, manifest projection, runtime depth (`depends_on`, manifest heartbeat republish).
