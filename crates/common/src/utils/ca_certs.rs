//! Custom CA certificate loading for enterprise networks.
//!
//! Many corporate environments terminate TLS at a proxy and re-sign
//! downstream traffic with an internal CA. Without trusting that CA,
//! every outbound HTTPS call from crab (LLM APIs, MCP servers, OAuth
//! endpoints) fails with a certificate error.
//!
//! This module reads PEM-encoded certificates from:
//!
//! 1. `CRAB_CA_BUNDLE` env var — path to a single PEM file with one
//!    or more concatenated certificates.
//! 2. `SSL_CERT_FILE` env var — same convention as curl/openssl.
//! 3. `SSL_CERT_DIR` env var — directory; every `.pem` / `.crt` file
//!    inside is loaded.
//! 4. Explicit paths passed to [`load_ca_bundle`].
//!
//! The returned `CaBundle` is `CaBundle::pem_blocks`-ready: each element
//! is a single PEM-encoded certificate block (BEGIN/END CERTIFICATE).
//! Callers wire these into `reqwest::ClientBuilder::add_root_certificate`
//! or equivalent.

use std::fs;
use std::path::{Path, PathBuf};

/// Env var to point at a custom CA bundle file (crab-specific).
pub const CRAB_CA_BUNDLE_ENV: &str = "CRAB_CA_BUNDLE";
/// OpenSSL / curl convention for single-file CA bundle.
pub const SSL_CERT_FILE_ENV: &str = "SSL_CERT_FILE";
/// OpenSSL / curl convention for directory of CA certificates.
pub const SSL_CERT_DIR_ENV: &str = "SSL_CERT_DIR";

/// One or more PEM-encoded CA certificates collected from env vars +
/// caller-supplied paths.
#[derive(Debug, Clone, Default)]
pub struct CaBundle {
    /// Individual PEM blocks (each containing exactly one certificate).
    pub pem_blocks: Vec<Vec<u8>>,
    /// Paths the bundle was loaded from, for diagnostics.
    pub sources: Vec<PathBuf>,
}

impl CaBundle {
    /// Is the bundle empty?
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pem_blocks.is_empty()
    }

    /// Number of certificates in the bundle.
    #[must_use]
    pub fn len(&self) -> usize {
        self.pem_blocks.len()
    }

    /// Concatenate every block into a single PEM blob suitable for
    /// `reqwest::Certificate::from_pem_bundle` etc.
    #[must_use]
    pub fn to_combined_pem(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.pem_blocks.iter().map(Vec::len).sum());
        for block in &self.pem_blocks {
            out.extend_from_slice(block);
            if !block.ends_with(b"\n") {
                out.push(b'\n');
            }
        }
        out
    }
}

/// Load the full CA bundle from env vars plus any extra paths.
///
/// Walks (in order) `CRAB_CA_BUNDLE`, `SSL_CERT_FILE`, `SSL_CERT_DIR`,
/// then each path in `extra_paths`. Missing env vars or non-existent
/// files are skipped silently (so setting one env var doesn't require
/// the others). Read errors are logged via `tracing::warn` and skipped
/// so one bad source doesn't block the whole load.
///
/// # Errors
///
/// Returns `Err` only on invalid PEM data in a file that was otherwise
/// readable. Missing files / missing env vars are not errors.
pub fn load_ca_bundle(extra_paths: &[PathBuf]) -> crate::Result<CaBundle> {
    let env = EnvSources {
        crab_bundle: std::env::var(CRAB_CA_BUNDLE_ENV).ok(),
        ssl_cert_file: std::env::var(SSL_CERT_FILE_ENV).ok(),
        ssl_cert_dir: std::env::var(SSL_CERT_DIR_ENV).ok(),
    };
    load_ca_bundle_with_env(&env, extra_paths)
}

