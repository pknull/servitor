//! Scope enforcement for MCP tool calls.
//!
//! Implements allow/block patterns to restrict which operations
//! MCP servers can perform.

pub mod matcher;
pub mod policy;

pub use matcher::ScopeMatcher;
pub use policy::{ScopeEnforcer, ScopePolicy};
