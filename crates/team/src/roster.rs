//! The team roster — the single register of every agent instance a session
//! has spawned.
//!
//! [`Teammate`] is the one actor abstraction: a spawned agent with an
//! identity, a role, and a [`Lifetime`]. Ephemeral teammates run one task and
//! hand their result back to the parent conversation; resident teammates live
//! until the session ends and stay addressable by name. Both run the same
//! agent loop — the lifetime only decides how the run terminates.

use std::collections::HashSet;
use std::time::{Duration, Instant};

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

/// How long a teammate lives, and how its run terminates.
///
/// This is the only essential difference between the two spawn paths: both
/// run the same agent loop with the same registry, permission posture, and
/// cancellation semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lifetime {
    /// Runs one task, reports a result, and leaves the roster. Spawned by an
    /// `Agent` tool call with no `name`.
    Ephemeral {
        /// Maximum query-loop turns before forced shutdown.
        max_turns: Option<usize>,
        /// Maximum wall-clock duration before forced shutdown.
        max_duration: Option<Duration>,
    },
    /// Lives until the session ends, stays addressable by name, and keeps its
    /// conversation across messages. Spawned by an `Agent` tool call carrying
    /// a `name`.
    Resident,
}

impl Lifetime {
    /// An ephemeral lifetime with no limits.
    #[must_use]
    pub fn ephemeral() -> Self {
        Self::Ephemeral {
            max_turns: None,
            max_duration: None,
        }
    }

    /// Whether this teammate leaves the roster after one task.
    #[must_use]
    pub fn is_ephemeral(self) -> bool {
        matches!(self, Self::Ephemeral { .. })
    }

    /// Turn limit, if any.
    #[must_use]
    pub fn max_turns(self) -> Option<usize> {
        match self {
            Self::Ephemeral { max_turns, .. } => max_turns,
            Self::Resident => None,
        }
    }

    /// Wall-clock limit, if any.
    #[must_use]
    pub fn max_duration(self) -> Option<Duration> {
        match self {
            Self::Ephemeral { max_duration, .. } => max_duration,
            Self::Resident => None,
        }
    }
}

impl std::fmt::Display for Lifetime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ephemeral { .. } => write!(f, "ephemeral"),
            Self::Resident => write!(f, "resident"),
        }
    }
}

/// Lifecycle state of a teammate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TeammateState {
    /// Created but not yet executing work.
    Idle,
    /// Actively processing a task.
    Running,
    /// Finished its work successfully.
    Done,
    /// Finished with an error.
    Failed,
    /// Torn down (cancelled or session ended).
    Stopped,
}

impl TeammateState {
    /// Whether no further work will run under this state.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Stopped)
    }
}

impl std::fmt::Display for TeammateState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Idle => write!(f, "idle"),
            Self::Running => write!(f, "running"),
            Self::Done => write!(f, "done"),
            Self::Failed => write!(f, "failed"),
            Self::Stopped => write!(f, "stopped"),
        }
    }
}

/// One spawned agent instance — the single actor abstraction.
#[derive(Debug, Clone)]
pub struct Teammate {
    /// Backend-assigned unique identifier.
    pub id: String,
    /// Addressable name (`@alice`). Ephemeral teammates get their id as name.
    pub name: String,
    /// Role / specialty, usually the `subagent_type` that defined it.
    pub role: String,
    /// Model override, or `None` to inherit the session's model.
    pub model: Option<String>,
    /// What this teammate can do (used for capability-based assignment).
    pub capabilities: HashSet<Capability>,
    /// How long it lives and how its run terminates.
    pub lifetime: Lifetime,
    /// Current lifecycle state.
    pub state: TeammateState,
    /// Whether this teammate leads the team (`LeaderWorker` mode only).
    pub is_leader: bool,
    created_at: Instant,
}

impl Teammate {
    /// Create a teammate in the [`TeammateState::Idle`] state.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        role: impl Into<String>,
        lifetime: Lifetime,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            role: role.into(),
            model: None,
            capabilities: HashSet::new(),
            lifetime,
            state: TeammateState::Idle,
            is_leader: false,
            created_at: Instant::now(),
        }
    }

    /// Add a capability.
    pub fn add_capability(&mut self, cap: Capability) {
        self.capabilities.insert(cap);
    }

    /// Whether this teammate declares a specific capability.
    #[must_use]
    pub fn has_capability(&self, cap: &Capability) -> bool {
        self.capabilities.contains(cap)
    }

    /// Whether this teammate declares a capability by name.
    #[must_use]
    pub fn has_capability_named(&self, name: &str) -> bool {
        self.capabilities.iter().any(|c| c.0 == name)
    }

    /// Whether this teammate is actively running.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.state == TeammateState::Running
    }

    /// Transition to a new state.
    pub fn set_state(&mut self, state: TeammateState) {
        self.state = state;
    }

    /// Wall-clock time since creation.
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        self.created_at.elapsed()
    }
}

