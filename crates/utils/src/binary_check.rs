//! Heuristic "is this a binary file" detector.
//!
//! Two complementary strategies:
//!
//! 1. **Magic-number sniff** — [`infer`] looks at the first ~16 bytes
//!    and matches known file signatures (PNG, ELF, zip, PDF, etc.).
//!    High precision for recognised formats.
//!
//! 2. **Byte-pattern sniff** — scan up to 8 KiB for NUL bytes or a high
//!    proportion of non-printable characters. Catches unrecognised
//!    binary formats (proprietary, compressed, encrypted) that `infer`
//!    doesn't know about.
//!
//! Fast path: [`is_binary_bytes`] takes a buffer and makes the decision
//! without touching the filesystem. Callers that have the path handy use
//! [`is_binary_path`] which reads just enough bytes.

use std::fs::File;
use std::io::Read;
use std::path::Path;

/// Number of bytes [`is_binary_path`] reads from the head of the file.
pub const SNIFF_BYTES: usize = 8192;

/// Fraction of non-printable bytes above which we declare a blob binary
/// (ignoring common whitespace). Tuned to classify gzipped JSON as binary
/// but keep colourised terminal transcripts (lots of ESC codes) on the
/// text side.
const NON_PRINTABLE_RATIO: f32 = 0.30;

/// Decide whether `bytes` looks like a binary file.
///
/// Rules (first-match wins):
/// 1. Contains a NUL byte → binary.
/// 2. `infer` recognises a known binary format → binary.
/// 3. More than [`NON_PRINTABLE_RATIO`] of bytes are non-printable
///    (excluding tab/newline/carriage-return/ESC) → binary.
/// 4. Otherwise → text.
///
/// An empty buffer is classified as text (nothing suggests binary).
#[must_use]
pub fn is_binary_bytes(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    if bytes.contains(&0) {
        return true;
    }

    if let Some(kind) = infer::get(bytes)
        && is_binary_mime(kind.mime_type())
    {
        return true;
    }

    let non_printable = bytes.iter().filter(|&&b| !is_printable(b)).count();
    let ratio = non_printable as f32 / bytes.len() as f32;
    ratio >= NON_PRINTABLE_RATIO
}

/// Read the head of a file and call [`is_binary_bytes`].
///
/// Reads at most [`SNIFF_BYTES`] bytes. Returns `Ok(false)` for
/// zero-length files (consistent with [`is_binary_bytes`]).
///
/// # Errors
///
/// Returns `Err` on file-open or read failure.
pub fn is_binary_path(path: &Path) -> std::io::Result<bool> {
    let mut f = File::open(path)?;
    let mut buf = vec![0u8; SNIFF_BYTES];
    let n = f.read(&mut buf)?;
    buf.truncate(n);
    Ok(is_binary_bytes(&buf))
}

/// Is this MIME type binary? Broadly: not `text/*`, not `application/json`,
/// not `application/xml`, not JavaScript sources.
fn is_binary_mime(mime: &str) -> bool {
    if mime.starts_with("text/") {
        return false;
    }
    // A few `application/*` types that are actually text.
    matches!(
        mime,
        "application/json"
            | "application/xml"
            | "application/javascript"
            | "application/typescript"
            | "application/x-sh"
            | "application/x-yaml"
            | "application/toml"
    )
    .not_into_binary()
}

/// Treat a byte as printable if it's ASCII graphic/space OR a common
/// whitespace / ESC-sequence byte. Terminal colour codes embed `\x1b`
/// (ESC) a lot in otherwise-readable logs, so we allow it.
fn is_printable(b: u8) -> bool {
    matches!(
        b,
        // graphic + space
        0x20..=0x7e
        // common whitespace
        | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c
        // ESC (colour codes)
        | 0x1b
    ) || b >= 0x80 // UTF-8 continuation / multibyte — valid in text
}

/// Small helper so `is_binary_mime` can short-circuit `text/*` on top of a
/// `matches!` result without awkward parens. Purely for readability.
trait NotIntoBinary {
    fn not_into_binary(self) -> bool;
}
impl NotIntoBinary for bool {
    fn not_into_binary(self) -> bool {
        !self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn empty_is_text() {
        assert!(!is_binary_bytes(&[]));
    }

    #[test]
    fn null_byte_is_binary() {
        assert!(is_binary_bytes(b"plain text\0more"));
    }

    #[test]
    fn ascii_text_is_text() {
        let lorem = "Lorem ipsum dolor sit amet, consectetur adipiscing elit. ";
        let body = lorem.repeat(100);
        assert!(!is_binary_bytes(body.as_bytes()));
    }

    #[test]
    fn utf8_with_multibyte_is_text() {
        let s = "你好，世界。🦀 crab rules everything around me.";
        assert!(!is_binary_bytes(s.as_bytes()));
    }

    #[test]
    fn ansi_colourised_log_is_text() {
        let colour = "\x1b[32mOK\x1b[0m pass: 42\n\x1b[31mFAIL\x1b[0m check: 3\n".repeat(50);
        assert!(!is_binary_bytes(colour.as_bytes()));
    }

    #[test]
    fn png_magic_is_binary() {
        // PNG signature
        let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        bytes.extend_from_slice(&[0u8; 32]);
        // Contains NUL, which triggers the first rule too — confirm binary.
        assert!(is_binary_bytes(&bytes));
    }

    #[test]
    fn gzip_header_is_binary() {
        // gzip magic: 1F 8B 08
        let bytes = [0x1f, 0x8b, 0x08, 0x00, 0x01, 0x02, 0x03];
        assert!(is_binary_bytes(&bytes));
    }

    #[test]
    fn mostly_non_printable_is_binary() {
        // 80% non-printable control bytes (but no NUL, no known magic).
        let mut bytes = vec![0x01u8; 80];
        bytes.extend_from_slice(b"ABCDEFGHIJKLMNOPQRST"); // 20 printable
        assert_eq!(bytes.len(), 100);
        assert!(is_binary_bytes(&bytes));
    }

    #[test]
    fn is_binary_path_reads_and_decides() {
        let tmp = tempfile::tempdir().unwrap();
        let text_path = tmp.path().join("a.txt");
        let bin_path = tmp.path().join("b.bin");
        std::fs::write(&text_path, "Hello, world!\n").unwrap();
        let mut f = std::fs::File::create(&bin_path).unwrap();
        f.write_all(&[0u8; 64]).unwrap();

        assert!(!is_binary_path(&text_path).unwrap());
        assert!(is_binary_path(&bin_path).unwrap());
    }

    #[test]
    fn is_binary_path_missing_file_errors() {
        assert!(is_binary_path(Path::new("/definitely/nope/nada.bin")).is_err());
    }

    #[test]
    fn json_mime_stays_text() {
        assert!(!is_binary_mime("application/json"));
        assert!(!is_binary_mime("text/plain"));
        assert!(is_binary_mime("image/png"));
        assert!(is_binary_mime("application/zip"));
        assert!(is_binary_mime("application/octet-stream"));
    }

    #[test]
    fn sniff_bytes_cap_respected() {
        // File larger than SNIFF_BYTES but all text → should still be text.
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("big.txt");
        std::fs::write(&p, "x".repeat(SNIFF_BYTES * 3)).unwrap();
        assert!(!is_binary_path(&p).unwrap());
    }
}
