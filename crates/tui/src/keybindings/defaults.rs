//! Default keybinding table wired into a fresh `Resolver`.

use crossterm::event::{KeyCode, KeyModifiers};

use super::resolver::Resolver;
use super::types::{Action, KeyChord, KeyContext, Sequence};

/// Build a `Resolver` pre-populated with all default bindings.
#[must_use]
pub fn defaults() -> Resolver {
    let mut r = Resolver::new();
    register_global(&mut r);
    register_chat(&mut r);
    register_input(&mut r);
    register_permission(&mut r);
    register_search(&mut r);
    register_history_search(&mut r);
    register_command_palette(&mut r);
    register_model_picker(&mut r);
    register_transcript(&mut r);
    register_task_list(&mut r);
    register_help(&mut r);
    register_sidebar(&mut r);
    register_scroll_box(&mut r);
    register_output_fold(&mut r);
    register_selection_mode(&mut r);
    register_agent_detail(&mut r);
    register_diff(&mut r);
    register_global_search(&mut r);
    r
}

fn single(chord: KeyChord) -> Sequence {
    Sequence::single(chord)
}

fn chord_seq(a: KeyChord, b: KeyChord) -> Sequence {
    Sequence::of(vec![a, b])
}

fn register_global(r: &mut Resolver) {
    r.bind(
        KeyContext::Global,
        single(KeyChord::ctrl(KeyCode::Char('c'))),
        Action::Quit,
    );
    r.bind(
        KeyContext::Global,
        single(KeyChord::ctrl(KeyCode::Char('d'))),
        Action::Quit,
    );
    r.bind(
        KeyContext::Global,
        single(KeyChord::ctrl(KeyCode::Char('l'))),
        Action::Redraw,
    );
    r.bind(
        KeyContext::Global,
        single(KeyChord::plain(KeyCode::Esc)),
        Action::Cancel,
    );
}

fn register_chat(r: &mut Resolver) {
    r.bind(
        KeyContext::Chat,
        single(KeyChord::ctrl(KeyCode::Char('n'))),
        Action::NewSession,
    );
    r.bind(
        KeyContext::Chat,
        single(KeyChord::ctrl(KeyCode::Tab)),
        Action::NextSession,
    );
    r.bind(
        KeyContext::Chat,
        single(KeyChord::ctrl(KeyCode::BackTab)),
        Action::PrevSession,
    );
    r.bind(
        KeyContext::Chat,
        single(KeyChord::ctrl(KeyCode::Char('b'))),
        Action::ToggleSidebar,
    );
    r.bind(
        KeyContext::Chat,
        single(KeyChord::plain(KeyCode::PageUp)),
        Action::ScrollUp,
    );
    r.bind(
        KeyContext::Chat,
        single(KeyChord::plain(KeyCode::PageDown)),
        Action::ScrollDown,
    );
    r.bind(
        KeyContext::Chat,
        single(KeyChord::new(KeyCode::Home, KeyModifiers::CONTROL)),
        Action::ScrollHome,
    );
    r.bind(
        KeyContext::Chat,
        single(KeyChord::new(KeyCode::End, KeyModifiers::CONTROL)),
        Action::ScrollEnd,
    );
    r.bind(
        KeyContext::Chat,
        single(KeyChord::ctrl(KeyCode::Char('t'))),
        Action::ToggleTodos,
    );
    r.bind(
        KeyContext::Chat,
        single(KeyChord::ctrl(KeyCode::Char('o'))),
        Action::ToggleTranscript,
    );
    r.bind(
        KeyContext::Chat,
        single(KeyChord::ctrl(KeyCode::Char('k'))),
        Action::KillAgents,
    );
    r.bind(
        KeyContext::Chat,
        single(KeyChord::alt(KeyCode::Char('p'))),
        Action::ModelPicker,
    );
    r.bind(
        KeyContext::Chat,
        single(KeyChord::new(KeyCode::BackTab, KeyModifiers::SHIFT)),
        Action::CycleMode,
    );
    r.bind(
        KeyContext::Chat,
        single(KeyChord::alt(KeyCode::Char('v'))),
        Action::EnterSelectionMode,
    );

    // Chord bindings (Ctrl+K prefix is the common CCB pattern).
    r.bind(
        KeyContext::Chat,
        chord_seq(
            KeyChord::ctrl(KeyCode::Char('k')),
            KeyChord::ctrl(KeyCode::Char('s')),
        ),
        Action::OpenGlobalSearch,
    );
    r.bind(
        KeyContext::Chat,
        chord_seq(
            KeyChord::ctrl(KeyCode::Char('k')),
            KeyChord::ctrl(KeyCode::Char('t')),
        ),
        Action::OpenTaskList,
    );
    r.bind(
        KeyContext::Chat,
        chord_seq(
            KeyChord::ctrl(KeyCode::Char('k')),
            KeyChord::ctrl(KeyCode::Char('h')),
        ),
        Action::OpenHelp,
    );
    r.bind(
        KeyContext::Chat,
        chord_seq(
            KeyChord::ctrl(KeyCode::Char('k')),
            KeyChord::ctrl(KeyCode::Char('p')),
        ),
        Action::OpenCommandPalette,
    );
    r.bind(
        KeyContext::Chat,
        chord_seq(
            KeyChord::ctrl(KeyCode::Char('k')),
            KeyChord::ctrl(KeyCode::Char('a')),
        ),
        Action::OpenAgentDetail,
    );
}