impl std::fmt::Display for Teammate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}({}, {})", self.name, self.id, self.state)
    }
}

/// The session's roster of spawned teammates.
///
/// Every session has exactly one team; there is no team lifecycle to manage.
/// Both ephemeral and resident teammates are members, so the roster, the
/// routing rules, and the TUI view all read from one place.
#[derive(Debug, Default)]
pub struct Team {
    pub name: String,
    pub mode: TeamMode,
    members: Vec<Teammate>,
}

impl Team {
    /// Create an empty team with the default mode (`LeaderWorker`).
    #[must_use]
    pub fn new(name: String) -> Self {
        Self {
            name,
            mode: TeamMode::default(),
            members: Vec::new(),
        }
    }

    /// Create an empty team with a specific collaboration mode.
    #[must_use]
    pub fn with_mode(name: String, mode: TeamMode) -> Self {
        Self {
            name,
            mode,
            members: Vec::new(),
        }
    }

    /// Add a teammate to the roster.
    pub fn add_member(&mut self, member: Teammate) {
        self.members.push(member);
    }

    /// Remove a teammate by id, returning it if present.
    pub fn remove(&mut self, id: &str) -> Option<Teammate> {
        let idx = self.members.iter().position(|m| m.id == id)?;
        Some(self.members.remove(idx))
    }

    /// All teammates, in spawn order.
    #[must_use]
    pub fn members(&self) -> &[Teammate] {
        &self.members
    }

    /// Look up a teammate by addressable name.
    #[must_use]
    pub fn get_member(&self, name: &str) -> Option<&Teammate> {
        self.members.iter().find(|m| m.name == name)
    }

    /// Mutable look up by addressable name.
    pub fn get_member_mut(&mut self, name: &str) -> Option<&mut Teammate> {
        self.members.iter_mut().find(|m| m.name == name)
    }

    /// Look up a teammate by backend id.
    #[must_use]
    pub fn by_id(&self, id: &str) -> Option<&Teammate> {
        self.members.iter().find(|m| m.id == id)
    }

    /// Mutable look up by backend id.
    pub fn by_id_mut(&mut self, id: &str) -> Option<&mut Teammate> {
        self.members.iter_mut().find(|m| m.id == id)
    }

    /// Ids of every teammate sharing an addressable name.
    #[must_use]
    pub fn ids_named(&self, name: &str) -> Vec<String> {
        self.members
            .iter()
            .filter(|m| m.name == name)
            .map(|m| m.id.clone())
            .collect()
    }

    /// Ids of every teammate on the roster.
    #[must_use]
    pub fn all_ids(&self) -> Vec<String> {
        self.members.iter().map(|m| m.id.clone()).collect()
    }

    /// The team leader (first member with `is_leader = true`).
    #[must_use]
    pub fn leader(&self) -> Option<&Teammate> {
        self.members.iter().find(|m| m.is_leader)
    }

    /// Members declaring a specific capability.
    #[must_use]
    pub fn members_with_capability(&self, cap: &Capability) -> Vec<&Teammate> {
        self.members
            .iter()
            .filter(|m| m.has_capability(cap))
            .collect()
    }

    /// Members declaring a capability by name.
    #[must_use]
    pub fn members_with_capability_named(&self, name: &str) -> Vec<&Teammate> {
        self.members
            .iter()
            .filter(|m| m.has_capability_named(name))
            .collect()
    }