/// Env-var inputs to the CA loader.
///
/// Split out from [`load_ca_bundle`] so tests can inject a hermetic
/// environment instead of reading the process's real env (which on CI
/// runners includes system-wide cert bundles that pollute assertions).
#[derive(Debug, Default, Clone)]
struct EnvSources {
    crab_bundle: Option<String>,
    ssl_cert_file: Option<String>,
    ssl_cert_dir: Option<String>,
}

fn load_ca_bundle_with_env(env: &EnvSources, extra_paths: &[PathBuf]) -> crate::Result<CaBundle> {
    let mut bundle = CaBundle::default();

    // 1. CRAB_CA_BUNDLE (single file, takes priority)
    if let Some(path) = env.crab_bundle.as_deref() {
        try_load_file(&mut bundle, Path::new(path));
    }
    // 2. SSL_CERT_FILE
    if let Some(path) = env.ssl_cert_file.as_deref() {
        try_load_file(&mut bundle, Path::new(path));
    }
    // 3. SSL_CERT_DIR
    if let Some(dir) = env.ssl_cert_dir.as_deref() {
        try_load_dir(&mut bundle, Path::new(dir));
    }
    // 4. Caller extras
    for path in extra_paths {
        if path.is_dir() {
            try_load_dir(&mut bundle, path);
        } else {
            try_load_file(&mut bundle, path);
        }
    }

    Ok(bundle)
}

/// Load a single PEM file into the bundle.
///
/// Non-existent paths are silently skipped (standard convention for
/// env-var-driven paths). Read errors produce a `tracing::warn`.
fn try_load_file(bundle: &mut CaBundle, path: &Path) {
    if !path.exists() {
        return;
    }
    match fs::read(path) {
        Ok(bytes) => match split_pem_blocks(&bytes) {
            blocks if !blocks.is_empty() => {
                tracing::debug!(
                    path = %path.display(),
                    count = blocks.len(),
                    "loaded CA certs"
                );
                bundle.pem_blocks.extend(blocks);
                bundle.sources.push(path.to_path_buf());
            }
            _ => {
                tracing::warn!(
                    path = %path.display(),
                    "no PEM CERTIFICATE blocks found"
                );
            }
        },
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "cannot read CA file"
            );
        }
    }
}

/// Load every `.pem` / `.crt` file from a directory.
fn try_load_dir(bundle: &mut CaBundle, dir: &Path) {
    if !dir.is_dir() {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        tracing::warn!(dir = %dir.display(), "cannot list CA dir");
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
            continue;
        };
        if ext.eq_ignore_ascii_case("pem") || ext.eq_ignore_ascii_case("crt") {
            try_load_file(bundle, &path);
        }
    }
}

