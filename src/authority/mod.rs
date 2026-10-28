//! Person/Skill authorization for Servitor.
//!
//! A Servitor serves **Keepers** using **Person/Skill** authorization.
//! Each Servitor holds its own authority definitions locally.
//!
//! ## Core Concepts
//!
//! - **Person (Keeper)**: Who is making the request. Has identities across planes.
//! - **Skill**: What capabilities can be invoked. Pattern format with wildcards.
//!
//! ## Example authority.toml
//!
//! ```toml
//! [[keeper]]
//! name = "pknull"
//! egregore = "@7JIN8TA3bZ1l786oQ6lPN3l94KEFFH0UlVz9lqTr5+E=.ed25519"
//! discord = "187488812471943168"
//!
//! [[permission]]
//! keeper = "pknull"
//! skills = ["*"]
//! ```

mod config;
mod keeper;
mod permission;

pub use config::AuthorityConfig;
pub use keeper::{Keeper, PersonId};
pub use permission::{
    pattern_matches, skill_pattern_matches, AuthRequest, AuthResult, DenialReason, Permission,
};

use std::collections::HashMap;
use std::path::Path;

use crate::error::{Result, ServitorError};

/// Authority manager for Person/Skill authorization.
#[derive(Debug, Clone)]
pub struct Authority {
    /// All defined keepers.
    keepers: Vec<Keeper>,

    /// All defined permissions.
    permissions: Vec<Permission>,

    /// Index: egregore pubkey -> keeper index.
    keeper_by_egregore: HashMap<String, usize>,

    /// Index: discord user id -> keeper index.
    keeper_by_discord: HashMap<String, usize>,

    /// Whether authority is in explicitly insecure open mode.
    open_mode: bool,
}

impl Default for Authority {
    fn default() -> Self {
        Self::empty()
    }
}

impl Authority {
    /// Create an empty authority (deny all).
    pub fn empty() -> Self {
        Self {
            keepers: Vec::new(),
            permissions: Vec::new(),
            keeper_by_egregore: HashMap::new(),
            keeper_by_discord: HashMap::new(),
            open_mode: false,
        }
    }

    /// Create an insecure authority that allows all requests.
    pub fn insecure_open() -> Self {
        Self {
            keepers: Vec::new(),
            permissions: Vec::new(),
            keeper_by_egregore: HashMap::new(),
            keeper_by_discord: HashMap::new(),
            open_mode: true,
        }
    }

