# Changelog

All notable changes to servitor are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and this crate's pre-1.0 versioning treats minor bumps as the breaking-change signal.

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
