//! Egregore network integration.
//!
//! Handles communication with the egregore decentralized network:
//! - Receiving tasks via hook (stdin JSON)
//! - Publishing messages via HTTP API
//! - Fetching context via thread queries
//! - Message schemas for Servitor protocol

pub mod context;
pub mod hook;
pub mod messages;
pub mod profile;
pub mod publish;

pub use context::ConversationTurn;
pub use hook::{parse_message, receive_message};
pub use messages::*;
pub use profile::{build_environment_snapshots, build_manifest, build_profile};
pub use publish::EgregoreClient;
