//! Chord-aware keybinding resolver.
//!
//! Feed each key event in turn; receive one of:
//!
//! - `Action` — a binding matched.
//! - `PendingChord` — the current key is a prefix of a multi-chord binding;
//!   the caller should display a hint and wait for the next key.
//! - `Timeout` — a pending prefix expired before completion; the caller
//!   should erase any hint.
//! - `Unhandled` — no binding matched; the caller bubbles the event up.
//!
//! Matching priority:
//!
//! 1. Longer sequences (chord bindings) take priority over shorter ones.
//! 2. Within the same length, the innermost `KeyContext` wins.
//! 3. Ties within a single context are resolved by insertion order; this
//!    matters only for user overrides, where user entries replace defaults.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crossterm::event::KeyEvent;

use super::types::{Action, KeyChord, KeyContext, Sequence};

/// Default pending-chord expiry.
pub const DEFAULT_CHORD_TIMEOUT: Duration = Duration::from_millis(1500);

/// Single entry in the resolver's binding table.
#[derive(Debug, Clone)]
struct Binding {
    context: KeyContext,
    sequence: Sequence,
    action: Action,
}

/// Outcome of feeding one key event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveOutcome {
    /// A full binding matched; dispatch the `Action`.
    Action(Action),
    /// The pressed key is a prefix of at least one multi-chord binding.
    /// The caller should hint the user and call `tick` or `feed` later.
    PendingChord { prefix: Vec<KeyChord> },
    /// A pending chord prefix expired.
    Timeout,
    /// No binding matches; propagate the event.
    Unhandled(KeyEvent),
}

/// The resolver holds the binding table and any chord prefix in flight.
pub struct Resolver {
    bindings: Vec<Binding>,
    pending: Option<Vec<KeyChord>>,
    deadline: Option<Instant>,
    timeout: Duration,
}

impl Resolver {
    pub fn new() -> Self {
        Self {
            bindings: Vec::new(),
            pending: None,
            deadline: None,
            timeout: DEFAULT_CHORD_TIMEOUT,
        }
    }

    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Register a binding. Later calls override earlier bindings that share
    /// the same `(context, sequence)` key, which is how user overrides
    /// replace defaults.
    pub fn bind(&mut self, context: KeyContext, sequence: Sequence, action: Action) {
        if let Some(existing) = self
            .bindings
            .iter_mut()
            .find(|b| b.context == context && b.sequence == sequence)
        {
            existing.action = action;
            return;
        }
        self.bindings.push(Binding {
            context,
            sequence,
            action,
        });
    }

    /// Remove a binding. Useful to let user overrides unbind a default.
    pub fn unbind(&mut self, context: KeyContext, sequence: &Sequence) {
        self.bindings
            .retain(|b| !(b.context == context && b.sequence == *sequence));
    }

    pub fn len(&self) -> usize {
        self.bindings.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }

    /// Look up an exact `(context, sequence)` binding without any chord
    /// tracking. Used by the simple single-key façade for compatibility
    /// with callers that don't participate in the chord state machine.
    pub fn lookup_exact(&self, context: KeyContext, sequence: &Sequence) -> Option<Action> {
        self.bindings
            .iter()
            .find(|b| b.context == context && b.sequence == *sequence)
            .map(|b| b.action)
    }

    /// Clear any in-flight chord prefix without dispatching.
    pub fn clear_pending(&mut self) {
        self.pending = None;
        self.deadline = None;
    }

    /// Return the current chord prefix if one is in flight, for hint UI.
    pub fn pending(&self) -> Option<&[KeyChord]> {
        self.pending.as_deref()
    }

    /// Check whether the chord timeout has elapsed. If yes, clear the
    /// pending prefix and return `ResolveOutcome::Timeout`; otherwise
    /// return `None`.
    pub fn tick(&mut self, now: Instant) -> Option<ResolveOutcome> {
        if let (Some(_), Some(deadline)) = (&self.pending, self.deadline)
            && now >= deadline
        {
            self.clear_pending();
            return Some(ResolveOutcome::Timeout);
        }
        None
    }

