use std::collections::HashSet;

use serde::{Deserialize, Serialize};

/// Collaboration mode for a team of agents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TeamMode {
    /// One leader coordinates and assigns tasks to workers.
    /// Workers report back to the leader only.
    #[default]
    LeaderWorker,
    /// All agents can communicate directly with each other.
    /// Any agent can assign tasks or request help from any other.
    PeerToPeer,
}

impl std::fmt::Display for TeamMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LeaderWorker => write!(f, "leader-worker"),
            Self::PeerToPeer => write!(f, "peer-to-peer"),
        }
    }
}

/// Capability that an agent declares it can perform.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Capability(pub String);

impl Capability {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Capability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Team member descriptor.
#[derive(Debug, Clone)]
pub struct TeamMember {
    pub agent_id: String,
    pub name: String,
    pub model: String,
    /// What this agent can do (used for capability-based assignment).
    pub capabilities: HashSet<Capability>,
    /// Whether this agent is the team leader (only meaningful in `LeaderWorker` mode).
    pub is_leader: bool,
}

impl TeamMember {
    /// Create a new team member with no capabilities.
    #[must_use]
    pub fn new(
        agent_id: impl Into<String>,
        name: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            agent_id: agent_id.into(),
            name: name.into(),
            model: model.into(),
            capabilities: HashSet::new(),
            is_leader: false,
        }
    }

    /// Add a capability to this member.
    pub fn add_capability(&mut self, cap: Capability) {
        self.capabilities.insert(cap);
    }

    /// Check whether this member has a specific capability.
    #[must_use]
    pub fn has_capability(&self, cap: &Capability) -> bool {
        self.capabilities.contains(cap)
    }

    /// Check whether this member has a capability by name.
    #[must_use]
    pub fn has_capability_named(&self, name: &str) -> bool {
        self.capabilities.iter().any(|c| c.0 == name)
    }
}

/// Team creation and member management.
pub struct Team {
    pub name: String,
    pub mode: TeamMode,
    pub members: Vec<TeamMember>,
}

impl Team {
    /// Create a new team with the default mode (`LeaderWorker`).
    #[must_use]
    pub fn new(name: String) -> Self {
        Self {
            name,
            mode: TeamMode::default(),
            members: Vec::new(),
        }
    }

    /// Create a team with a specific collaboration mode.
    #[must_use]
    pub fn with_mode(name: String, mode: TeamMode) -> Self {
        Self {
            name,
            mode,
            members: Vec::new(),
        }
    }

    pub fn add_member(&mut self, member: TeamMember) {
        self.members.push(member);
    }

    #[must_use]
    pub fn get_member(&self, name: &str) -> Option<&TeamMember> {
        self.members.iter().find(|m| m.name == name)
    }

    #[must_use]
    pub fn get_member_mut(&mut self, name: &str) -> Option<&mut TeamMember> {
        self.members.iter_mut().find(|m| m.name == name)
    }

    /// Get the team leader (first member with `is_leader = true`).
    #[must_use]
    pub fn leader(&self) -> Option<&TeamMember> {
        self.members.iter().find(|m| m.is_leader)
    }

    /// Find members that have a specific capability.
    #[must_use]
    pub fn members_with_capability(&self, cap: &Capability) -> Vec<&TeamMember> {
        self.members
            .iter()
            .filter(|m| m.has_capability(cap))
            .collect()
    }

    /// Find members that have a capability by name.
    #[must_use]
    pub fn members_with_capability_named(&self, name: &str) -> Vec<&TeamMember> {
        self.members
            .iter()
            .filter(|m| m.has_capability_named(name))
            .collect()
    }

