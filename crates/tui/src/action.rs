use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    // ─── Application ───
    Quit,
    Redraw,
    ClearScreen,

    // ─── Input / Composer ───
    Submit,
    InputSubmit,
    InputCancel,
    InputClear,
    InputKillToEnd,
    InputYank,
    InputInsertNewline,
    NewLine,

    // ─── Input history ───
    HistoryPrev,
    HistoryNext,
    HistorySearch,

    // ─── Session ───
    NewSession,
    NextSession,
    PrevSession,

    // ─── Sidebar ───
    ToggleSidebar,

    // ─── Cancel / Interrupt ───
    Cancel,
    KillAgents,

    // ─── Scroll ───
    ScrollUp,
    ScrollDown,
    ScrollHome,
    ScrollEnd,
    ScrollToBottom,

    // ─── Permission ───
    PermissionAllow,
    PermissionDeny,
    PermissionAllowSession,
    PermissionAllowAlways,
    CyclePermissionMode,

    // ─── Fold ───
    ToggleFold,
    FoldOutput,
    UnfoldOutput,

    // ─── Copy ───
    CopyCodeBlock,
    SelectionCopy,

    // ─── Search ───
    Search,
    SearchNext,
    SearchPrev,
    OpenGlobalSearch,

    // ─── Tab completion ───
    TabComplete,
    TabCompleteNext,
    TabCompletePrev,

    // ─── Popup ───
    PopupMoveUp,
    PopupMoveDown,
    PopupAccept,
    PopupClose,

    // ─── Overlay (generic) ───
    CloseOverlay,
    OverlayMoveUp,
    OverlayMoveDown,
    OverlayPageUp,
    OverlayPageDown,
    OverlayJumpTop,
    OverlayJumpBottom,
    OverlaySelect,

    // ─── Open overlays / panels ───
    OpenDiffViewer,
    OpenTranscript,
    ToggleTranscript,
    ToggleTodos,
    OpenTaskList,
    OpenCommandPalette,
    OpenHelp,
    OpenAgentDetail,
    ModelPicker,
    ExternalEditor,

    // ─── Selection mode ───
    EnterSelectionMode,
    ExitSelectionMode,

    // ─── Message actions ───
    MessageCopy,
    MessageEdit,
    MessageDelete,
    MessageRewind,

    // ─── Vim ───
    ToggleVimMode,

    // ─── Misc ───
    Stash,
    CycleMode,
    ImagePaste,
    Undo,
    Redo,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_serde_roundtrip() {
        for action in [
            Action::Quit,
            Action::InputSubmit,
            Action::PermissionAllowSession,
            Action::OpenGlobalSearch,
            Action::MessageRewind,
        ] {
            let json = serde_json::to_string(&action).unwrap();
            let back: Action = serde_json::from_str(&json).unwrap();
            assert_eq!(action, back);
        }
    }

    #[test]
    fn action_json_schema_generates() {
        let schema = schemars::schema_for!(Action);
        let json = serde_json::to_string_pretty(&schema).unwrap();
        assert!(json.contains("quit"));
        assert!(json.contains("input_submit"));
    }
}
