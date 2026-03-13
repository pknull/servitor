//! Consumer group coordination for SSE task distribution.

use chrono::{DateTime, Duration, Utc};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

use crate::egregore::ServitorProfile;
use crate::identity::PublicId;

const HEARTBEAT_GRACE_MULTIPLIER: i32 = 3;

#[derive(Debug, Clone)]
struct GroupMember {
    servitor_id: PublicId,
    last_seen: DateTime<Utc>,
    heartbeat_interval_ms: u64,
}

/// Tracks live members for a named consumer group and deterministically assigns
/// each task hash to exactly one active member.
#[derive(Debug, Clone)]
pub struct ConsumerGroupCoordinator {
    group_name: String,
    self_id: PublicId,
    members: HashMap<String, GroupMember>,
}

impl ConsumerGroupCoordinator {
    pub fn new(group_name: impl Into<String>, self_id: PublicId) -> Self {
        Self {
            group_name: group_name.into(),
            self_id,
            members: HashMap::new(),
        }
    }

    pub fn group_name(&self) -> &str {
        &self.group_name
    }

    pub fn observe_profile(&mut self, profile: &ServitorProfile, seen_at: DateTime<Utc>) {
        if !profile.groups.iter().any(|group| group == &self.group_name) {
            self.members.remove(&profile.servitor_id.0);
            return;
        }

        self.members.insert(
            profile.servitor_id.0.clone(),
            GroupMember {
                servitor_id: profile.servitor_id.clone(),
                last_seen: seen_at,
                heartbeat_interval_ms: profile.heartbeat_interval_ms.max(1),
            },
        );
    }

    pub fn evict_stale(&mut self, now: DateTime<Utc>) {
        self.members.retain(|_, member| {
            now <= member.last_seen + heartbeat_grace(member.heartbeat_interval_ms)
        });
    }

    pub fn active_members(&mut self, now: DateTime<Utc>) -> Vec<PublicId> {
        self.evict_stale(now);

        let mut members: Vec<PublicId> = self
            .members
            .values()
            .map(|member| member.servitor_id.clone())
            .collect();
        members.sort_by(|left, right| left.0.cmp(&right.0));
        members
    }

    pub fn owner_for(&mut self, task_hash: &str, now: DateTime<Utc>) -> Option<PublicId> {
        let members = self.active_members(now);
        if members.is_empty() {
            return None;
        }

        let index = ownership_index(&self.group_name, task_hash, members.len());
        members.get(index).cloned()
    }

    pub fn should_process(&mut self, task_hash: &str, now: DateTime<Utc>) -> bool {
        match self.owner_for(task_hash, now) {
            Some(owner) => owner == self.self_id,
            None => true,
        }
    }
}

fn heartbeat_grace(heartbeat_interval_ms: u64) -> Duration {
    let millis = heartbeat_interval_ms
        .saturating_mul(HEARTBEAT_GRACE_MULTIPLIER as u64)
        .max(1_000);
    Duration::milliseconds(millis as i64)
}

fn ownership_index(group_name: &str, task_hash: &str, member_count: usize) -> usize {
    let mut hasher = Sha256::new();
    hasher.update(group_name.as_bytes());
    hasher.update(b":");
    hasher.update(task_hash.as_bytes());
    let digest = hasher.finalize();
    let bucket = u64::from_be_bytes(digest[..8].try_into().expect("sha256 digest length"));
    (bucket as usize) % member_count
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::egregore::ServitorProfile;

    fn profile(id: &str, heartbeat_interval_ms: u64, groups: &[&str]) -> ServitorProfile {
        let mut profile = ServitorProfile::new(PublicId(id.to_string()), heartbeat_interval_ms);
        profile.groups = groups.iter().map(|group| group.to_string()).collect();
        profile
    }

    #[test]
    fn ignores_profiles_outside_group() {
        let now = Utc::now();
        let mut coordinator =
            ConsumerGroupCoordinator::new("workers", PublicId("@self.ed25519".to_string()));

        coordinator.observe_profile(&profile("@peer.ed25519", 10_000, &["other"]), now);

        assert!(coordinator.active_members(now).is_empty());
    }

    #[test]
    fn evicts_stale_members() {
        let now = Utc::now();
        let mut coordinator =
            ConsumerGroupCoordinator::new("workers", PublicId("@self.ed25519".to_string()));

        coordinator.observe_profile(&profile("@self.ed25519", 1_000, &["workers"]), now);
        coordinator.observe_profile(
            &profile("@peer.ed25519", 1_000, &["workers"]),
            now - Duration::seconds(5),
        );

        let members = coordinator.active_members(now);
        assert_eq!(members, vec![PublicId("@self.ed25519".to_string())]);
    }

    #[test]
    fn ownership_is_deterministic_for_same_membership() {
        let now = Utc::now();
        let mut left =
            ConsumerGroupCoordinator::new("workers", PublicId("@self.ed25519".to_string()));
        let mut right =
            ConsumerGroupCoordinator::new("workers", PublicId("@peer1.ed25519".to_string()));

        for id in ["@peer2.ed25519", "@self.ed25519", "@peer1.ed25519"] {
            let profile = profile(id, 10_000, &["workers"]);
            left.observe_profile(&profile, now);
            right.observe_profile(&profile, now);
        }

        assert_eq!(
            left.owner_for("task-123", now),
            right.owner_for("task-123", now)
        );
    }

    #[test]
    fn stale_owner_rebalances_to_live_member() {
        let now = Utc::now();
        let mut coordinator =
            ConsumerGroupCoordinator::new("workers", PublicId("@self.ed25519".to_string()));

        coordinator.observe_profile(&profile("@self.ed25519", 1_000, &["workers"]), now);
        coordinator.observe_profile(
            &profile("@peer.ed25519", 1_000, &["workers"]),
            now - Duration::seconds(5),
        );

        assert_eq!(
            coordinator.owner_for("task-123", now),
            Some(PublicId("@self.ed25519".to_string()))
        );
    }
}