fn register_input(r: &mut Resolver) {
    r.bind(
        KeyContext::Input,
        single(KeyChord::plain(KeyCode::Enter)),
        Action::Submit,
    );
    r.bind(
        KeyContext::Input,
        single(KeyChord::new(KeyCode::Enter, KeyModifiers::ALT)),
        Action::NewLine,
    );
    r.bind(
        KeyContext::Input,
        single(KeyChord::new(KeyCode::Enter, KeyModifiers::SHIFT)),
        Action::NewLine,
    );
    r.bind(
        KeyContext::Input,
        single(KeyChord::ctrl(KeyCode::Char('r'))),
        Action::HistorySearch,
    );
    r.bind(
        KeyContext::Input,
        single(KeyChord::ctrl(KeyCode::Char('g'))),
        Action::ExternalEditor,
    );
    r.bind(
        KeyContext::Input,
        single(KeyChord::ctrl(KeyCode::Char('s'))),
        Action::Stash,
    );
    r.bind(
        KeyContext::Input,
        single(KeyChord::ctrl(KeyCode::Char('z'))),
        Action::Undo,
    );
    r.bind(
        KeyContext::Input,
        single(KeyChord::ctrl(KeyCode::Char('_'))),
        Action::Undo,
    );
    r.bind(
        KeyContext::Input,
        single(KeyChord::ctrl(KeyCode::Char('y'))),
        Action::Redo,
    );
    r.bind(
        KeyContext::Input,
        single(KeyChord::plain(KeyCode::Tab)),
        Action::TabComplete,
    );

    #[cfg(target_os = "windows")]
    r.bind(
        KeyContext::Input,
        single(KeyChord::alt(KeyCode::Char('v'))),
        Action::ImagePaste,
    );
    #[cfg(not(target_os = "windows"))]
    r.bind(
        KeyContext::Input,
        single(KeyChord::ctrl(KeyCode::Char('v'))),
        Action::ImagePaste,
    );
}

fn register_permission(r: &mut Resolver) {
    r.bind(
        KeyContext::Permission,
        single(KeyChord::plain(KeyCode::Char('y'))),
        Action::PermissionAllow,
    );
    r.bind(
        KeyContext::Permission,
        single(KeyChord::plain(KeyCode::Char('n'))),
        Action::PermissionDeny,
    );
    r.bind(
        KeyContext::Permission,
        single(KeyChord::plain(KeyCode::Esc)),
        Action::PermissionDeny,
    );
}

