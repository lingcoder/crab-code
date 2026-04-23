//! Tmux pane creation, layout, and lifecycle management.
//!
//! [`PaneManager`] wraps the `tmux` CLI to create, kill, and resize panes
//! for sub-agent teammates. Each pane is tracked by its tmux pane ID and
//! associated teammate ID.

use std::collections::HashMap;

use crab_process::spawn::{SpawnOptions, SpawnOutput};

/// Metadata for a single tmux pane.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct PaneInfo {
    /// Tmux pane identifier (e.g. `%42`).
    pub pane_id: String,
    /// The teammate running in this pane.
    pub teammate_id: String,
    /// Current pane width in columns.
    pub width: u16,
    /// Current pane height in rows.
    pub height: u16,
}

/// Manages tmux panes for the swarm backend.
///
/// Tracks the tmux session and window where teammate panes live, and
/// provides methods to create, kill, resize, and list panes.
#[allow(dead_code)]
pub struct PaneManager {
    /// Tmux session name (e.g. `crab-swarm`).
    session_name: String,
    /// Tmux window identifier within the session.
    window_id: String,
    /// Active panes, keyed by pane ID.
    panes: HashMap<String, PaneInfo>,
}

#[allow(dead_code)]
impl PaneManager {
    /// Create a new pane manager for the given tmux session and window.
    #[must_use]
    pub fn new(session_name: impl Into<String>, window_id: impl Into<String>) -> Self {
        Self {
            session_name: session_name.into(),
            window_id: window_id.into(),
            panes: HashMap::new(),
        }
    }

    /// The tmux session name.
    #[must_use]
    pub fn session_name(&self) -> &str {
        &self.session_name
    }

    /// The tmux window ID.
    #[must_use]
    pub fn window_id(&self) -> &str {
        &self.window_id
    }

    /// Create a new tmux pane for the given teammate.
    ///
    /// Runs `tmux split-window` and captures the new pane ID from stdout.
    ///
    /// # Errors
    ///
    /// Returns an error if the `tmux` command fails.
    #[allow(clippy::literal_string_with_formatting_args)]
    pub async fn create_pane(&mut self, teammate_id: &str) -> crab_core::Result<String> {
        let target = format!("{}:{}", self.session_name, self.window_id);
        let output = run_tmux(&["split-window", "-t", &target, "-P", "-F", "#{pane_id}"]).await?;

        let pane_id = output.stdout.trim().to_owned();
        if pane_id.is_empty() {
            return Err(crab_core::Error::Other(
                "tmux split-window returned empty pane ID".into(),
            ));
        }

        self.panes.insert(
            pane_id.clone(),
            PaneInfo {
                pane_id: pane_id.clone(),
                teammate_id: teammate_id.to_owned(),
                width: 80,
                height: 24,
            },
        );

        Ok(pane_id)
    }

    /// Kill a tmux pane by its pane ID.
    ///
    /// # Errors
    ///
    /// Returns an error if the `tmux` command fails.
    pub async fn kill_pane(&mut self, pane_id: &str) -> crab_core::Result<()> {
        run_tmux(&["kill-pane", "-t", pane_id]).await?;
        self.panes.remove(pane_id);
        Ok(())
    }

    /// Resize a tmux pane to the specified dimensions.
    ///
    /// # Errors
    ///
    /// Returns an error if the `tmux` command fails or the pane is not tracked.
    pub async fn resize_pane(
        &mut self,
        pane_id: &str,
        width: u16,
        height: u16,
    ) -> crab_core::Result<()> {
        let w = width.to_string();
        let h = height.to_string();
        run_tmux(&["resize-pane", "-t", pane_id, "-x", &w, "-y", &h]).await?;

        if let Some(info) = self.panes.get_mut(pane_id) {
            info.width = width;
            info.height = height;
        }

        Ok(())
    }

    /// Send keystrokes to a tmux pane.
    ///
    /// # Errors
    ///
    /// Returns an error if the `tmux` command fails.
    pub async fn send_keys(&self, pane_id: &str, keys: &str) -> crab_core::Result<()> {
        run_tmux(&["send-keys", "-t", pane_id, keys, "Enter"]).await?;
        Ok(())
    }

    /// List all tracked panes.
    #[must_use]
    pub fn list_panes(&self) -> Vec<&PaneInfo> {
        self.panes.values().collect()
    }

    /// Look up a pane by its ID.
    #[must_use]
    pub fn get_pane(&self, pane_id: &str) -> Option<&PaneInfo> {
        self.panes.get(pane_id)
    }

    /// Number of active panes.
    #[must_use]
    pub fn pane_count(&self) -> usize {
        self.panes.len()
    }
}

/// Run a `tmux` command and return its output.
///
/// # Errors
///
/// Returns an error if the process cannot be spawned or exits with a
/// non-zero status.
async fn run_tmux(args: &[&str]) -> crab_core::Result<SpawnOutput> {
    let output = crab_process::spawn::run(SpawnOptions {
        command: "tmux".into(),
        args: args.iter().map(|s| (*s).to_owned()).collect(),
        working_dir: None,
        env: Vec::new(),
        timeout: Some(std::time::Duration::from_secs(10)),
        stdin_data: None,
        clear_env: false,
        kill_grace_period: None,
    })
    .await?;

    if output.exit_code != 0 {
        return Err(crab_core::Error::Other(format!(
            "tmux command failed (exit {}): {}",
            output.exit_code,
            output.stderr.trim()
        )));
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_manager_new() {
        let pm = PaneManager::new("crab-swarm", "0");
        assert_eq!(pm.session_name(), "crab-swarm");
        assert_eq!(pm.window_id(), "0");
        assert_eq!(pm.pane_count(), 0);
    }

    #[test]
    fn list_panes_empty() {
        let pm = PaneManager::new("session", "0");
        assert!(pm.list_panes().is_empty());
    }

    #[test]
    fn pane_info_fields() {
        let info = PaneInfo {
            pane_id: "%42".into(),
            teammate_id: "t-1".into(),
            width: 120,
            height: 40,
        };
        assert_eq!(info.pane_id, "%42");
        assert_eq!(info.teammate_id, "t-1");
        assert_eq!(info.width, 120);
        assert_eq!(info.height, 40);
    }

    #[test]
    fn get_pane_not_found() {
        let pm = PaneManager::new("session", "0");
        assert!(pm.get_pane("%99").is_none());
    }
}
