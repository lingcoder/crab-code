//! Pseudo-terminal allocation via `portable-pty`.
//!
//! Gated behind `feature = "pty"`.

/// A running process attached to a pseudo-terminal.
pub struct PtyProcess {
    _private: (),
}

/// Options for spawning a PTY process.
#[derive(Debug, Clone)]
pub struct PtyOptions {
    pub command: String,
    pub args: Vec<String>,
    pub working_dir: Option<std::path::PathBuf>,
    pub env: Vec<(String, String)>,
    pub rows: u16,
    pub cols: u16,
}

impl PtyProcess {
    /// Spawn a command in a new pseudo-terminal.
    pub fn spawn(_opts: PtyOptions) -> crab_common::Result<Self> {
        todo!()
    }

    /// Write data to the PTY stdin.
    pub fn write(&mut self, _data: &[u8]) -> crab_common::Result<()> {
        todo!()
    }

    /// Resize the PTY.
    pub fn resize(&self, _rows: u16, _cols: u16) -> crab_common::Result<()> {
        todo!()
    }

    /// Kill the PTY process.
    pub fn kill(&mut self) -> crab_common::Result<()> {
        todo!()
    }
}