    /// Load authority from a TOML file.
    ///
    /// If the file doesn't exist, returns empty authority (deny all).
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::empty());
        }

        let content = std::fs::read_to_string(path).map_err(|e| ServitorError::Config {
            reason: format!("failed to read authority file: {e}"),
        })?;

        let config = AuthorityConfig::from_toml(&content).map_err(|e| ServitorError::Config {
            reason: format!("failed to parse authority file: {e}"),
        })?;

        Ok(Self::from_config(config))
    }

    /// Create authority from parsed config.
    pub fn from_config(config: AuthorityConfig) -> Self {
        let mut keeper_by_egregore = HashMap::new();
        let mut keeper_by_discord = HashMap::new();

        for (i, keeper) in config.keepers.iter().enumerate() {
            if let Some(ref pubkey) = keeper.egregore {
                keeper_by_egregore.insert(pubkey.clone(), i);
            }
            if let Some(ref user_id) = keeper.discord {
                keeper_by_discord.insert(user_id.clone(), i);
            }
        }

        Self {
            keepers: config.keepers,
            permissions: config.permissions,
            keeper_by_egregore,
            keeper_by_discord,
            open_mode: false,
        }
    }

    /// Check if running in insecure open mode (no restrictions).
    pub fn is_open_mode(&self) -> bool {
        self.open_mode
    }

    /// Identify a keeper from a person identity.
    pub fn identify(&self, person: &PersonId) -> Option<&Keeper> {
        let idx = match person {
            PersonId::Egregore(pubkey) => self.keeper_by_egregore.get(pubkey).copied(),
            PersonId::Discord(user_id) => self.keeper_by_discord.get(user_id).copied(),
            PersonId::Http(token) => {
                if token.trim().is_empty() {
                    return None;
                }
                // Scan every keeper without stopping at a match; each check is
                // a constant-time digest comparison (`token_eq`). As on the
                // other planes, a token configured twice resolves to the last
                // keeper.
                let exact = self
                    .keepers
                    .iter()
                    .enumerate()
                    .fold(None, |found, (i, keeper)| {
                        if keeper.matches(person) {
                            Some(i)
                        } else {
                            found
                        }
                    });
                // Fall back to wildcard match
                exact.or_else(|| {
                    self.keepers
                        .iter()
                        .rposition(|keeper| keeper.http_token.as_deref() == Some("*"))
                })
            }
        };
        idx.map(|i| &self.keepers[i])
    }

    /// The skill label an `auth_denied` event may publish for `skill`.
    ///
    /// A task type is the requestor's own text, so a `request:` or `assign:`
    /// skill is published only when a permission names it literally (for
    /// example `request:deploy`); a pattern with a wildcard approves nothing,
    /// and any other skill reads as `<gate>:other`.
    pub fn public_skill_label(&self, skill: &str) -> String {
        let gate = match skill.split_once(':') {
            Some((gate @ ("request" | "assign"), _)) => gate,
            _ => return "other".to_string(),
        };
        let named = self
            .permissions
            .iter()
            .flat_map(|permission| &permission.skills)
            .any(|pattern| pattern == skill && !pattern.contains(['*', '?']));
        if named {
            skill.to_string()
        } else {
            format!("{gate}:other")
        }
    }

    /// Get a keeper by name.
    pub fn get_keeper(&self, name: &str) -> Option<&Keeper> {
        self.keepers.iter().find(|k| k.name == name)
    }

    /// Get all permissions for a keeper.
    pub fn permissions_for(&self, keeper_name: &str) -> Vec<&Permission> {
        self.permissions
            .iter()
            .filter(|p| p.keeper == keeper_name)
            .collect()
    }

    /// Check if a request is authorized.
    ///
    /// Returns AuthResult with allowed status, keeper name, and reason.
    pub fn authorize(&self, req: &AuthRequest) -> AuthResult {
        // Open mode accepts all
        if self.open_mode {
            return AuthResult {
                allowed: true,
                keeper: None,
                reason: "open mode - no authority configured".to_string(),
                denial: None,
            };
        }

        // Identify the keeper
        let keeper = match self.identify(&req.person) {
            Some(k) => k,
            None => {
                return AuthResult::denied(
                    DenialReason::UnknownIdentity,
                    format!("unknown identity: {}", req.person.display()),
                );
            }
        };

        // Find matching permission
        let permissions = self.permissions_for(&keeper.name);
        if permissions.is_empty() {
            return AuthResult::denied_keeper(
                &keeper.name,
                DenialReason::NoPermissions,
                "no permissions defined for keeper",
            );
        }

        for perm in permissions {
            if perm.matches(&req.skill) {
                return AuthResult::allowed(
                    &keeper.name,
                    format!("matched permission: skills={:?}", perm.skills),
                );
            }
        }

        AuthResult::denied_keeper(
            &keeper.name,
            DenialReason::NoMatchingPermission,
            format!("no matching permission for skill={}", req.skill),
        )
    }

    /// Authorize a skill check only (when keeper is already known).
    ///
    /// Used during tool execution when the keeper was already identified
    /// at task intake.
    pub fn authorize_skill(&self, keeper_name: &str, skill: &str) -> AuthResult {
        if self.open_mode {
            return AuthResult {
                allowed: true,
                keeper: Some(keeper_name.to_string()),
                reason: "open mode - no authority configured".to_string(),
                denial: None,
            };
        }

        let permissions = self.permissions_for(keeper_name);
        if permissions.is_empty() {
            return AuthResult::denied_keeper(
                keeper_name,
                DenialReason::NoPermissions,
                "no permissions defined for keeper",
            );
        }

        // Check if any permission allows this skill
        for perm in permissions {
            if perm.skills.iter().any(|s| skill_pattern_matches(s, skill)) {
                return AuthResult::allowed(
                    keeper_name,
                    format!("matched skill permission: {:?}", perm.skills),
                );
            }
        }

        AuthResult::denied_keeper(
            keeper_name,
            DenialReason::NoMatchingPermission,
            format!("no matching permission for skill={skill}"),
        )
    }
}

