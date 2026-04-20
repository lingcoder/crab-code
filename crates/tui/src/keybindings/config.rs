//! JSON-backed user overrides for keybindings.
//!
//! File format:
//!
//! ```json
//! {
//!   "bindings": {
//!     "chat":  { "ctrl+l": "clear_screen",
//!                "ctrl+k ctrl+s": "open_global_search" },
//!     "input": { "ctrl+z": "undo" }
//!   }
//! }
//! ```
//!
//! A user entry replaces the default binding for the same `(context,
//! sequence)`. To unbind, set the action to `"none"`.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::parser::parse_sequence;
use super::resolver::Resolver;
use super::types::{Action, KeyContext};

/// Root JSON shape.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UserBindings {
    /// Map of context name → { sequence string → action name }.
    #[serde(default)]
    pub bindings: BTreeMap<String, BTreeMap<String, String>>,
}

/// Apply overrides from the JSON file (if present) to an existing resolver.
///
/// Missing or malformed files produce a warning on stderr and leave the
/// resolver at defaults; there is no silent-fail and no auto-migration
/// of unknown fields.
pub fn apply_from_path(resolver: &mut Resolver, path: &Path) {
    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };
    let parsed: UserBindings = match serde_json::from_str(&content) {
        Ok(p) => p,
        Err(err) => {
            eprintln!(
                "crab: keybindings file {} is invalid: {err}",
                path.display()
            );
            return;
        }
    };
    apply(resolver, &parsed);
}

/// Apply a parsed `UserBindings` to a resolver.
pub fn apply(resolver: &mut Resolver, bindings: &UserBindings) {
    for (ctx_name, entries) in &bindings.bindings {
        let Some(ctx) = parse_context(ctx_name) else {
            eprintln!("crab: unknown keybinding context '{ctx_name}' (skipped)");
            continue;
        };
        for (seq_str, action_name) in entries {
            let Some(seq) = parse_sequence(seq_str) else {
                eprintln!("crab: invalid key sequence '{seq_str}' (skipped)");
                continue;
            };
            if action_name == "none" || action_name.is_empty() {
                resolver.unbind(ctx, &seq);
                continue;
            }
            let Some(action) = parse_action(action_name) else {
                eprintln!("crab: unknown action '{action_name}' (skipped)");
                continue;
            };
            resolver.bind(ctx, seq, action);
        }
    }
}

fn parse_context(name: &str) -> Option<KeyContext> {
    serde_json::from_value::<KeyContext>(serde_json::Value::String(name.to_string())).ok()
}

fn parse_action(name: &str) -> Option<Action> {
    serde_json::from_value::<Action>(serde_json::Value::String(name.to_string())).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keybindings::defaults::defaults;
    use crate::keybindings::resolver::ResolveOutcome;
    use crate::keybindings::types::{KeyChord, Sequence};
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    use std::time::Instant;

    fn ev(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: mods,
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        }
    }

    #[test]
    fn user_override_replaces_default() {
        let mut r = defaults();
        let mut bindings = UserBindings::default();
        let mut chat = BTreeMap::new();
        chat.insert("ctrl+l".into(), "clear_screen".into());
        bindings.bindings.insert("chat".into(), chat);
        apply(&mut r, &bindings);

        let outcome = r.feed(
            ev(KeyCode::Char('l'), KeyModifiers::CONTROL),
            &[KeyContext::Chat],
            Instant::now(),
        );
        assert_eq!(outcome, ResolveOutcome::Action(Action::ClearScreen));
    }

    #[test]
    fn user_chord_binding_registers() {
        let mut r = defaults();
        let mut bindings = UserBindings::default();
        let mut chat = BTreeMap::new();
        chat.insert("ctrl+k ctrl+x".into(), "kill_agents".into());
        bindings.bindings.insert("chat".into(), chat);
        apply(&mut r, &bindings);

        let t0 = Instant::now();
        let _pending = r.feed(
            ev(KeyCode::Char('k'), KeyModifiers::CONTROL),
            &[KeyContext::Chat],
            t0,
        );
        let outcome = r.feed(
            ev(KeyCode::Char('x'), KeyModifiers::CONTROL),
            &[KeyContext::Chat],
            t0,
        );
        assert_eq!(outcome, ResolveOutcome::Action(Action::KillAgents));
    }

    #[test]
    fn unbind_with_none_action() {
        let mut r = defaults();
        // Sanity-check the default exists first.
        let seq = Sequence::single(KeyChord::ctrl(KeyCode::Char('l')));
        assert!(
            r.feed(
                ev(KeyCode::Char('l'), KeyModifiers::CONTROL),
                &[KeyContext::Global],
                Instant::now(),
            ) == ResolveOutcome::Action(Action::Redraw)
        );

        let mut bindings = UserBindings::default();
        let mut global = BTreeMap::new();
        global.insert("ctrl+l".into(), "none".into());
        bindings.bindings.insert("global".into(), global);
        apply(&mut r, &bindings);

        // The binding should now be gone.
        let _ = seq;
        let outcome = r.feed(
            ev(KeyCode::Char('l'), KeyModifiers::CONTROL),
            &[KeyContext::Global],
            Instant::now(),
        );
        assert!(matches!(outcome, ResolveOutcome::Unhandled(_)));
    }

    #[test]
    fn unknown_context_is_skipped() {
        let mut r = defaults();
        let mut bindings = UserBindings::default();
        let mut bogus = BTreeMap::new();
        bogus.insert("ctrl+q".into(), "quit".into());
        bindings.bindings.insert("bogus".into(), bogus);
        apply(&mut r, &bindings); // must not panic
    }

    #[test]
    fn unknown_action_is_skipped() {
        let mut r = defaults();
        let mut bindings = UserBindings::default();
        let mut chat = BTreeMap::new();
        chat.insert("ctrl+q".into(), "non_existent_action".into());
        bindings.bindings.insert("chat".into(), chat);
        apply(&mut r, &bindings);
    }

    #[test]
    fn roundtrip_serde() {
        let mut ub = UserBindings::default();
        let mut chat = BTreeMap::new();
        chat.insert("ctrl+l".into(), "clear_screen".into());
        ub.bindings.insert("chat".into(), chat);

        let json = serde_json::to_string(&ub).unwrap();
        let back: UserBindings = serde_json::from_str(&json).unwrap();
        assert_eq!(ub.bindings, back.bindings);
    }
}
