//! AWS Signature Version 4 request signing.
//!
//! Implements the canonical request / string-to-sign / signing-key derivation
//! described in the AWS `SigV4` specification. Used by Bedrock Runtime signing
//! and STS `AssumeRole` calls. This is the single source of `SigV4` logic in
//! the crate; no other module re-implements the signing chain.

use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

/// AWS credentials used to sign a request.
#[derive(Debug, Clone)]
pub struct SigningCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
}

/// A request to be signed.
///
/// `headers` must include every header that should be covered by the
/// signature except `host` and `x-amz-date`, which are derived here from
/// `host` and `timestamp`. Header names are matched case-insensitively and
/// canonicalized to lowercase per the `SigV4` spec.
pub struct CanonicalRequest<'a> {
    pub method: &'a str,
    /// Host header value, e.g. `bedrock-runtime.us-east-1.amazonaws.com`.
    pub host: &'a str,
    /// URI path, e.g. `/model/.../invoke`. Must be non-empty (`/` for root).
    pub path: &'a str,
    /// Raw query string without leading `?` (already percent-encoded by caller).
    pub query: &'a str,
    /// Extra headers to include in the signature (name, value).
    pub headers: &'a [(&'a str, &'a str)],
    /// Request body bytes (empty slice for no body).
    pub payload: &'a [u8],
}

/// Output of signing: the `Authorization` header plus the headers that the
/// signature commits to and therefore must be sent verbatim on the request.
#[derive(Debug, Clone)]
pub struct SignedRequest {
    pub authorization: String,
    pub amz_date: String,
    pub host: String,
    pub session_token: Option<String>,
}

impl SignedRequest {
    /// The signed headers as `(name, value)` pairs to attach to the request.
    ///
    /// `host` is omitted: HTTP clients set it from the request URL, and it is
    /// already covered by the signature.
    #[must_use]
    pub fn header_pairs(&self) -> Vec<(String, String)> {
        let mut pairs = Vec::with_capacity(3);
        pairs.push(("authorization".to_string(), self.authorization.clone()));
        pairs.push(("x-amz-date".to_string(), self.amz_date.clone()));
        if let Some(token) = &self.session_token {
            pairs.push(("x-amz-security-token".to_string(), token.clone()));
        }
        pairs
    }
}

/// Sign a request with AWS `SigV4`.
///
/// `timestamp_secs` is seconds since the Unix epoch; it is formatted as the
/// `x-amz-date` value and used to build the credential scope.
#[must_use]
pub fn sign(
    req: &CanonicalRequest<'_>,
    creds: &SigningCredentials,
    region: &str,
    service: &str,
    timestamp_secs: u64,
) -> SignedRequest {
    let amz_date = format_amz_date(timestamp_secs);
    let date_stamp = &amz_date[..8];

    let payload_hash = sha256_hex(req.payload);

    let mut signed: Vec<(String, String)> = Vec::with_capacity(req.headers.len() + 3);
    signed.push(("host".to_string(), req.host.to_string()));
    signed.push(("x-amz-date".to_string(), amz_date.clone()));
    if let Some(token) = &creds.session_token {
        signed.push(("x-amz-security-token".to_string(), token.clone()));
    }
    for (name, value) in req.headers {
        signed.push((name.to_lowercase(), (*value).to_string()));
    }
    signed.sort_by(|a, b| a.0.cmp(&b.0));

    let signed_headers = signed
        .iter()
        .map(|(n, _)| n.as_str())
        .collect::<Vec<_>>()
        .join(";");

    let mut canonical_headers = String::new();
    for (name, value) in &signed {
        canonical_headers.push_str(name);
        canonical_headers.push(':');
        canonical_headers.push_str(value.trim());
        canonical_headers.push('\n');
    }

    let canonical_request = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        req.method, req.path, req.query, canonical_headers, signed_headers, payload_hash,
    );

    let credential_scope = format!("{date_stamp}/{region}/{service}/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{credential_scope}\n{}",
        sha256_hex(canonical_request.as_bytes()),
    );

    let signing_key = derive_signing_key(&creds.secret_access_key, date_stamp, region, service);
    let signature = hex_encode(&hmac_sha256(&signing_key, string_to_sign.as_bytes()));

    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/{credential_scope}, SignedHeaders={signed_headers}, Signature={signature}",
        creds.access_key_id,
    );

    SignedRequest {
        authorization,
        amz_date,
        host: req.host.to_string(),
        session_token: creds.session_token.clone(),
    }
}