fn register_search(r: &mut Resolver) {
    r.bind(
        KeyContext::Search,
        single(KeyChord::plain(KeyCode::Enter)),
        Action::SearchNext,
    );
    r.bind(
        KeyContext::Search,
        single(KeyChord::new(KeyCode::Enter, KeyModifiers::SHIFT)),
        Action::SearchPrev,
    );
    r.bind(
        KeyContext::Search,
        single(KeyChord::plain(KeyCode::F(3))),
        Action::SearchNext,
    );
    r.bind(
        KeyContext::Search,
        single(KeyChord::new(KeyCode::F(3), KeyModifiers::SHIFT)),
        Action::SearchPrev,
    );
}

fn register_history_search(r: &mut Resolver) {
    r.bind(
        KeyContext::HistorySearch,
        single(KeyChord::ctrl(KeyCode::Char('r'))),
        Action::SearchNext,
    );
    r.bind(
        KeyContext::HistorySearch,
        single(KeyChord::ctrl(KeyCode::Char('s'))),
        Action::SearchPrev,
    );
}

fn register_command_palette(r: &mut Resolver) {
    r.bind(
        KeyContext::CommandPalette,
        single(KeyChord::plain(KeyCode::Enter)),
        Action::Submit,
    );
    r.bind(
        KeyContext::CommandPalette,
        single(KeyChord::plain(KeyCode::Tab)),
        Action::TabCompleteNext,
    );
    r.bind(
        KeyContext::CommandPalette,
        single(KeyChord::new(KeyCode::Tab, KeyModifiers::SHIFT)),
        Action::TabCompletePrev,
    );
}

fn register_model_picker(r: &mut Resolver) {
    r.bind(
        KeyContext::ModelPicker,
        single(KeyChord::plain(KeyCode::Enter)),
        Action::Submit,
    );
}

fn register_transcript(r: &mut Resolver) {
    r.bind(
        KeyContext::Transcript,
        single(KeyChord::plain(KeyCode::Char('q'))),
        Action::Cancel,
    );
    r.bind(
        KeyContext::Transcript,
        single(KeyChord::ctrl(KeyCode::Char('o'))),
        Action::ToggleTranscript,
    );
}

fn register_task_list(r: &mut Resolver) {
    r.bind(
        KeyContext::TaskList,
        single(KeyChord::plain(KeyCode::Char('q'))),
        Action::Cancel,
    );
}

fn register_help(r: &mut Resolver) {
    r.bind(
        KeyContext::Help,
        single(KeyChord::plain(KeyCode::Char('q'))),
        Action::Cancel,
    );
    r.bind(
        KeyContext::Help,
        single(KeyChord::plain(KeyCode::Esc)),
        Action::Cancel,
    );
}

fn register_sidebar(r: &mut Resolver) {
    r.bind(
        KeyContext::Sidebar,
        single(KeyChord::plain(KeyCode::Char('q'))),
        Action::ToggleSidebar,
    );
}

fn register_scroll_box(r: &mut Resolver) {
    r.bind(
        KeyContext::ScrollBox,
        single(KeyChord::plain(KeyCode::Up)),
        Action::ScrollUp,
    );
    r.bind(
        KeyContext::ScrollBox,
        single(KeyChord::plain(KeyCode::Down)),
        Action::ScrollDown,
    );
    r.bind(
        KeyContext::ScrollBox,
        single(KeyChord::plain(KeyCode::PageUp)),
        Action::ScrollUp,
    );
    r.bind(
        KeyContext::ScrollBox,
        single(KeyChord::plain(KeyCode::PageDown)),
        Action::ScrollDown,
    );
    r.bind(
        KeyContext::ScrollBox,
        single(KeyChord::plain(KeyCode::Home)),
        Action::ScrollHome,
    );
    r.bind(
        KeyContext::ScrollBox,
        single(KeyChord::plain(KeyCode::End)),
        Action::ScrollEnd,
    );
}

