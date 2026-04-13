//! WSL path conversion.
//!
//! `JetBrains` on Windows often reports workspace paths as
//! `\\wsl$\Ubuntu\home\user\project`, while the shell inside WSL sees
//! `/home/user/project`. This module will translate between them so
//! `IdeSelection::file_path` remains useful regardless of where the
//! crab process is running.

#![allow(dead_code)] // R1 scaffolding; wired up in R3

use std::path::{Path, PathBuf};

/// Translate an IDE-reported path into the current shell's namespace.
///
/// For R1 this is the identity function; R3 will add `\\wsl$\…`
/// translation, `C:\` ↔ `/mnt/c/` mapping, and path canonicalization.
pub fn to_local(path: &Path) -> PathBuf {
    path.to_path_buf()
}