    /// Check whether a given agent is allowed to send a message to another
    /// under the team's collaboration mode.
    ///
    /// In `LeaderWorker` mode, only the leader can send to workers and
    /// workers can only reply to the leader. In `PeerToPeer` mode, anyone
    /// can send to anyone.
    #[must_use]
    pub fn can_communicate(&self, from: &str, to: &str) -> bool {
        match self.mode {
            TeamMode::PeerToPeer => {
                // Anyone can talk to anyone
                self.get_member(from).is_some() && self.get_member(to).is_some()
            }
            TeamMode::LeaderWorker => {
                let from_member = self.get_member(from);
                let to_member = self.get_member(to);
                match (from_member, to_member) {
                    (Some(f), Some(t)) => {
                        // Leader can talk to anyone; workers can only talk to leader
                        f.is_leader || t.is_leader
                    }
                    _ => false,
                }
            }
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.members.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alice() -> TeamMember {
        let mut m = TeamMember::new("a1", "Alice", "claude-3");
        m.is_leader = true;
        m.add_capability(Capability::new("code_review"));
        m.add_capability(Capability::new("planning"));
        m
    }

    fn bob() -> TeamMember {
        let mut m = TeamMember::new("a2", "Bob", "gpt-4o");
        m.add_capability(Capability::new("code_review"));
        m.add_capability(Capability::new("testing"));
        m
    }

    fn charlie() -> TeamMember {
        let mut m = TeamMember::new("a3", "Charlie", "claude-3");
        m.add_capability(Capability::new("frontend"));
        m
    }

    #[test]
    fn team_creation() {
        let team = Team::new("dev-team".into());
        assert_eq!(team.name, "dev-team");
        assert!(team.is_empty());
        assert_eq!(team.len(), 0);
        assert_eq!(team.mode, TeamMode::LeaderWorker);
    }

    #[test]
    fn team_with_mode() {
        let team = Team::with_mode("p2p-team".into(), TeamMode::PeerToPeer);
        assert_eq!(team.mode, TeamMode::PeerToPeer);
    }

    #[test]
    fn add_and_get_member() {
        let mut team = Team::new("team".into());
        team.add_member(alice());
        assert_eq!(team.len(), 1);
        let member = team.get_member("Alice").unwrap();
        assert_eq!(member.agent_id, "a1");
        assert_eq!(member.model, "claude-3");
    }

    #[test]
    fn get_nonexistent_member() {
        let team = Team::new("team".into());
        assert!(team.get_member("nobody").is_none());
    }

    #[test]
    fn multiple_members() {
        let mut team = Team::new("team".into());
        team.add_member(alice());
        team.add_member(bob());
        assert_eq!(team.len(), 2);
        assert!(!team.is_empty());
        assert!(team.get_member("Alice").is_some());
        assert!(team.get_member("Bob").is_some());
    }

    #[test]
    fn team_member_clone() {
        let member = alice();
        let cloned = member;
        assert_eq!(cloned.agent_id, "a1");
        assert_eq!(cloned.name, "Alice");
    }

    // ─── TeamMode ───

    #[test]
    fn team_mode_default() {
        assert_eq!(TeamMode::default(), TeamMode::LeaderWorker);
    }

    #[test]
    fn team_mode_display() {
        assert_eq!(TeamMode::LeaderWorker.to_string(), "leader-worker");
        assert_eq!(TeamMode::PeerToPeer.to_string(), "peer-to-peer");
    }

    #[test]
    fn team_mode_serde_roundtrip() {
        let modes = [TeamMode::LeaderWorker, TeamMode::PeerToPeer];
        for mode in &modes {
            let json = serde_json::to_string(mode).unwrap();
            let parsed: TeamMode = serde_json::from_str(&json).unwrap();
            assert_eq!(*mode, parsed);
        }
    }

    // ─── Capability ───

    #[test]
    fn capability_new() {
        let cap = Capability::new("testing");
        assert_eq!(cap.name(), "testing");
        assert_eq!(cap.to_string(), "testing");
    }

    #[test]
    fn capability_equality() {
        let a = Capability::new("testing");
        let b = Capability::new("testing");
        let c = Capability::new("planning");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn capability_serde_roundtrip() {
        let cap = Capability::new("code_review");
        let json = serde_json::to_string(&cap).unwrap();
        let parsed: Capability = serde_json::from_str(&json).unwrap();
        assert_eq!(cap, parsed);
    }

    // ─── TeamMember capabilities ───

    #[test]
    fn member_has_capability() {
        let m = alice();
        assert!(m.has_capability(&Capability::new("code_review")));
        assert!(m.has_capability_named("planning"));
        assert!(!m.has_capability_named("testing"));
    }

    #[test]
    fn member_add_capability() {
        let mut m = TeamMember::new("a1", "Alice", "model");
        assert!(!m.has_capability_named("testing"));
        m.add_capability(Capability::new("testing"));
        assert!(m.has_capability_named("testing"));
    }

    // ─── Team leader ───

    #[test]
    fn team_leader() {
        let mut team = Team::new("team".into());
        team.add_member(alice()); // leader
        team.add_member(bob());
        let leader = team.leader().unwrap();
        assert_eq!(leader.name, "Alice");
        assert!(leader.is_leader);
    }

    #[test]
    fn team_no_leader() {
        let mut team = Team::new("team".into());
        team.add_member(bob()); // not a leader
        assert!(team.leader().is_none());
    }

    // ─── Capability-based member lookup ───

    #[test]
    fn members_with_capability() {
        let mut team = Team::new("team".into());
        team.add_member(alice());
        team.add_member(bob());
        team.add_member(charlie());

        let reviewers = team.members_with_capability(&Capability::new("code_review"));
        assert_eq!(reviewers.len(), 2);

        let frontend = team.members_with_capability_named("frontend");
        assert_eq!(frontend.len(), 1);
        assert_eq!(frontend[0].name, "Charlie");

        let none = team.members_with_capability_named("devops");
        assert!(none.is_empty());
    }

    // ─── Communication rules ───

    #[test]
    fn leader_worker_communication() {
        let mut team = Team::new("team".into());
        team.add_member(alice()); // leader
        team.add_member(bob()); // worker
        team.add_member(charlie()); // worker

        // Leader can talk to workers
        assert!(team.can_communicate("Alice", "Bob"));
        assert!(team.can_communicate("Alice", "Charlie"));

        // Workers can talk to leader
        assert!(team.can_communicate("Bob", "Alice"));
        assert!(team.can_communicate("Charlie", "Alice"));

        // Workers cannot talk to each other
        assert!(!team.can_communicate("Bob", "Charlie"));
        assert!(!team.can_communicate("Charlie", "Bob"));
    }

    #[test]
    fn peer_to_peer_communication() {
        let mut team = Team::with_mode("team".into(), TeamMode::PeerToPeer);
        team.add_member(alice());
        team.add_member(bob());
        team.add_member(charlie());

        // Everyone can talk to everyone
        assert!(team.can_communicate("Alice", "Bob"));
        assert!(team.can_communicate("Bob", "Charlie"));
        assert!(team.can_communicate("Charlie", "Alice"));
        assert!(team.can_communicate("Bob", "Alice"));
    }

    #[test]
    fn communication_with_nonmember() {
        let mut team = Team::new("team".into());
        team.add_member(alice());

        assert!(!team.can_communicate("Alice", "nobody"));
        assert!(!team.can_communicate("nobody", "Alice"));
    }

    // ─── Mutable member access ───

    #[test]
    fn get_member_mut() {
        let mut team = Team::new("team".into());
        team.add_member(bob());

        let member = team.get_member_mut("Bob").unwrap();
        member.add_capability(Capability::new("devops"));

        assert!(
            team.get_member("Bob")
                .unwrap()
                .has_capability_named("devops")
        );
    }
}