/// Split a PEM blob into individual `-----BEGIN CERTIFICATE-----` blocks.
///
/// Each returned `Vec<u8>` contains exactly one complete block including
/// both BEGIN and END lines + trailing newline. Lines outside any block
/// (free-text commentary, blank lines) are discarded.
pub fn split_pem_blocks(bytes: &[u8]) -> Vec<Vec<u8>> {
    const BEGIN: &[u8] = b"-----BEGIN CERTIFICATE-----";
    const END: &[u8] = b"-----END CERTIFICATE-----";

    let Ok(text) = std::str::from_utf8(bytes) else {
        return Vec::new();
    };

    let mut blocks = Vec::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        let trimmed = line.trim();
        let begin = trimmed.as_bytes() == BEGIN;
        let end = trimmed.as_bytes() == END;

        if begin {
            current = Some(String::new());
        }
        if let Some(buf) = current.as_mut() {
            buf.push_str(line);
            buf.push('\n');
        }
        if end && let Some(buf) = current.take() {
            blocks.push(buf.into_bytes());
        }
    }
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const CERT_A: &str = "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----";
    const CERT_B: &str = "-----BEGIN CERTIFICATE-----\nBBBB\n-----END CERTIFICATE-----";

    #[test]
    fn bundle_starts_empty() {
        let b = CaBundle::default();
        assert!(b.is_empty());
        assert_eq!(b.len(), 0);
    }

    #[test]
    fn split_pem_single_block() {
        let blocks = split_pem_blocks(CERT_A.as_bytes());
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].starts_with(b"-----BEGIN CERTIFICATE-----"));
        assert!(blocks[0].ends_with(b"-----END CERTIFICATE-----\n"));
    }

    #[test]
    fn split_pem_multiple_blocks() {
        let combined = format!("{CERT_A}\n{CERT_B}");
        let blocks = split_pem_blocks(combined.as_bytes());
        assert_eq!(blocks.len(), 2);
        assert!(blocks[0].windows(4).any(|w| w == b"AAAA"));
        assert!(blocks[1].windows(4).any(|w| w == b"BBBB"));
    }

    #[test]
    fn split_pem_ignores_commentary() {
        let input = format!("# Issued by Acme\nfree text\n{CERT_A}\n\n# trailing\n");
        let blocks = split_pem_blocks(input.as_bytes());
        assert_eq!(blocks.len(), 1);
    }

    #[test]
    fn split_pem_empty_returns_empty_vec() {
        assert!(split_pem_blocks(b"").is_empty());
        assert!(split_pem_blocks(b"no cert here").is_empty());
    }

    #[test]
    fn combined_pem_joins_blocks_with_newline_separator() {
        let bundle = CaBundle {
            pem_blocks: vec![b"A-----END CERTIFICATE-----".to_vec(), b"B\n".to_vec()],
            sources: vec![],
        };
        let combined = bundle.to_combined_pem();
        // First block lacks trailing newline → one added; second already has.
        assert!(
            combined
                .windows(b"CERTIFICATE-----\n".len())
                .any(|w| w == b"CERTIFICATE-----\n")
        );
    }

    /// Hermetic load: no env vars, only explicit paths. Used by tests to
    /// avoid picking up the CI runner's system cert bundle (e.g. Ubuntu
    /// sets `SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt`, which
    /// would add hundreds of certs and break `assert_eq!(b.len(), N)`).
    fn load_hermetic(extra_paths: &[PathBuf]) -> CaBundle {
        load_ca_bundle_with_env(&EnvSources::default(), extra_paths).unwrap()
    }

    #[test]
    fn load_missing_env_returns_empty_bundle() {
        let b = load_hermetic(&[]);
        assert!(b.is_empty());
        assert_eq!(b.len(), 0);
    }

    #[test]
    fn load_from_explicit_path() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("custom.pem");
        let mut f = fs::File::create(&path).unwrap();
        f.write_all(CERT_A.as_bytes()).unwrap();

        let b = load_hermetic(std::slice::from_ref(&path));
        assert_eq!(b.len(), 1);
        assert_eq!(b.sources, vec![path]);
    }

    #[test]
    fn load_from_directory_picks_up_pem_and_crt() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("a.pem"), CERT_A).unwrap();
        fs::write(tmp.path().join("b.crt"), CERT_B).unwrap();
        fs::write(tmp.path().join("readme.txt"), "ignored").unwrap();

        let b = load_hermetic(std::slice::from_ref(&tmp.path().to_path_buf()));
        assert_eq!(b.len(), 2);
        assert_eq!(b.sources.len(), 2);
    }

    #[test]
    fn load_extra_path_nonexistent_is_silent() {
        let b = load_hermetic(&[PathBuf::from("/definitely/does/not/exist/bundle.pem")]);
        assert!(b.is_empty());
    }

    #[test]
    fn load_extra_file_with_no_cert_blocks_warns_but_succeeds() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("empty.pem");
        fs::write(&path, "just some text, no BEGIN CERTIFICATE").unwrap();

        let b = load_hermetic(std::slice::from_ref(&path));
        // The file was readable but had no valid blocks; bundle stays empty.
        assert_eq!(b.len(), 0);
    }
}
