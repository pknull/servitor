//! TOML configuration schema for authority.toml.

use serde::{Deserialize, Serialize};

use super::keeper::Keeper;
use super::permission::Permission;

/// Top-level authority configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuthorityConfig {
    /// Keeper definitions.
    #[serde(default, rename = "keeper")]
    pub keepers: Vec<Keeper>,

    /// Permission grants.
    #[serde(default, rename = "permission")]
    pub permissions: Vec<Permission>,
}

impl AuthorityConfig {
    /// Parse authority configuration from TOML string.
    pub fn from_toml(content: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_authority_config() {
        let toml = r#"
[[keeper]]
name = "pknull"
egregore = "@7JIN8TA3bZ1l786oQ6lPN3l94KEFFH0UlVz9lqTr5+E=.ed25519"
discord = "187488812471943168"

[[keeper]]
name = "automation"
egregore = "@AutomationKey.ed25519"

[[permission]]
keeper = "pknull"
place = "*"
skills = ["*"]

[[permission]]
keeper = "automation"
place = "egregore:local"
skills = ["docker:inspect_*"]
"#;

        let config = AuthorityConfig::from_toml(toml).unwrap();
        assert_eq!(config.keepers.len(), 2);
        assert_eq!(config.permissions.len(), 2);

        assert_eq!(config.keepers[0].name, "pknull");
        assert_eq!(
            config.keepers[0].egregore,
            Some("@7JIN8TA3bZ1l786oQ6lPN3l94KEFFH0UlVz9lqTr5+E=.ed25519".to_string())
        );

        assert_eq!(config.permissions[0].keeper, "pknull");
        assert_eq!(config.permissions[0].place, "*");
        assert_eq!(config.permissions[0].skills, vec!["*"]);
    }

    #[test]
    fn test_empty_config() {
        let config = AuthorityConfig::from_toml("").unwrap();
        assert!(config.keepers.is_empty());
        assert!(config.permissions.is_empty());
    }
}
