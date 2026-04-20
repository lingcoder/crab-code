//! Configurable, chord-aware keybinding system.
//!
//! Contexts are organized along the focus chain (innermost → outermost),
//! with `KeyContext::Global` as the implicit outermost fallback. Chord
//! sequences like `Ctrl+K Ctrl+S` resolve progressively: feed each key,
//! receive `PendingChord` until the sequence completes, times out, or
//! disambiguates.
//!
//! Load order:
//!
//! 1. [`defaults::defaults`] populates a new [`Resolver`] with the
//!    built-in bindings for every context.
//! 2. [`config::apply_from_path`] layers user overrides from
//!    `~/.crab/keybindings.json` on top.

pub mod config;
pub mod defaults;
pub mod parser;
pub mod resolver;
pub mod types;

pub use config::{UserBindings, apply as apply_user_bindings, apply_from_path};
pub use defaults::defaults;
pub use parser::{parse_chord, parse_key_code, parse_sequence};
pub use resolver::{DEFAULT_CHORD_TIMEOUT, ResolveOutcome, Resolver, grouped_bindings};
pub use types::{Action, KeyChord, KeyCombo, KeyContext, Sequence};

use std::path::Path;
use std::time::Instant;

use crossterm::event::KeyEvent;

/// Convenience façade that wraps a `Resolver` and a single-context chain.
///
/// Callers that do not yet use the chord / focus-chain APIs can keep
/// working with this struct and gradually migrate.
pub struct Keybindings {
    resolver: Resolver,
}

impl Keybindings {
    /// Build a resolver with default bindings only.
    #[must_use]
    pub fn defaults() -> Self {
        Self {
            resolver: defaults::defaults(),
        }
    }

    /// Build a resolver with defaults plus user overrides from `path`.
    #[must_use]
    pub fn load_from_file(path: &Path) -> Self {
        let mut resolver = defaults::defaults();
        config::apply_from_path(&mut resolver, path);
        Self { resolver }
    }

    /// Single-key resolve without chord support (legacy single-call API).
    ///
    /// Returns `Some(Action)` only when the pressed key matches a binding
    /// of length 1. Multi-chord bindings are ignored by this shortcut and
    /// must be accessed through [`Self::resolver_mut`] + [`Resolver::feed`].
    #[must_use]
    pub fn resolve(
        &self,
        code: crossterm::event::KeyCode,
        modifiers: crossterm::event::KeyModifiers,
    ) -> Option<Action> {
        let chord = KeyChord::new(code, modifiers);
        let sequence = Sequence::single(chord);
        for ctx in [KeyContext::Input, KeyContext::Chat, KeyContext::Global] {
            if let Some(action) = self.resolver.lookup_exact(ctx, &sequence) {
                return Some(action);
            }
        }
        None
    }

    /// Chord-aware feed against a focus chain (innermost first). Returns
    /// `PendingChord` while a multi-chord binding is in progress, so
    /// callers should absorb the key in that case.
    pub fn feed(&mut self, key: KeyEvent, focus_chain: &[KeyContext]) -> ResolveOutcome {
        self.resolver.feed(key, focus_chain, Instant::now())
    }

    /// Drop any pending chord prefix (for instance when an overlay
    /// opens and focus changes mid-sequence).
    pub fn clear_pending_chord(&mut self) {
        self.resolver.clear_pending();
    }

    /// Inspect the current chord prefix for hint rendering.
    #[must_use]
    pub fn pending_chord(&self) -> Option<&[KeyChord]> {
        self.resolver.pending()
    }

    /// Tick the chord-timeout state. Call once per render frame.
    pub fn tick(&mut self) -> Option<ResolveOutcome> {
        self.resolver.tick(Instant::now())
    }

    /// Full resolver access.
    pub fn resolver(&self) -> &Resolver {
        &self.resolver
    }

    pub fn resolver_mut(&mut self) -> &mut Resolver {
        &mut self.resolver
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.resolver.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.resolver.is_empty()
    }
}

impl Default for Keybindings {
    fn default() -> Self {
        Self::defaults()
    }
}
