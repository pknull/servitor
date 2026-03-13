//! Memory storage layer for personal/hybrid modes.
//!
//! Provides local task queue, result storage, and conversation caching.
//! Enables offline operation with sync to egregore when online.

pub mod hybrid;
pub mod sqlite;
pub mod store;

pub use hybrid::HybridStore;
pub use sqlite::SqliteStore;
pub use store::{ConversationTurn, MemoryStore, StoredResult, StoredTask};
