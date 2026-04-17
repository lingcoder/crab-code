//! `FileResourceHandler` — exposes files under a root directory as MCP
//! resources. Listing returns `file://<relative-path>` URIs; reads
//! return file contents as text with a best-effort MIME type.
//!
//! Includes path-traversal protection: URIs resolved outside the root
//! directory (via `../` or symlinks pointing elsewhere) are rejected.

use super::ResourceHandler;
use crate::protocol::{McpResource, ResourceContent, ResourceReadResult};

/// A resource handler that exposes files under a root directory as MCP resources.
///
/// Files are listed with `file://` URIs relative to the root. Only regular files
/// are listed (no directories or symlinks). Reads return file contents as text.
pub struct FileResourceHandler {
    root: std::path::PathBuf,
}

impl FileResourceHandler {
    /// Create a handler that serves files under `root`.
    pub fn new(root: std::path::PathBuf) -> Self {
        Self { root }
    }

    /// Convert a `file://` URI back to an absolute path, validating it is
    /// under the root directory (prevents path traversal).
    fn uri_to_path(&self, uri: &str) -> Result<std::path::PathBuf, String> {
        let path_str = uri
            .strip_prefix("file://")
            .ok_or_else(|| format!("unsupported URI scheme: {uri}"))?;

        // On Windows, file:// URIs may have a leading slash before the drive letter
        #[cfg(windows)]
        let path_str = path_str.strip_prefix('/').unwrap_or(path_str);

        let path = std::path::PathBuf::from(path_str);

        // Canonicalize both to prevent traversal via .. or symlinks
        let canonical = path
            .canonicalize()
            .map_err(|e| format!("cannot resolve path: {e}"))?;
        let canonical_root = self
            .root
            .canonicalize()
            .map_err(|e| format!("cannot resolve root: {e}"))?;

        if !canonical.starts_with(&canonical_root) {
            return Err(format!("path outside root directory: {uri}"));
        }

        Ok(canonical)
    }

    /// Build a `file://` URI for a path.
    pub(crate) fn path_to_uri(path: &std::path::Path) -> String {
        let s = path.to_string_lossy().replace('\\', "/");
        if s.starts_with('/') {
            format!("file://{s}")
        } else {
            format!("file:///{s}")
        }
    }

    /// Guess MIME type from file extension.
    pub(crate) fn guess_mime(path: &std::path::Path) -> Option<String> {
        let ext = path.extension()?.to_str()?;
        let mime = match ext {
            "rs" => "text/x-rust",
            "toml" => "text/x-toml",
            "json" => "application/json",
            "md" => "text/markdown",
            "yaml" | "yml" => "text/x-yaml",
            "py" => "text/x-python",
            "js" => "text/javascript",
            "ts" => "text/typescript",
            "html" | "htm" => "text/html",
            "css" => "text/css",
            "sh" | "bash" => "text/x-shellscript",
            "xml" => "text/xml",
            "csv" => "text/csv",
            _ => "text/plain",
        };
        Some(mime.to_string())
    }
}

impl ResourceHandler for FileResourceHandler {
    fn list_resources(&self) -> Vec<McpResource> {
        let entries = list_files_recursive(&self.root, 500);
        let mut resources = Vec::with_capacity(entries.len());
        for path in entries {
            let uri = Self::path_to_uri(&path);
            let name = path
                .strip_prefix(&self.root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let mime_type = Self::guess_mime(&path);
            resources.push(McpResource {
                uri,
                name,
                description: None,
                mime_type,
            });
        }
        resources
    }

    fn read_resource(
        &self,
        uri: &str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ResourceReadResult, String>> + Send + '_>,
    > {
        let uri = uri.to_string();
        Box::pin(async move {
            let path = self.uri_to_path(&uri)?;

            let content = tokio::fs::read_to_string(&path)
                .await
                .map_err(|e| format!("failed to read {uri}: {e}"))?;

            let mime_type = Self::guess_mime(&path);

            Ok(ResourceReadResult {
                contents: vec![ResourceContent {
                    uri,
                    mime_type,
                    text: Some(content),
                }],
            })
        })
    }
}

/// Recursively list regular files under `dir`, up to `max_files`.
pub(crate) fn list_files_recursive(
    dir: &std::path::Path,
    max_files: usize,
) -> Vec<std::path::PathBuf> {
    let mut result = Vec::new();
    let mut stack = vec![dir.to_path_buf()];

    while let Some(current) = stack.pop() {
        if result.len() >= max_files {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries {
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|name| {
                    name.starts_with('.') || name == "target" || name == "node_modules"
                })
            {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() {
                result.push(path);
                if result.len() >= max_files {
                    break;
                }
            }
        }
    }

    result.sort();
    result
}