    /// Feed one key event from the active focus chain.
    ///
    /// `focus_chain` lists the contexts to consider, innermost first.
    /// The resolver always implicitly falls back to `KeyContext::Global`
    /// when nothing in the chain matches.
    pub fn feed(
        &mut self,
        key: KeyEvent,
        focus_chain: &[KeyContext],
        now: Instant,
    ) -> ResolveOutcome {
        // Build the effective prefix including the new key.
        let mut prefix = self.pending.clone().unwrap_or_default();
        prefix.push(KeyChord::new(key.code, key.modifiers));

        // Collect per-context matches at this prefix length.
        let mut exact_match: Option<(KeyContext, Action)> = None;
        let mut has_longer_prefix = false;

        let chain: Vec<KeyContext> = focus_chain
            .iter()
            .copied()
            .chain(std::iter::once(KeyContext::Global))
            .collect();

        for &ctx in &chain {
            for b in self.bindings.iter().filter(|b| b.context == ctx) {
                if b.sequence.0.as_slice() == prefix.as_slice() {
                    if exact_match.is_none() {
                        exact_match = Some((ctx, b.action));
                    }
                } else if b.sequence.starts_with(&prefix) && b.sequence.len() > prefix.len() {
                    has_longer_prefix = true;
                }
            }
            if exact_match.is_some() && !has_longer_prefix {
                break;
            }
        }

        // Exact match + nothing longer pending: dispatch and clear.
        // (If there IS a longer chord also pending, fall through to the
        // PendingChord branch so the user can choose.)
        if let Some((_, action)) = exact_match
            && !has_longer_prefix
        {
            self.clear_pending();
            return ResolveOutcome::Action(action);
        }

        // Prefix has a longer continuation; hold and wait for next key.
        if has_longer_prefix {
            self.pending = Some(prefix.clone());
            self.deadline = Some(now + self.timeout);
            return ResolveOutcome::PendingChord { prefix };
        }

        // No match at all: drop any pending state and surface the event.
        self.clear_pending();
        ResolveOutcome::Unhandled(key)
    }
}

impl Default for Resolver {
    fn default() -> Self {
        Self::new()
    }
}

/// Index-friendly export for external inspection / docs generation.
#[must_use]
pub fn grouped_bindings(resolver: &Resolver) -> BTreeMap<KeyContext, Vec<(Sequence, Action)>> {
    let mut out: BTreeMap<KeyContext, Vec<(Sequence, Action)>> = BTreeMap::new();
    for b in &resolver.bindings {
        out.entry(b.context)
            .or_default()
            .push((b.sequence.clone(), b.action));
    }
    out
}

