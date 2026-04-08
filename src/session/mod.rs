//! Session management for Servitor.
//!
//! Sessions track interactions with keepers across transports (A2A, CLI, Egregore).
//! Each session maintains:
//! - A JSONL transcript of all messages
//! - Pending task correlations (delegated work awaiting response)
//! - Transport context for reply routing
//!
//! ## Architecture
//!
//! ```text
//! ~/.servitor/
//! ├── sessions/
//! │   ├── <session_id>.jsonl     # Append-only transcript
//! │   └── index.sqlite           # Session metadata + correlations
//! ```
//!
//! ## Session Lifecycle
//!
//! 1. Task arrives from transport (A2A, CLI, egregore)
//! 2. Session created or resumed based on (keeper, channel, transport)
//! 3. Transcript appended with user message
//! 4. If task delegated to external agent, correlation stored
//! 5. Egregore watcher monitors for task_result with matching `relates`
//! 6. On completion, keeper notified via original transport

mod store;
mod transcript;
mod types;
mod watcher;

pub use store::SessionStore;
pub use transcript::{TranscriptEntry, TranscriptWriter};
pub use types::{PendingTask, Session, SessionId, SessionState, Transport};
pub use watcher::{TaskCompletionEvent, TaskResultInfo, TaskWatcher, TaskWatcherHandle};