fn register_output_fold(r: &mut Resolver) {
    r.bind(
        KeyContext::OutputFold,
        single(KeyChord::plain(KeyCode::Char(' '))),
        Action::ToggleFold,
    );
    r.bind(
        KeyContext::OutputFold,
        single(KeyChord::plain(KeyCode::Enter)),
        Action::ToggleFold,
    );
}

fn register_selection_mode(r: &mut Resolver) {
    r.bind(
        KeyContext::SelectionMode,
        single(KeyChord::plain(KeyCode::Esc)),
        Action::ExitSelectionMode,
    );
    r.bind(
        KeyContext::SelectionMode,
        single(KeyChord::plain(KeyCode::Char('y'))),
        Action::SelectionCopy,
    );
}

fn register_agent_detail(r: &mut Resolver) {
    r.bind(
        KeyContext::AgentDetail,
        single(KeyChord::plain(KeyCode::Char('q'))),
        Action::Cancel,
    );
}

fn register_diff(r: &mut Resolver) {
    r.bind(
        KeyContext::Diff,
        single(KeyChord::plain(KeyCode::Char('q'))),
        Action::Cancel,
    );
    r.bind(
        KeyContext::Diff,
        single(KeyChord::ctrl(KeyCode::Char('y'))),
        Action::CopyCodeBlock,
    );
}

fn register_global_search(r: &mut Resolver) {
    r.bind(
        KeyContext::GlobalSearch,
        single(KeyChord::plain(KeyCode::Enter)),
        Action::Submit,
    );
    r.bind(
        KeyContext::GlobalSearch,
        single(KeyChord::plain(KeyCode::Esc)),
        Action::Cancel,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyEventKind, KeyEventState};
    use std::time::Instant;

    use crate::keybindings::resolver::ResolveOutcome;

    fn key_event(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: mods,
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        }
    }

    #[test]
    fn defaults_binds_quit() {
        let mut r = defaults();
        let outcome = r.feed(
            key_event(KeyCode::Char('c'), KeyModifiers::CONTROL),
            &[KeyContext::Chat],
            Instant::now(),
        );
        assert_eq!(outcome, ResolveOutcome::Action(Action::Quit));
    }

    #[test]
    fn defaults_binds_new_session() {
        let mut r = defaults();
        let outcome = r.feed(
            key_event(KeyCode::Char('n'), KeyModifiers::CONTROL),
            &[KeyContext::Chat],
            Instant::now(),
        );
        assert_eq!(outcome, ResolveOutcome::Action(Action::NewSession));
    }

    #[test]
    fn defaults_permission_y_n() {
        let mut r = defaults();
        let t0 = Instant::now();
        assert_eq!(
            r.feed(
                key_event(KeyCode::Char('y'), KeyModifiers::NONE),
                &[KeyContext::Permission],
                t0,
            ),
            ResolveOutcome::Action(Action::PermissionAllow)
        );
        assert_eq!(
            r.feed(
                key_event(KeyCode::Char('n'), KeyModifiers::NONE),
                &[KeyContext::Permission],
                t0,
            ),
            ResolveOutcome::Action(Action::PermissionDeny)
        );
    }

    #[test]
    fn defaults_ctrl_k_ctrl_s_opens_global_search() {
        let mut r = defaults();
        let t0 = Instant::now();
        let first = r.feed(
            key_event(KeyCode::Char('k'), KeyModifiers::CONTROL),
            &[KeyContext::Chat],
            t0,
        );
        assert!(matches!(first, ResolveOutcome::PendingChord { .. }));
        let second = r.feed(
            key_event(KeyCode::Char('s'), KeyModifiers::CONTROL),
            &[KeyContext::Chat],
            t0,
        );
        assert_eq!(second, ResolveOutcome::Action(Action::OpenGlobalSearch));
    }
}
