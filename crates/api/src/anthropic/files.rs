//! Anthropic Files API — upload images / PDFs to the `/v1/files` endpoint
//! and reference them from messages by `file_id`.
//!
//! Two-phase attachment workflow:
//!
//! 1. User supplies a local path. [`AnthropicFilesClient::upload`] POSTs
//!    the bytes as `multipart/form-data`, receives back a response with
//!    `id` / `filename` / `size_bytes` / `mime_type` / `created_at`.
//! 2. Caller references the returned `id` in subsequent message content
//!    blocks (`ContentBlockParam::Document { source: Source::File { file_id } }`
//!    / `Source::Image { file_id }`).
//!
//! Mirrors CCB `services/api/filesApi.ts`. Sharing the same
//! `reqwest::Client` + auth chain as the Messages API would be nice in a
//! future refactor; for now this client builds its own so the module is
//! self-contained and usable from workflow helpers without needing to
//! hold on to an `AnthropicClient`.

use std::path::Path;

use crate::error::{ApiError, Result};
use serde::{Deserialize, Serialize};

/// Anthropic `files-api-2025-04-14` beta header value. Kept as a module
/// constant so upgrades are one edit.
const FILES_BETA: &str = "files-api-2025-04-14";

/// Anthropic API version header value (same as messages client).
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Response body of `POST /v1/files`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileUploadResponse {
    /// Opaque file identifier to use in message content blocks.
    pub id: String,
    /// Original file name (server may sanitise).
    pub filename: String,
    /// Size in bytes of the uploaded file.
    pub size_bytes: u64,
    /// Detected / provided MIME type.
    pub mime_type: String,
    /// Unix epoch (seconds) when the server accepted the upload.
    pub created_at: i64,
    /// Type discriminator set by the server (e.g. `"file"`).
    #[serde(default)]
    #[serde(rename = "type")]
    pub type_field: Option<String>,
}

/// Client for the `/v1/files` endpoint.
///
/// Build once and reuse — holds a pooled `reqwest::Client`.
pub struct AnthropicFilesClient {
    http: reqwest::Client,
    base_url: String,
    auth: Box<dyn crab_auth::AuthProvider>,
}

impl AnthropicFilesClient {
    /// Create a new client. `base_url` should match the one used by
    /// `AnthropicClient` (typically `https://api.anthropic.com`).
    pub fn new(base_url: impl Into<String>, auth: Box<dyn crab_auth::AuthProvider>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .pool_max_idle_per_host(4)
            .build()
            .expect("failed to build HTTP client");

        Self {
            http,
            base_url: base_url.into(),
            auth,
        }
    }

    /// Upload a local file, returning its `FileUploadResponse`.
    ///
    /// MIME type is inferred from the file extension via `mime_guess`;
    /// falls back to `application/octet-stream` when unknown.
    ///
    /// # Errors
    ///
    /// Returns `Err` on read failure, auth failure, non-2xx HTTP status,
    /// or malformed response.
    pub async fn upload(&self, path: &Path) -> Result<FileUploadResponse> {
        let bytes = tokio::fs::read(path).await.map_err(|e| {
            ApiError::Common(crab_core::Error::Other(format!(
                "files/upload: read {} failed: {e}",
                path.display()
            )))
        })?;
        let filename = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("upload.bin")
            .to_owned();
        let mime = detect_mime(path);
        self.upload_bytes(&filename, &mime, bytes).await
    }

