//! Telemetry published to the feed.
//!
//! Exported execution telemetry is built exclusively from approved public
//! metadata. A [`TraceSpan`] is made only from a [`ToolSpanRecord`] or an
//! [`ExecutionSpanRecord`], and every field of those records has a closed
//! type: a [`TelemetryId`], a [`RegisteredToolId`], a timestamp, a count or
//! position, or an outcome enum. None takes a `serde_json::Value`, a
//! `String`, tool arguments, tool output or error text, so a span has no
//! slot for payload. Text cannot become a [`TelemetryId`] or a
//! [`RegisteredToolId`] either: their fields are private.
//!
//! The types are the API. A span exposes no attribute map:
//!
//! ```compile_fail,E0616
//! # use servitor::egregore::telemetry::{ExecutionSpanRecord, TelemetryId, TraceSpan, TraceSpanStatus};
//! # let now = chrono::Utc::now();
//! let mut span = TraceSpan::execution(ExecutionSpanRecord {
//!     trace_id: TelemetryId::generate(),
//!     span_id: TelemetryId::generate(),
//!     parent_span_id: None,
//!     execution_id: TelemetryId::generate(),
//!     tool_count: 1,
//!     start: now,
//!     end: now,
//!     status: TraceSpanStatus::Ok,
//! });
//! span.attributes.insert("arguments".to_string(), serde_json::json!({}));
//! ```
//!
//! An identifier cannot be made from arbitrary text:
//!
//! ```compile_fail,E0423
//! # use servitor::egregore::telemetry::TelemetryId;
//! let id = TelemetryId("mysql -phunter2".to_string());
//! ```
//!
//! and only the MCP pool names a tool:
//!
//! ```compile_fail,E0451
//! # use servitor::mcp::RegisteredToolId;
//! let tool = RegisteredToolId { provider: "shell".into(), tool: "execute".into() };
//! ```
//!
//! whereas the same span, built only from its record, compiles:
//!
//! ```
//! # use servitor::egregore::telemetry::{ExecutionSpanRecord, TelemetryId, TraceSpan, TraceSpanStatus};
//! # let now = chrono::Utc::now();
//! let span = TraceSpan::execution(ExecutionSpanRecord {
//!     trace_id: TelemetryId::generate(),
//!     span_id: TelemetryId::generate(),
//!     parent_span_id: TelemetryId::inherit("00f067aa0ba902b7"),
//!     execution_id: TelemetryId::generate(),
//!     tool_count: 1,
//!     start: now,
//!     end: now,
//!     status: TraceSpanStatus::Ok,
//! });
//! assert_eq!(span.name(), "direct_execution");
//! ```
//!
//! Every timestamp and generated identifier comes from [`now`] and
//! [`TelemetryId::generate`], so tests can freeze both and compare published
//! bytes exactly. Types bound what a span can hold; the information-flow
//! tests check that what it does hold ignores payload.

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::mcp::RegisteredToolId;

/// Name of the service in every span this crate publishes.
const SERVICE: &str = "servitor";

/// A trace, span or execution identifier.
///
/// Either generated ([`TelemetryId::generate`]: 32 lowercase hex digits,
/// independent of any task content) or inherited from a task's trace context
/// when the candidate has identifier syntax ([`TelemetryId::inherit`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct TelemetryId(String);

impl TelemetryId {
    /// A fresh identifier.
    pub fn generate() -> Self {
        Self(generate_id())
    }