impl PartialOrd for KeyContext {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for KeyContext {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (*self as u8).cmp(&(*other as u8))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEventKind, KeyEventState, KeyModifiers};

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: mods,
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        }
    }

    #[test]
    fn single_key_binding_resolves() {
        let mut r = Resolver::new();
        r.bind(
            KeyContext::Chat,
            Sequence::single(KeyChord::ctrl(KeyCode::Char('l'))),
            Action::Redraw,
        );

        let outcome = r.feed(
            key(KeyCode::Char('l'), KeyModifiers::CONTROL),
            &[KeyContext::Chat],
            Instant::now(),
        );
        assert_eq!(outcome, ResolveOutcome::Action(Action::Redraw));
        assert!(r.pending().is_none());
    }

    #[test]
    fn unknown_key_is_unhandled() {
        let mut r = Resolver::new();
        let ev = key(KeyCode::Char('x'), KeyModifiers::empty());
        let outcome = r.feed(ev, &[KeyContext::Chat], Instant::now());
        assert!(matches!(outcome, ResolveOutcome::Unhandled(_)));
    }

    #[test]
    fn chord_binding_resolves_in_two_steps() {
        let mut r = Resolver::new();
        r.bind(
            KeyContext::Chat,
            Sequence::of(vec![
                KeyChord::ctrl(KeyCode::Char('k')),
                KeyChord::ctrl(KeyCode::Char('s')),
            ]),
            Action::OpenGlobalSearch,
        );

        let t0 = Instant::now();
        let out1 = r.feed(
            key(KeyCode::Char('k'), KeyModifiers::CONTROL),
            &[KeyContext::Chat],
            t0,
        );
        assert!(matches!(out1, ResolveOutcome::PendingChord { .. }));
        assert!(r.pending().is_some());

        let out2 = r.feed(
            key(KeyCode::Char('s'), KeyModifiers::CONTROL),
            &[KeyContext::Chat],
            t0 + Duration::from_millis(100),
        );
        assert_eq!(out2, ResolveOutcome::Action(Action::OpenGlobalSearch));
        assert!(r.pending().is_none());
    }

    #[test]
    fn pending_chord_times_out() {
        let mut r = Resolver::new().with_timeout(Duration::from_millis(50));
        r.bind(
            KeyContext::Chat,
            Sequence::of(vec![
                KeyChord::ctrl(KeyCode::Char('k')),
                KeyChord::ctrl(KeyCode::Char('s')),
            ]),
            Action::OpenGlobalSearch,
        );

        let t0 = Instant::now();
        r.feed(
            key(KeyCode::Char('k'), KeyModifiers::CONTROL),
            &[KeyContext::Chat],
            t0,
        );
        assert!(r.pending().is_some());

        let after = t0 + Duration::from_millis(60);
        assert_eq!(r.tick(after), Some(ResolveOutcome::Timeout));
        assert!(r.pending().is_none());
    }

    #[test]
    fn inner_context_shadows_outer() {
        let mut r = Resolver::new();
        r.bind(
            KeyContext::Global,
            Sequence::single(KeyChord::ctrl(KeyCode::Char('c'))),
            Action::Quit,
        );
        r.bind(
            KeyContext::Permission,
            Sequence::single(KeyChord::ctrl(KeyCode::Char('c'))),
            Action::PermissionDeny,
        );

        let t0 = Instant::now();
        let outcome = r.feed(
            key(KeyCode::Char('c'), KeyModifiers::CONTROL),
            &[KeyContext::Permission],
            t0,
        );
        assert_eq!(outcome, ResolveOutcome::Action(Action::PermissionDeny));
    }

    #[test]
    fn bare_key_without_chord_overlap_dispatches() {
        let mut r = Resolver::new();
        r.bind(
            KeyContext::Chat,
            Sequence::single(KeyChord::ctrl(KeyCode::Char('k'))),
            Action::KillAgents,
        );

        let outcome = r.feed(
            key(KeyCode::Char('k'), KeyModifiers::CONTROL),
            &[KeyContext::Chat],
            Instant::now(),
        );
        assert_eq!(outcome, ResolveOutcome::Action(Action::KillAgents));
    }

    #[test]
    fn chord_prefix_holds_when_ambiguous() {
        let mut r = Resolver::new();
        r.bind(
            KeyContext::Chat,
            Sequence::single(KeyChord::ctrl(KeyCode::Char('k'))),
            Action::KillAgents,
        );
        r.bind(
            KeyContext::Chat,
            Sequence::of(vec![
                KeyChord::ctrl(KeyCode::Char('k')),
                KeyChord::ctrl(KeyCode::Char('s')),
            ]),
            Action::OpenGlobalSearch,
        );

        let t0 = Instant::now();
        let out1 = r.feed(
            key(KeyCode::Char('k'), KeyModifiers::CONTROL),
            &[KeyContext::Chat],
            t0,
        );
        assert!(matches!(out1, ResolveOutcome::PendingChord { .. }));
    }

    #[test]
    fn unbind_removes_binding() {
        let mut r = Resolver::new();
        let seq = Sequence::single(KeyChord::ctrl(KeyCode::Char('l')));
        r.bind(KeyContext::Chat, seq.clone(), Action::Redraw);
        assert_eq!(r.len(), 1);
        r.unbind(KeyContext::Chat, &seq);
        assert_eq!(r.len(), 0);
    }

    #[test]
    fn rebind_replaces_existing_action() {
        let mut r = Resolver::new();
        let seq = Sequence::single(KeyChord::ctrl(KeyCode::Char('l')));
        r.bind(KeyContext::Chat, seq.clone(), Action::Redraw);
        r.bind(KeyContext::Chat, seq, Action::ClearScreen);
        assert_eq!(r.len(), 1);

        let outcome = r.feed(
            key(KeyCode::Char('l'), KeyModifiers::CONTROL),
            &[KeyContext::Chat],
            Instant::now(),
        );
        assert_eq!(outcome, ResolveOutcome::Action(Action::ClearScreen));
    }
}
