//! Agent card generation for the A2A server.
//!
//! Builds an A2A agent card from the servitor's MCP tools and A2A pool skills,
//! presenting them as capabilities to external agents.

use crate::a2a::card::{AgentCard, AuthScheme, Authentication, Skill};
use crate::a2a::A2aPool;
use crate::config::A2aServerConfig;
use crate::mcp::McpPool;

/// Build an agent card from servitor capabilities.
///
/// Converts MCP tools and A2A pool skills to A2A skills format.
/// Tool names are prefixed to avoid collisions.
pub fn build_agent_card(
    config: &A2aServerConfig,
    mcp_pool: &McpPool,
    a2a_pool: &A2aPool,
    base_url: &str,
) -> AgentCard {
    let mut skills = Vec::new();

    // Add MCP tools as skills
    for (prefixed_name, tool_def) in mcp_pool.all_tools() {
        skills.push(Skill {
            name: prefixed_name.to_string(),
            description: tool_def.description.clone(),
            input_schema: tool_def.input_schema.clone(),
            output_schema: None,
            input_modes: vec!["application/json".to_string()],
            output_modes: vec!["application/json".to_string(), "text/plain".to_string()],
            tags: vec!["mcp".to_string()],
        });
    }

    // Add A2A pool skills
    for llm_tool in a2a_pool.tools_for_llm() {
        skills.push(Skill {
            name: llm_tool.name.clone(),
            description: llm_tool.description,
            input_schema: Some(llm_tool.input_schema),
            output_schema: None,
            input_modes: vec!["application/json".to_string()],
            output_modes: vec!["application/json".to_string(), "text/plain".to_string()],
            tags: vec!["a2a".to_string()],
        });
    }

    AgentCard {
        name: config.name.clone(),
        description: config.description.clone(),
        version: Some(env!("CARGO_PKG_VERSION").to_string()),
        url: Some(base_url.to_string()),
        skills,
        authentication: Some(Authentication {
            schemes: vec![AuthScheme::Bearer { description: None }],
        }),
        default_input_modes: vec!["application/json".to_string()],
        default_output_modes: vec!["application/json".to_string(), "text/plain".to_string()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_empty_card() {
        let config = A2aServerConfig {
            enabled: true,
            bind: "127.0.0.1:8765".to_string(),
            name: "test-servitor".to_string(),
            description: Some("Test agent".to_string()),
            task_timeout_secs: 300,
            max_concurrent_tasks: 10,
        };

        let mcp_pool = McpPool::new();
        let a2a_pool = A2aPool::new();

        let card = build_agent_card(&config, &mcp_pool, &a2a_pool, "http://localhost:8765");

        assert_eq!(card.name, "test-servitor");
        assert_eq!(card.description, Some("Test agent".to_string()));
        assert!(card.skills.is_empty());
        assert!(card.authentication.is_some());
    }
}
