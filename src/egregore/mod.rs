//! Egregore network integration.
//!
//! Handles communication with the egregore decentralized network:
//! - Receiving tasks via hook (stdin JSON)
//! - Publishing messages via HTTP API
//! - Message schemas for Servitor protocol

pub mod hook;
pub mod messages;
pub mod publish;

pub use hook::{parse_message, receive_message};
pub use messages::*;
pub use publish::EgregoreClient;
