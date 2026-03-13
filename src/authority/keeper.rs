//! Keeper identity types.
//!
//! A Keeper has identities across planes (egregore, discord, http).
//! PersonId represents a single-plane identity used for authentication.

use serde::{Deserialize, Serialize};

/// A Keeper (person) with identities across planes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Keeper {
    /// Unique name for this keeper.
    pub name: String,

    /// Egregore identity: `@pubkey.ed25519`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub egregore: Option<String>,

    /// Discord user ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discord: Option<String>,

    /// HTTP bearer token.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_token: Option<String>,
}

impl Keeper {
    /// Check if this keeper matches the given person identity.
    pub fn matches(&self, person: &PersonId) -> bool {
        match person {
            PersonId::Egregore(pubkey) => self.egregore.as_ref() == Some(pubkey),
            PersonId::Discord(user_id) => self.discord.as_ref() == Some(user_id),
            PersonId::Http(token) => self.http_token.as_ref() == Some(token),
        }
    }
}

/// A single-plane identity used for authentication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PersonId {
    /// Egregore identity: `@pubkey.ed25519`
    Egregore(String),

    /// Discord user ID.
    Discord(String),

    /// HTTP bearer token.
    Http(String),
}

impl PersonId {
    /// Parse an egregore author string into a PersonId.
    pub fn from_egregore(pubkey: impl Into<String>) -> Self {
        PersonId::Egregore(pubkey.into())
    }

    /// Parse a Discord user ID into a PersonId.
    pub fn from_discord(user_id: impl Into<String>) -> Self {
        PersonId::Discord(user_id.into())
    }

    /// Parse an HTTP bearer token into a PersonId.
    pub fn from_http(token: impl Into<String>) -> Self {
        PersonId::Http(token.into())
    }

    /// Get a display string for logging.
    pub fn display(&self) -> String {
        match self {
            PersonId::Egregore(pubkey) => {
                if pubkey.len() > 12 {
                    format!("egregore:{}..", &pubkey[..12])
                } else {
                    format!("egregore:{pubkey}")
                }
            }
            PersonId::Discord(user_id) => format!("discord:{user_id}"),
            PersonId::Http(_) => "http:<token>".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keeper_matches_egregore() {
        let keeper = Keeper {
            name: "pknull".to_string(),
            egregore: Some("@abc123.ed25519".to_string()),
            discord: None,
            http_token: None,
        };

        assert!(keeper.matches(&PersonId::Egregore("@abc123.ed25519".to_string())));
        assert!(!keeper.matches(&PersonId::Egregore("@other.ed25519".to_string())));
        assert!(!keeper.matches(&PersonId::Discord("123".to_string())));
    }

    #[test]
    fn test_keeper_matches_discord() {
        let keeper = Keeper {
            name: "pknull".to_string(),
            egregore: None,
            discord: Some("187488812471943168".to_string()),
            http_token: None,
        };

        assert!(keeper.matches(&PersonId::Discord("187488812471943168".to_string())));
        assert!(!keeper.matches(&PersonId::Discord("other".to_string())));
        assert!(!keeper.matches(&PersonId::Egregore("@abc.ed25519".to_string())));
    }

    #[test]
    fn test_person_id_display() {
        let egregore =
            PersonId::Egregore("@7JIN8TA3bZ1l786oQ6lPN3l94KEFFH0UlVz9lqTr5+E=.ed25519".to_string());
        // Truncates to first 12 chars: "@7JIN8TA3bZ1"
        assert!(egregore.display().starts_with("egregore:@7JIN8TA3bZ1"));

        let discord = PersonId::Discord("187488812471943168".to_string());
        assert_eq!(discord.display(), "discord:187488812471943168");

        let http = PersonId::Http("secret-token".to_string());
        assert_eq!(http.display(), "http:<token>");
    }
}