/// Load authority configuration for runtime use.
///
/// Handles insecure mode and missing authority file errors with appropriate
/// warnings and error messages.
pub fn load_runtime_authority(identity_dir: &Path, insecure: bool) -> Result<Authority> {
    let authority_path = identity_dir.join("authority.toml");

    if insecure {
        if std::env::var("SERVITOR_INSECURE").as_deref() != Ok("1") {
            return Err(ServitorError::Config {
                reason: "Insecure mode requires SERVITOR_INSECURE=1 environment variable. \
                        This prevents accidental deployment without authorization."
                    .into(),
            });
        }
        tracing::warn!("=======================================================");
        tracing::warn!("  INSECURE MODE: All authorization checks are disabled");
        tracing::warn!("  Any identity can execute any skill");
        tracing::warn!("  DO NOT use in production");
        tracing::warn!("=======================================================");
        return Ok(Authority::insecure_open());
    }

    if !authority_path.exists() {
        tracing::warn!(
            path = %authority_path.display(),
            "authority file missing; refusing to start without explicit access control"
        );
        return Err(ServitorError::Config {
            reason: format!(
                "authority file not found: {}. Copy authority.example.toml there, or use --insecure for development only",
                authority_path.display()
            ),
        });
    }

    let authority = Authority::load(&authority_path)?;
    tracing::debug!("authority: loaded from {}", authority_path.display());
    Ok(authority)
}

