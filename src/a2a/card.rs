//! A2A Agent Card parsing.
//!
//! Agent Cards are JSON documents that describe an A2A agent's capabilities.
//! They are typically served at `/.well-known/agent.json`.

use serde::{Deserialize, Serialize};

/// A2A Agent Card — describes an agent's capabilities.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCard {
    /// Agent name.
    pub name: String,

    /// Human-readable description.
    #[serde(default)]
    pub description: Option<String>,

    /// Agent version.
    #[serde(default)]
    pub version: Option<String>,

    /// Base URL for agent API.
    #[serde(default)]
    pub url: Option<String>,

    /// Available skills.
    #[serde(default)]
    pub skills: Vec<Skill>,

    /// Authentication requirements.
    #[serde(default)]
    pub authentication: Option<Authentication>,

    /// Default input modes accepted.
    #[serde(default, rename = "defaultInputModes")]
    pub default_input_modes: Vec<String>,

    /// Default output modes produced.
    #[serde(default, rename = "defaultOutputModes")]
    pub default_output_modes: Vec<String>,
}

impl AgentCard {
    /// Find a skill by name.
    pub fn skill(&self, name: &str) -> Option<&Skill> {
        self.skills.iter().find(|s| s.name == name)
    }

    /// Get all skill names.
    pub fn skill_names(&self) -> Vec<&str> {
        self.skills.iter().map(|s| s.name.as_str()).collect()
    }
}

/// A skill that an agent can perform.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    /// Skill identifier.
    pub name: String,

    /// Human-readable description.
    #[serde(default)]
    pub description: Option<String>,

    /// Input schema (JSON Schema).
    #[serde(default, rename = "inputSchema")]
    pub input_schema: Option<serde_json::Value>,

    /// Output schema (JSON Schema).
    #[serde(default, rename = "outputSchema")]
    pub output_schema: Option<serde_json::Value>,

    /// Input modes this skill accepts.
    #[serde(default, rename = "inputModes")]
    pub input_modes: Vec<String>,

    /// Output modes this skill produces.
    #[serde(default, rename = "outputModes")]
    pub output_modes: Vec<String>,

    /// Tags for categorization.
    #[serde(default)]
    pub tags: Vec<String>,
}

impl Skill {
    /// Create a prefixed tool name for LLM consumption.
    pub fn prefixed_name(&self, agent_name: &str) -> String {
        format!("{}_{}", agent_name, self.name)
    }
}

/// Authentication requirements for the agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Authentication {
    /// Supported auth schemes.
    #[serde(default)]
    pub schemes: Vec<AuthScheme>,
}

/// Authentication scheme.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum AuthScheme {
    /// Bearer token authentication.
    Bearer {
        #[serde(default)]
        description: Option<String>,
    },
    /// API key authentication.
    ApiKey {
        #[serde(default)]
        header: Option<String>,
        #[serde(default)]
        description: Option<String>,
    },
    /// OAuth2 authentication.
    OAuth2 {
        #[serde(default, rename = "authorizationUrl")]
        authorization_url: Option<String>,
        #[serde(default, rename = "tokenUrl")]
        token_url: Option<String>,
        #[serde(default)]
        scopes: Vec<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_agent_card() {
        let json = r#"{
            "name": "researcher",
            "description": "Research agent for web searches",
            "version": "1.0.0",
            "skills": [
                {
                    "name": "web_search",
                    "description": "Search the web for information",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "query": { "type": "string" }
                        },
                        "required": ["query"]
                    }
                },
                {
                    "name": "summarize",
                    "description": "Summarize text content"
                }
            ],
            "authentication": {
                "schemes": [
                    { "type": "bearer" }
                ]
            }
        }"#;

        let card: AgentCard = serde_json::from_str(json).unwrap();
        assert_eq!(card.name, "researcher");
        assert_eq!(card.skills.len(), 2);
        assert_eq!(card.skill("web_search").unwrap().name, "web_search");
        assert!(card.skill("nonexistent").is_none());
        assert_eq!(
            card.skills[0].prefixed_name(&card.name),
            "researcher_web_search"
        );
    }

    #[test]
    fn parse_minimal_card() {
        let json = r#"{"name": "minimal", "skills": []}"#;
        let card: AgentCard = serde_json::from_str(json).unwrap();
        assert_eq!(card.name, "minimal");
        assert!(card.skills.is_empty());
    }
}
