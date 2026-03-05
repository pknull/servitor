//! Scope enforcement.
//!
//! Enforces allow/block policies on MCP tool calls. Block patterns
//! take precedence over allow patterns.

pub mod matcher;
pub mod policy;

pub use policy::{ScopeEnforcer, ScopePolicy};
