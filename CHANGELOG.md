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
- `agent.trace_span_arguments` (default `false`) attaches each tool call's
  sanitized arguments to its tool `trace_span` as an `arguments` attribute,
  capped at 4KB serialized. It has no effect unless `publish_trace_spans` is
  also enabled; with it off, tool spans are unchanged.
- Argument sanitization scans string values for credential-shaped content in
  addition to field names: authorization headers, bearer tokens, URL
  passwords, secret query parameters, `name=value` and `--flag value`
  secrets, known token formats, PEM private keys, and high-entropy tokens.
  Values are scanned before the 1KB truncation.

### Changed

- Cross-component reference only: Familiar now binds `task_offer.servitor` to
  the offer's signing author and fails closed unless assignment trust is
  explicitly configured; this does not add a Familiar-side execution path to
  Servitor.
- `environment_snapshot` probe arguments pass through the same value scan, so
  they may now redact more than before (never less).

### Fixed

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