    /// Upload an in-memory byte slice with an explicit filename + MIME
    /// type. Primarily for callers that have already decoded / generated
    /// the bytes and don't want a round-trip through the filesystem.
    ///
    /// # Errors
    ///
    /// Same as [`Self::upload`].
    pub async fn upload_bytes(
        &self,
        filename: &str,
        mime: &str,
        bytes: Vec<u8>,
    ) -> Result<FileUploadResponse> {
        let auth = self.auth.get_auth().await.map_err(ApiError::Common)?;
        let url = format!("{}/v1/files", self.base_url);

        let part = reqwest::multipart::Part::bytes(bytes)
            .file_name(filename.to_string())
            .mime_str(mime)
            .map_err(|e| {
                ApiError::Common(crab_core::Error::Other(format!(
                    "files/upload: invalid mime `{mime}`: {e}"
                )))
            })?;
        let form = reqwest::multipart::Form::new().part("file", part);

        let mut builder = self
            .http
            .post(&url)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("anthropic-beta", FILES_BETA)
            .multipart(form);

        match auth {
            crab_auth::AuthMethod::ApiKey(key) => {
                builder = builder.header("x-api-key", key);
            }
            crab_auth::AuthMethod::OAuth(token) => {
                builder = builder.header("authorization", format!("Bearer {}", token.access_token));
            }
        }

        let resp = builder.send().await.map_err(|e| {
            ApiError::Common(crab_core::Error::Other(format!(
                "files/upload: POST failed: {e}"
            )))
        })?;

        let status = resp.status();
        let body = resp.text().await.map_err(|e| {
            ApiError::Common(crab_core::Error::Other(format!(
                "files/upload: read body failed: {e}"
            )))
        })?;

        if !status.is_success() {
            return Err(ApiError::Common(crab_core::Error::Other(format!(
                "files/upload: HTTP {status}: {body}"
            ))));
        }

        serde_json::from_str::<FileUploadResponse>(&body).map_err(|e| {
            ApiError::Common(crab_core::Error::Other(format!(
                "files/upload: parse response failed: {e}; body: {body}"
            )))
        })
    }

    /// Delete an uploaded file by its `id`. Useful for cleanup in workflows
    /// that attach transient files (screenshots etc.) they don't intend
    /// to keep server-side.
    ///
    /// # Errors
    ///
    /// Returns `Err` on auth failure or non-2xx HTTP status.
    pub async fn delete(&self, file_id: &str) -> Result<()> {
        let auth = self.auth.get_auth().await.map_err(ApiError::Common)?;
        let url = format!("{}/v1/files/{file_id}", self.base_url);

        let mut builder = self
            .http
            .delete(&url)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("anthropic-beta", FILES_BETA);

        match auth {
            crab_auth::AuthMethod::ApiKey(key) => {
                builder = builder.header("x-api-key", key);
            }
            crab_auth::AuthMethod::OAuth(token) => {
                builder = builder.header("authorization", format!("Bearer {}", token.access_token));
            }
        }

        let resp = builder.send().await.map_err(|e| {
            ApiError::Common(crab_core::Error::Other(format!(
                "files/delete: DELETE failed: {e}"
            )))
        })?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(ApiError::Common(crab_core::Error::Other(format!(
                "files/delete: HTTP {status}: {body}"
            ))));
        }
        Ok(())
    }
}

/// Infer MIME type from path extension. Falls back to
/// `application/octet-stream` when the extension is unknown or absent.
#[must_use]
pub fn detect_mime(path: &Path) -> String {
    mime_guess::from_path(path)
        .first_or_octet_stream()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_mime_png() {
        assert_eq!(detect_mime(Path::new("screenshot.png")), "image/png");
    }

    #[test]
    fn detect_mime_pdf() {
        assert_eq!(detect_mime(Path::new("report.pdf")), "application/pdf");
    }

    #[test]
    fn detect_mime_unknown_falls_back() {
        assert_eq!(
            detect_mime(Path::new("blob.xyzzy")),
            "application/octet-stream"
        );
    }

    #[test]
    fn detect_mime_no_extension_falls_back() {
        assert_eq!(detect_mime(Path::new("README")), "application/octet-stream");
    }

    #[test]
    fn upload_response_serde_roundtrip() {
        let r = FileUploadResponse {
            id: "file_abc123".into(),
            filename: "photo.png".into(),
            size_bytes: 204_800,
            mime_type: "image/png".into(),
            created_at: 1_700_000_000,
            type_field: Some("file".into()),
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: FileUploadResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn upload_response_minimal_parse() {
        // Server shape with no `type` field — must still parse.
        let json = r#"{
            "id":"file_x",
            "filename":"a.pdf",
            "size_bytes":1024,
            "mime_type":"application/pdf",
            "created_at":1700000000
        }"#;
        let r: FileUploadResponse = serde_json::from_str(json).unwrap();
        assert_eq!(r.id, "file_x");
        assert!(r.type_field.is_none());
    }

    // Note: network-dependent tests (actual multipart POST) would need a
    // mock HTTP server (wiremock). Deferred to the Phase 7 integration
    // test batch so unit tests stay offline + deterministic.
}