    /// Reuse an identifier from a task's trace context: 8 to 64 characters,
    /// all hex digits or hyphens, the shape of UUIDs and W3C trace ids.
    /// Anything else is rejected rather than published.
    pub fn inherit(candidate: &str) -> Option<Self> {
        let is_id = (8..=64).contains(&candidate.len())
            && candidate
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() || byte == b'-');
        is_id.then(|| Self(candidate.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::ops::Deref for TelemetryId {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

/// Execution outcome for a trace span.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TraceSpanStatus {
    Ok,
    Error,
    Timeout,
}

/// How one tool call ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolOutcome {
    /// The tool returned a result.
    Ok,
    /// The tool ran and reported an error result.
    ToolError,
    /// The call failed before the tool returned: transport, validation or
    /// an open circuit.
    CallError,
}

impl ToolOutcome {
    fn status(self) -> TraceSpanStatus {
        match self {
            Self::Ok => TraceSpanStatus::Ok,
            Self::ToolError | Self::CallError => TraceSpanStatus::Error,
        }
    }
}

/// Everything a tool-call span may say.
#[derive(Debug, Clone)]
pub struct ToolSpanRecord {
    pub trace_id: TelemetryId,
    pub span_id: TelemetryId,
    /// The execution span this call belongs to.
    pub parent_span_id: TelemetryId,
    pub execution_id: TelemetryId,
    pub tool: RegisteredToolId,
    /// Position of the call in the task's plan.
    pub call_index: usize,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub outcome: ToolOutcome,
}

/// Everything an execution span, the parent of a task's tool spans, may say.
#[derive(Debug, Clone)]
pub struct ExecutionSpanRecord {
    pub trace_id: TelemetryId,
    pub span_id: TelemetryId,
    /// The span that requested the task, when its trace context names one.
    pub parent_span_id: Option<TelemetryId>,
    pub execution_id: TelemetryId,
    /// Number of planned calls in the task.
    pub tool_count: usize,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub status: TraceSpanStatus,
}

/// A published `trace_span` message, built only from a span record.
#[derive(Debug, Clone, Serialize)]
pub struct TraceSpan {
    #[serde(rename = "type")]
    msg_type: &'static str,
    trace_id: TelemetryId,
    span_id: TelemetryId,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_span_id: Option<TelemetryId>,
    name: String,
    service: &'static str,
    start_ts: DateTime<Utc>,
    end_ts: DateTime<Utc>,
    status: TraceSpanStatus,
    attributes: SpanAttributes,
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
enum SpanAttributes {
    Tool {
        execution_id: TelemetryId,
        provider: String,
        tool: String,
        call_index: usize,
        duration_ms: u64,
        outcome: ToolOutcome,
    },
    Execution {
        execution_id: TelemetryId,
        mode: &'static str,
        tool_count: usize,
        duration_ms: u64,
    },
}

impl TraceSpan {
    /// The span for one tool call, named `direct:<provider>:<tool>`.
    pub fn tool(record: ToolSpanRecord) -> Self {
        Self {
            msg_type: "trace_span",
            trace_id: record.trace_id,
            span_id: record.span_id,
            parent_span_id: Some(record.parent_span_id),
            name: format!("direct:{}", record.tool.public_label()),
            service: SERVICE,
            start_ts: record.start,
            end_ts: record.end,
            status: record.outcome.status(),
            attributes: SpanAttributes::Tool {
                execution_id: record.execution_id,
                provider: record.tool.provider().to_string(),
                tool: record.tool.public_tool().to_string(),
                call_index: record.call_index,
                duration_ms: duration_ms(record.start, record.end),
                outcome: record.outcome,
            },
        }
    }

    /// The span for one direct execution, named `direct_execution`.
    pub fn execution(record: ExecutionSpanRecord) -> Self {
        Self {
            msg_type: "trace_span",
            trace_id: record.trace_id,
            span_id: record.span_id,
            parent_span_id: record.parent_span_id,
            name: "direct_execution".to_string(),
            service: SERVICE,
            start_ts: record.start,
            end_ts: record.end,
            status: record.status,
            attributes: SpanAttributes::Execution {
                execution_id: record.execution_id,
                mode: "direct",
                tool_count: record.tool_count,
                duration_ms: duration_ms(record.start, record.end),
            },
        }
    }

    pub fn trace_id(&self) -> &TelemetryId {
        &self.trace_id
    }

    pub fn span_id(&self) -> &TelemetryId {
        &self.span_id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn status(&self) -> TraceSpanStatus {
        self.status
    }
}

fn duration_ms(start: DateTime<Utc>, end: DateTime<Utc>) -> u64 {
    u64::try_from((end - start).num_milliseconds()).unwrap_or(0)
}

/// Current time for an exported telemetry record.
pub fn now() -> DateTime<Utc> {
    #[cfg(test)]
    if let Some(at) = frozen::tick() {
        return at;
    }
    Utc::now()
}

fn generate_id() -> String {
    #[cfg(test)]
    if let Some(id) = frozen::next_id() {
        return id;
    }
    uuid::Uuid::new_v4().simple().to_string()
}

#[cfg(test)]
pub(crate) use frozen::freeze;

#[cfg(test)]
mod frozen {
    use std::cell::RefCell;

    use chrono::{DateTime, Duration, Utc};

    struct State {
        at: DateTime<Utc>,
        next_id: u64,
    }

    thread_local! {
        static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    }

    /// Freeze telemetry on this thread: [`super::now`] starts at `at` and
    /// advances 1ms per call, and [`super::generate_id`] counts up from 1.
    /// The clock thaws when the guard drops.
    pub(crate) fn freeze(at: DateTime<Utc>) -> Frozen {
        STATE.with(|state| *state.borrow_mut() = Some(State { at, next_id: 1 }));
        Frozen(())
    }

    pub(crate) struct Frozen(());

    impl Drop for Frozen {
        fn drop(&mut self) {
            STATE.with(|state| *state.borrow_mut() = None);
        }
    }

    pub(super) fn tick() -> Option<DateTime<Utc>> {
        STATE.with(|state| {
            state.borrow_mut().as_mut().map(|state| {
                let at = state.at;
                state.at += Duration::milliseconds(1);
                at
            })
        })
    }

    pub(super) fn next_id() -> Option<String> {
        STATE.with(|state| {
            state.borrow_mut().as_mut().map(|state| {
                let id = format!("{:032x}", state.next_id);
                state.next_id += 1;
                id
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frozen_clock_repeats_exactly_and_thaws() {
        let at = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let run = || {
            let _frozen = freeze(at);
            (now(), now(), generate_id(), generate_id())
        };

        let first = run();
        assert_eq!(first, run());
        assert_eq!(first.1 - first.0, chrono::Duration::milliseconds(1));
        assert_eq!(first.2, format!("{:032x}", 1));

        assert_ne!(generate_id(), generate_id());
    }

    #[test]
    fn inherited_ids_need_identifier_syntax() {
        for id in [
            "4bf92f3577b34da6a3ce929d0e0e4736",
            "00f067aa0ba902b7",
            "123e4567-e89b-12d3-a456-426614174000",
        ] {
            assert_eq!(TelemetryId::inherit(id).unwrap().as_str(), id);
        }
        for text in ["", "abc", "trace-123", "mysql -phunter2", &"a".repeat(65)] {
            assert_eq!(TelemetryId::inherit(text), None, "{text}");
        }
    }
}
