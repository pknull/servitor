//! Egregore network integration.
//!
//! Handles communication with the egregore decentralized network:
//! - Publishing messages via HTTP API
//! - Fetching context via thread queries
//! - Message schemas for Servitor protocol

pub mod context;
pub mod messages;
pub mod profile;
pub mod publish;
pub mod telemetry;

pub use context::ConversationTurn;
pub use messages::*;
pub use profile::{build_environment_snapshots, build_manifest, build_profile};
pub use publish::EgregoreClient;
pub use telemetry::{TraceSpan, TraceSpanStatus};