    /// Whether `from` may send a message to `to` under the team's mode.
    ///
    /// Both arguments are teammate **ids**, not addressable names: names can
    /// repeat across a respawn, and routing keys mailboxes by id, so the rule
    /// check has to agree with the mailbox it guards.
    ///
    /// In `LeaderWorker` mode only the leader can send to workers and workers
    /// can only reply to the leader. In `PeerToPeer` mode anyone may send to
    /// anyone. A team with no leader falls back to permitting any member pair,
    /// so a roster without a designated leader is not silently cut off.
    #[must_use]
    pub fn can_communicate(&self, from: &str, to: &str) -> bool {
        let (Some(f), Some(t)) = (self.by_id(from), self.by_id(to)) else {
            return false;
        };
        match self.mode {
            TeamMode::PeerToPeer => true,
            TeamMode::LeaderWorker => self.leader().is_none() || f.is_leader || t.is_leader,
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

    fn alice() -> Teammate {
        let mut m = Teammate::new("a1", "Alice", "code_reviewer", Lifetime::Resident);
        m.is_leader = true;
        m.model = Some("claude-3".into());
        m.add_capability(Capability::new("code_review"));
        m.add_capability(Capability::new("planning"));
        m
    }

    fn bob() -> Teammate {
        let mut m = Teammate::new("a2", "Bob", "tester", Lifetime::Resident);
        m.model = Some("gpt-4o".into());
        m.add_capability(Capability::new("code_review"));
        m.add_capability(Capability::new("testing"));
        m
    }

    fn charlie() -> Teammate {
        let mut m = Teammate::new("a3", "Charlie", "frontend", Lifetime::Resident);
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
        assert_eq!(member.id, "a1");
        assert_eq!(member.model.as_deref(), Some("claude-3"));
    }

    #[test]
    fn get_nonexistent_member() {
        let team = Team::new("team".into());
        assert!(team.get_member("nobody").is_none());
    }

    #[test]
    fn by_id_finds_member() {
        let mut team = Team::new("team".into());
        team.add_member(alice());
        assert_eq!(team.by_id("a1").unwrap().name, "Alice");
        assert!(team.by_id("nope").is_none());
    }

    #[test]
    fn remove_takes_member_off_roster() {
        let mut team = Team::new("team".into());
        team.add_member(alice());
        team.add_member(bob());
        let removed = team.remove("a1").unwrap();
        assert_eq!(removed.name, "Alice");
        assert_eq!(team.len(), 1);
        assert!(team.remove("a1").is_none());
    }

    #[test]
    fn ids_named_collects_duplicates() {
        let mut team = Team::new("team".into());
        team.add_member(alice());
        let mut second = alice();
        second.id = "a9".into();
        team.add_member(second);
        assert_eq!(team.ids_named("Alice").len(), 2);
        assert_eq!(team.all_ids().len(), 2);
    }

    #[test]
    fn team_leader() {
        let mut team = Team::new("team".into());
        team.add_member(alice());
        team.add_member(bob());
        let leader = team.leader().unwrap();
        assert_eq!(leader.name, "Alice");
    }

    #[test]
    fn team_no_leader() {
        let mut team = Team::new("team".into());
        team.add_member(bob());
        assert!(team.leader().is_none());
    }

    #[test]
    fn members_with_capability() {
        let mut team = Team::new("team".into());
        team.add_member(alice());
        team.add_member(bob());
        team.add_member(charlie());

        assert_eq!(
            team.members_with_capability(&Capability::new("code_review"))
                .len(),
            2
        );
        let frontend = team.members_with_capability_named("frontend");
        assert_eq!(frontend.len(), 1);
        assert_eq!(frontend[0].name, "Charlie");
        assert!(team.members_with_capability_named("devops").is_empty());
    }

    #[test]
    fn leader_worker_communication() {
        let mut team = Team::new("team".into());
        team.add_member(alice());
        team.add_member(bob());
        team.add_member(charlie());

        assert!(team.can_communicate("a1", "a2"));
        assert!(team.can_communicate("a2", "a1"));
        assert!(!team.can_communicate("a2", "a3"));
        assert!(!team.can_communicate("a3", "a2"));
    }

    #[test]
    fn leader_worker_without_leader_allows_any_pair() {
        // A plain session marks nobody leader; teammates must still be
        // reachable rather than silently cut off.
        let mut team = Team::new("team".into());
        team.add_member(bob());
        team.add_member(charlie());
        assert!(team.can_communicate("a2", "a3"));
    }

    #[test]
    fn peer_to_peer_communication() {
        let mut team = Team::with_mode("team".into(), TeamMode::PeerToPeer);
        team.add_member(alice());
        team.add_member(bob());
        team.add_member(charlie());

        assert!(team.can_communicate("a2", "a3"));
        assert!(team.can_communicate("a3", "a1"));
    }

    #[test]
    fn communication_with_nonmember() {
        let mut team = Team::new("team".into());
        team.add_member(alice());
        assert!(!team.can_communicate("a1", "nobody"));
        assert!(!team.can_communicate("nobody", "a1"));
    }

    #[test]
    fn get_member_mut() {
        let mut team = Team::new("team".into());
        team.add_member(bob());
        team.get_member_mut("Bob")
            .unwrap()
            .add_capability(Capability::new("devops"));
        assert!(
            team.get_member("Bob")
                .unwrap()
                .has_capability_named("devops")
        );
    }

    // ─── Lifetime ───

    #[test]
    fn lifetime_accessors() {
        let eph = Lifetime::Ephemeral {
            max_turns: Some(5),
            max_duration: Some(Duration::from_secs(30)),
        };
        assert!(eph.is_ephemeral());
        assert_eq!(eph.max_turns(), Some(5));
        assert_eq!(eph.max_duration(), Some(Duration::from_secs(30)));

        assert!(!Lifetime::Resident.is_ephemeral());
        assert_eq!(Lifetime::Resident.max_turns(), None);
        assert_eq!(Lifetime::Resident.max_duration(), None);

        let bare = Lifetime::ephemeral();
        assert!(bare.is_ephemeral());
        assert_eq!(bare.max_turns(), None);
    }

    #[test]
    fn lifetime_display() {
        assert_eq!(Lifetime::ephemeral().to_string(), "ephemeral");
        assert_eq!(Lifetime::Resident.to_string(), "resident");
    }

    // ─── TeammateState ───

    #[test]
    fn teammate_state_transitions() {
        let mut t = Teammate::new("t-1", "Alice", "reviewer", Lifetime::Resident);
        assert_eq!(t.state, TeammateState::Idle);
        assert!(!t.is_running());

        t.set_state(TeammateState::Running);
        assert!(t.is_running());
        assert!(!t.state.is_terminal());

        t.set_state(TeammateState::Done);
        assert!(t.state.is_terminal());
    }

    #[test]
    fn teammate_state_display_and_terminal() {
        assert_eq!(TeammateState::Idle.to_string(), "idle");
        assert_eq!(TeammateState::Running.to_string(), "running");
        assert_eq!(TeammateState::Done.to_string(), "done");
        assert_eq!(TeammateState::Failed.to_string(), "failed");
        assert_eq!(TeammateState::Stopped.to_string(), "stopped");

        assert!(!TeammateState::Idle.is_terminal());
        assert!(!TeammateState::Running.is_terminal());
        assert!(TeammateState::Failed.is_terminal());
        assert!(TeammateState::Stopped.is_terminal());
    }

    #[test]
    fn teammate_state_serde_roundtrip() {
        for state in [
            TeammateState::Idle,
            TeammateState::Running,
            TeammateState::Done,
            TeammateState::Failed,
            TeammateState::Stopped,
        ] {
            let json = serde_json::to_string(&state).unwrap();
            let parsed: TeammateState = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, state);
        }
    }

    #[test]
    fn teammate_display_and_elapsed() {
        let t = Teammate::new("t-1", "Alice", "reviewer", Lifetime::Resident);
        let s = format!("{t}");
        assert!(s.contains("Alice"));
        assert!(s.contains("t-1"));
        assert!(s.contains("idle"));
        assert!(t.elapsed().as_secs() < 1);
    }

    // ─── Capability ───

    #[test]
    fn capability_new_and_equality() {
        let cap = Capability::new("testing");
        assert_eq!(cap.name(), "testing");
        assert_eq!(cap.to_string(), "testing");
        assert_eq!(cap, Capability::new("testing"));
        assert_ne!(cap, Capability::new("planning"));
    }

    #[test]
    fn capability_serde_roundtrip() {
        let cap = Capability::new("code_review");
        let json = serde_json::to_string(&cap).unwrap();
        let parsed: Capability = serde_json::from_str(&json).unwrap();
        assert_eq!(cap, parsed);
    }

    #[test]
    fn team_mode_default_display_and_serde() {
        assert_eq!(TeamMode::default(), TeamMode::LeaderWorker);
        assert_eq!(TeamMode::LeaderWorker.to_string(), "leader-worker");
        assert_eq!(TeamMode::PeerToPeer.to_string(), "peer-to-peer");
        for mode in [TeamMode::LeaderWorker, TeamMode::PeerToPeer] {
            let json = serde_json::to_string(&mode).unwrap();
            let parsed: TeamMode = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, mode);
        }
    }
}