/// Authorize local execution for a servitor identity.
///
/// Returns the keeper name if authorized, None if in open mode.
/// Returns an error if the identity is not authorized.
pub fn authorize_local_exec(
    authority: &Authority,
    identity: &crate::identity::Identity,
) -> Result<Option<String>> {
    if authority.is_open_mode() {
        return Ok(None);
    }

    let auth_result = authority.authorize(&AuthRequest {
        person: PersonId::from_egregore(identity.public_id().0.clone()),
        skill: "*".to_string(),
    });

    if !auth_result.allowed {
        return Err(ServitorError::Unauthorized {
            reason: format!(
                "local exec not authorized for {}: {}",
                identity.public_id(),
                auth_result.reason
            ),
        });
    }

    Ok(auth_result.keeper)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_authority() -> Authority {
        let config = AuthorityConfig::from_toml(
            r#"
[[keeper]]
name = "pknull"
egregore = "@7JIN8TA3bZ1l786oQ6lPN3l94KEFFH0UlVz9lqTr5+E=.ed25519"
discord = "187488812471943168"

[[keeper]]
name = "automation"
egregore = "@AutomationKey.ed25519"

[[permission]]
keeper = "pknull"
skills = ["*"]

[[permission]]
keeper = "automation"
skills = ["docker:inspect_*"]
"#,
        )
        .unwrap();
        Authority::from_config(config)
    }

    #[test]
    fn test_empty_authority() {
        let auth = Authority::empty();
        assert!(!auth.is_open_mode());

        let result = auth.authorize(&AuthRequest {
            person: PersonId::Egregore("@unknown.ed25519".to_string()),
            skill: "anything".to_string(),
        });
        assert!(!result.allowed);
    }

    #[test]
    fn test_insecure_open_authority() {
        let auth = Authority::insecure_open();
        assert!(auth.is_open_mode());

        let result = auth.authorize(&AuthRequest {
            person: PersonId::Egregore("@unknown.ed25519".to_string()),
            skill: "anything".to_string(),
        });
        assert!(result.allowed);
    }

    #[test]
    fn test_load_missing_authority_is_deny_all() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("authority.toml");

        let auth = Authority::load(&path).unwrap();
        assert!(!auth.is_open_mode());

        let result = auth.authorize(&AuthRequest {
            person: PersonId::Egregore("@unknown.ed25519".to_string()),
            skill: "*".to_string(),
        });
        assert!(!result.allowed);
    }

    #[test]
    fn test_identify_keeper() {
        let auth = test_authority();

        let keeper = auth.identify(&PersonId::Egregore(
            "@7JIN8TA3bZ1l786oQ6lPN3l94KEFFH0UlVz9lqTr5+E=.ed25519".to_string(),
        ));
        assert!(keeper.is_some());
        assert_eq!(keeper.unwrap().name, "pknull");

        let keeper = auth.identify(&PersonId::Discord("187488812471943168".to_string()));
        assert!(keeper.is_some());
        assert_eq!(keeper.unwrap().name, "pknull");

        let keeper = auth.identify(&PersonId::Egregore("@unknown.ed25519".to_string()));
        assert!(keeper.is_none());
    }

    #[test]
    fn test_authorize_full_access() {
        let auth = test_authority();

        // pknull has full access
        let result = auth.authorize(&AuthRequest {
            person: PersonId::Egregore(
                "@7JIN8TA3bZ1l786oQ6lPN3l94KEFFH0UlVz9lqTr5+E=.ed25519".to_string(),
            ),
            skill: "shell:execute".to_string(),
        });
        assert!(result.allowed);
        assert_eq!(result.keeper, Some("pknull".to_string()));
    }

    #[test]
    fn test_authorize_limited_access() {
        let auth = test_authority();

        // automation can only use docker:inspect_*
        let result = auth.authorize(&AuthRequest {
            person: PersonId::Egregore("@AutomationKey.ed25519".to_string()),
            skill: "docker:inspect_container".to_string(),
        });
        assert!(result.allowed);

        // automation denied for wrong skill
        let result = auth.authorize(&AuthRequest {
            person: PersonId::Egregore("@AutomationKey.ed25519".to_string()),
            skill: "shell:execute".to_string(),
        });
        assert!(!result.allowed);

        // Nested-namespace negative: the docker:inspect_* grant must not
        // authorize a deeper, more privileged skill segment.
        let result = auth.authorize(&AuthRequest {
            person: PersonId::Egregore("@AutomationKey.ed25519".to_string()),
            skill: "docker:inspect_container:privileged".to_string(),
        });
        assert!(!result.allowed);
    }

    #[test]
    fn test_authorize_unknown_person() {
        let auth = test_authority();

        let result = auth.authorize(&AuthRequest {
            person: PersonId::Egregore("@unknown.ed25519".to_string()),
            skill: "anything".to_string(),
        });
        assert!(!result.allowed);
        assert!(result.reason.contains("unknown identity"));
    }

    #[test]
    fn test_authorize_unknown_non_ascii_person_is_denied() {
        let auth = test_authority();

        let result = auth.authorize(&AuthRequest {
            person: PersonId::Egregore("@abcdefghijé.ed25519".to_string()),
            skill: "anything".to_string(),
        });
        assert!(!result.allowed);
        assert_eq!(result.reason, "unknown identity: egregore:@abcdefghij..");
    }

    #[test]
    fn test_authorize_skill_only() {
        let auth = test_authority();

        // pknull can use any skill
        let result = auth.authorize_skill("pknull", "shell:execute");
        assert!(result.allowed);

        // automation can only use docker:inspect_*
        let result = auth.authorize_skill("automation", "docker:inspect_container");
        assert!(result.allowed);

        let result = auth.authorize_skill("automation", "shell:execute");
        assert!(!result.allowed);

        // Nested-namespace negative: the docker:inspect_* grant must not
        // authorize a deeper, more privileged skill segment.
        let result = auth.authorize_skill("automation", "docker:inspect_container:privileged");
        assert!(!result.allowed);
    }

    #[test]
    fn test_http_wildcard_token() {
        let config = AuthorityConfig::from_toml(
            r#"
[[keeper]]
name = "test-user"
http_token = "*"

[[permission]]
keeper = "test-user"
skills = ["*"]
"#,
        )
        .unwrap();
        let auth = Authority::from_config(config);

        // Any HTTP token should match the wildcard
        let result = auth.authorize(&AuthRequest {
            person: PersonId::Http("any-random-token".to_string()),
            skill: "some_skill".to_string(),
        });
        assert!(result.allowed);
        assert_eq!(result.keeper, Some("test-user".to_string()));

        // Another random token should also match
        let result = auth.authorize(&AuthRequest {
            person: PersonId::Http("test-token".to_string()),
            skill: "another_skill".to_string(),
        });
        assert!(result.allowed);
    }

    #[test]
    fn test_http_exact_token_takes_precedence() {
        let config = AuthorityConfig::from_toml(
            r#"
[[keeper]]
name = "wildcard-user"
http_token = "*"

[[keeper]]
name = "specific-user"
http_token = "specific-token"

[[permission]]
keeper = "wildcard-user"
skills = ["basic:*"]

[[permission]]
keeper = "specific-user"
skills = ["*"]
"#,
        )
        .unwrap();
        let auth = Authority::from_config(config);

        // Exact token match should take precedence
        let result = auth.authorize(&AuthRequest {
            person: PersonId::Http("specific-token".to_string()),
            skill: "advanced_skill".to_string(),
        });
        assert!(result.allowed);
        assert_eq!(result.keeper, Some("specific-user".to_string()));

        // Other tokens fall back to wildcard
        let result = auth.authorize(&AuthRequest {
            person: PersonId::Http("other-token".to_string()),
            skill: "basic:read".to_string(),
        });
        assert!(result.allowed);
        assert_eq!(result.keeper, Some("wildcard-user".to_string()));
    }

    #[test]
    fn test_http_token_must_match_exactly() {
        let config = AuthorityConfig::from_toml(
            r#"
[[keeper]]
name = "bot"
http_token = "secure-token"
"#,
        )
        .unwrap();
        let auth = Authority::from_config(config);

        let found = auth.identify(&PersonId::Http("secure-token".to_string()));
        assert_eq!(found.map(|k| k.name.as_str()), Some("bot"));
        for token in [
            "secure",
            "secure-token-extra",
            "Secure-token",
            "secure-tokem",
        ] {
            assert!(
                auth.identify(&PersonId::Http(token.to_string())).is_none(),
                "{token:?}"
            );
        }
    }

    #[test]
    fn test_blank_http_token_identifies_nobody() {
        let config = AuthorityConfig::from_toml(
            r#"
[[keeper]]
name = "misconfigured"
http_token = ""

[[keeper]]
name = "spaces"
http_token = "  "

[[keeper]]
name = "wildcard-user"
http_token = "*"
"#,
        )
        .unwrap();
        let auth = Authority::from_config(config);

        // A blank token is no token: it reaches neither a blank configured
        // token nor the wildcard. A whitespace-only configured token admits
        // nobody either.
        for token in ["", " ", "  "] {
            assert!(
                auth.identify(&PersonId::Http(token.to_string())).is_none(),
                "{token:?}"
            );
        }
    }

    #[test]
    fn test_public_skill_label_names_only_literal_permissions() {
        let config = AuthorityConfig::from_toml(
            r#"
[[keeper]]
name = "operator"
egregore = "@operator.ed25519"

[[permission]]
keeper = "operator"
skills = ["request:deploy", "request:deploy_*", "assign:*", "shell:*"]
"#,
        )
        .unwrap();
        let auth = Authority::from_config(config);

        assert_eq!(auth.public_skill_label("request:deploy"), "request:deploy");
        assert_eq!(
            auth.public_skill_label("request:other-thing"),
            "request:other"
        );
        // A wildcard grant approves no particular task type for publication,
        // even when the requestor's text equals the pattern.
        assert_eq!(auth.public_skill_label("assign:deploy"), "assign:other");
        assert_eq!(auth.public_skill_label("assign:*"), "assign:other");
        assert_eq!(auth.public_skill_label("request:deploy_*"), "request:other");
        assert_eq!(auth.public_skill_label("shell:execute"), "other");
        assert_eq!(auth.public_skill_label("no-gate"), "other");
    }

    #[test]
    fn test_duplicate_http_tokens_resolve_to_the_last_keeper() {
        let config = AuthorityConfig::from_toml(
            r#"
[[keeper]]
name = "first-wildcard"
http_token = "*"

[[keeper]]
name = "first"
http_token = "shared-token"

[[keeper]]
name = "second"
http_token = "shared-token"

[[keeper]]
name = "second-wildcard"
http_token = "*"
"#,
        )
        .unwrap();
        let auth = Authority::from_config(config);
        let name = |token: &str| {
            auth.identify(&PersonId::Http(token.to_string()))
                .map(|k| k.name.clone())
        };

        assert_eq!(name("shared-token").as_deref(), Some("second"));
        assert_eq!(name("unlisted").as_deref(), Some("second-wildcard"));
        // The literal `*` matches the wildcard entries exactly.
        assert_eq!(name("*").as_deref(), Some("second-wildcard"));
    }
}