/// Derive the `SigV4` signing key via the HMAC chain
/// `kDate -> kRegion -> kService -> kSigning`.
#[must_use]
pub fn derive_signing_key(secret: &str, date_stamp: &str, region: &str, service: &str) -> Vec<u8> {
    let k_date = hmac_sha256(format!("AWS4{secret}").as_bytes(), date_stamp.as_bytes());
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, service.as_bytes());
    hmac_sha256(&k_service, b"aws4_request")
}

/// HMAC-SHA256 of `data` under `key`.
#[must_use]
pub fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts keys of any length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// Lowercase hex-encoded SHA-256 digest of `data`.
#[must_use]
pub fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex_encode(&hasher.finalize())
}

/// Lowercase hex-encode a byte slice.
#[must_use]
pub fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Format a Unix timestamp as `YYYYMMDD'T'HHMMSS'Z'`.
#[must_use]
pub fn format_amz_date(timestamp_secs: u64) -> String {
    let days = timestamp_secs / 86_400;
    let (year, month, day) = days_to_ymd(days);
    let rem = timestamp_secs % 86_400;
    let (hour, minute, second) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    format!("{year:04}{month:02}{day:02}T{hour:02}{minute:02}{second:02}Z")
}

/// Convert days since the Unix epoch to `(year, month, day)` using Howard
/// Hinnant's `civil_from_days` algorithm.
fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_hex_empty() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn hmac_sha256_rfc4231_case2() {
        let key = b"Jefe";
        let data = b"what do ya want for nothing?";
        assert_eq!(
            hex_encode(&hmac_sha256(key, data)),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn format_amz_date_known() {
        assert_eq!(format_amz_date(1_705_320_000), "20240115T120000Z");
    }

    // Official AWS SigV4 test-suite `get-vanilla` vector.
    // Documented credentials and expected signature from the AWS signature
    // examples (sigv4-examples / aws-sig-v4-test-suite).
    #[test]
    fn sigv4_get_vanilla_vector() {
        let creds = SigningCredentials {
            access_key_id: "AKIDEXAMPLE".into(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".into(),
            session_token: None,
        };
        // 2015-08-30T12:36:00Z = 1440938160
        let req = CanonicalRequest {
            method: "GET",
            host: "example.amazonaws.com",
            path: "/",
            query: "",
            headers: &[],
            payload: b"",
        };
        let signed = sign(&req, &creds, "us-east-1", "service", 1_440_938_160);
        assert_eq!(signed.amz_date, "20150830T123600Z");
        // Expected Authorization from the AWS test suite get-vanilla case.
        assert_eq!(
            signed.authorization,
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, \
             SignedHeaders=host;x-amz-date, \
             Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
        );
    }

    #[test]
    fn derive_signing_key_matches_aws_example() {
        // AWS documented intermediate: signing key for
        // 20150830 / us-east-1 / iam derived from the example secret.
        let key = derive_signing_key(
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            "20150830",
            "us-east-1",
            "iam",
        );
        assert_eq!(
            hex_encode(&key),
            "c4afb1cc5771d871763a393e44b703571b55cc28424d1a5e86da6ed3c154a4b9"
        );
    }

    #[test]
    fn sign_includes_session_token_when_present() {
        let creds = SigningCredentials {
            access_key_id: "AKIDEXAMPLE".into(),
            secret_access_key: "secret".into(),
            session_token: Some("session-token-value".into()),
        };
        let req = CanonicalRequest {
            method: "POST",
            host: "bedrock-runtime.us-east-1.amazonaws.com",
            path: "/model/test/invoke",
            query: "",
            headers: &[("content-type", "application/json")],
            payload: b"{}",
        };
        let signed = sign(&req, &creds, "us-east-1", "bedrock", 1_440_938_160);
        assert_eq!(signed.session_token.as_deref(), Some("session-token-value"));
        assert!(signed.authorization.contains("x-amz-security-token"));
        assert!(signed.authorization.contains("content-type"));
    }
}
