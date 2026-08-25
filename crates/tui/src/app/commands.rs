//! Slash-command surface and overlay routing for the App.

use super::App;
use super::state::{AppState, ChatMessage};
use crate::components::autocomplete::CommandInfo;
use crate::history::cells::SystemKind;

impl App {
    /// Push a system-level message cell (used for slash command output,
    /// lifecycle announcements, and similar non-conversational text).
    pub fn push_system_message(&mut self, text: impl Into<String>) {
        self.messages.push(ChatMessage::System {
            text: text.into(),
            kind: SystemKind::Info,
        });
    }

    /// Register slash commands for Tab completion.
    pub fn set_slash_commands(&mut self, commands: Vec<CommandInfo>) {
        self.autocomplete.set_commands(commands);
    }

    /// Set the working directory for file path completion.
    pub fn set_completion_cwd(&mut self, cwd: impl Into<std::path::PathBuf>) {
        self.autocomplete.set_cwd(cwd);
    }

    /// Open the overlay corresponding to a slash command's requested kind.
    ///
    /// Refuses while a permission dialog is pending (`AppState::Confirming`);
    /// users must answer the dialog first. Overlays whose data source is not
    /// yet available (no memory dir, no MCP registry, no diff in recent tool
    /// output, …) emit a toast instead of pushing an empty overlay.
    pub fn open_overlay_by_kind(&mut self, kind: crab_commands::OverlayKind) {
        use crab_commands::OverlayKind;
        if self.state == AppState::Confirming {
            self.notifications
                .warn("Resolve the pending permission prompt first");
            return;
        }
        match kind {
            OverlayKind::Help => {
                let overlay = crate::components::shortcut_hint::HelpOverlay::new();
                self.overlay_stack.push(Box::new(overlay));
            }
            OverlayKind::Model => {
                // Model list is currently a static set matching what the
                // backend selectors know about. Enumerating every provider's
                // catalogue belongs in a future /model discovery path.
                let models = vec![
                    "claude-opus-4-6".to_string(),
                    "claude-sonnet-4-6".to_string(),
                    "claude-haiku-4-5-20251001".to_string(),
                    "gpt-4o".to_string(),
                    "deepseek-chat".to_string(),
                ];
                let overlay = crate::components::model_picker::ModelPickerOverlay::new(
                    models,
                    self.model_name.clone(),
                );
                self.overlay_stack.push(Box::new(overlay));
            }
            OverlayKind::Memory => {
                let Some(dir) = self.memory_dir.clone() else {
                    self.notifications.warn("Memory directory not configured");
                    return;
                };
                let entries = crate::components::memory_browser::load_memories(&dir);
                let overlay = crate::components::memory_browser::MemoryBrowserOverlay::new(entries);
                self.overlay_stack.push(Box::new(overlay));
            }
            OverlayKind::Mcp => {
                let Some(registry) = self.tool_registry.clone() else {
                    self.notifications.warn("Tool registry not yet initialized");
                    return;
                };
                let servers = crate::components::mcp_browser::load_mcp_servers(&registry);
                let overlay = crate::components::mcp_browser::McpBrowserOverlay::new(servers);
                self.overlay_stack.push(Box::new(overlay));
            }
            OverlayKind::Team => {
                let overlay = self.build_team_browser();
                self.overlay_stack.push(Box::new(overlay));
            }
            OverlayKind::Diff => {
                let Some(diff_text) = self.latest_diff_text() else {
                    self.notifications.warn("No diff in recent tool output");
                    return;
                };
                let overlay = crate::components::diff_viewer::DiffViewerOverlay::from_unified_diff(
                    &diff_text,
                );
                self.overlay_stack.push(Box::new(overlay));
            }
            OverlayKind::Config => {
                let overlay = crate::components::config_browser::ConfigBrowserOverlay::new(
                    self.model_name.clone(),
                    self.permission_mode,
                    self.working_dir.clone(),
                    self.memory_dir.clone(),
                );
                self.overlay_stack.push(Box::new(overlay));
            }
            OverlayKind::Permissions => {
                let overlay =
                    crate::components::permissions_browser::PermissionsBrowserOverlay::new(
                        self.permission_mode,
                        self.session_grants.iter().cloned().collect(),
                    );
                self.overlay_stack.push(Box::new(overlay));
            }
            OverlayKind::Resume => {
                let sessions = self.session_sidebar.sessions.clone();
                let overlay = crate::components::resume_browser::ResumeBrowserOverlay::new(
                    sessions,
                    self.session_id.clone(),
                );
                self.overlay_stack.push(Box::new(overlay));
            }
        }
    }
}
