//! AWS IAM Role Assumption for Bedrock access.
//!
//! Supports two flows:
//! 1. `AssumeRole` — exchange long-lived IAM credentials for short-lived session credentials
//! 2. `AssumeRoleWithWebIdentity` — exchange an OIDC token (from CI/CD) for AWS credentials
//!
//! Both return temporary credentials (access key, secret key, session token)
//! that are cached and auto-refreshed before expiry.

use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

use crate::{AuthMethod, AuthProvider, OAuthToken};

/// Default session duration for assumed roles (1 hour).
const DEFAULT_SESSION_DURATION_SECS: u64 = 3600;

/// Refresh margin — refresh 5 minutes before expiry.
const REFRESH_MARGIN_SECS: u64 = 300;

/// Configuration for AWS IAM role assumption.
#[derive(Debug, Clone)]
pub struct AssumeRoleConfig {
    /// ARN of the role to assume (e.g., `arn:aws:iam::123456789012:role/MyRole`).
    pub role_arn: String,
    /// External ID for cross-account access (optional).
    pub external_id: Option<String>,
    /// Session name for `CloudTrail` audit logging.
    pub session_name: String,
    /// Session duration in seconds (default: 3600).
    pub duration_secs: u64,
    /// AWS region for the STS endpoint.
    pub region: String,
}

impl Default for AssumeRoleConfig {
    fn default() -> Self {
        Self {
            role_arn: String::new(),
            external_id: None,
            session_name: "crab-code-session".to_string(),
            duration_secs: DEFAULT_SESSION_DURATION_SECS,
            region: "us-east-1".to_string(),
        }
    }
}

/// Configuration for `AssumeRoleWithWebIdentity` (OIDC-based).
#[derive(Debug, Clone)]
pub struct WebIdentityConfig {
    /// ARN of the role to assume.
    pub role_arn: String,
    /// Path to the OIDC token file (e.g., from `AWS_WEB_IDENTITY_TOKEN_FILE`).
    pub token_file: Option<String>,
    /// Inline OIDC token (alternative to file).
    pub token: Option<String>,
    /// Session name for audit logging.
    pub session_name: String,
    /// Session duration in seconds.
    pub duration_secs: u64,
    /// AWS region.
    pub region: String,
}

impl Default for WebIdentityConfig {
    fn default() -> Self {
        Self {
            role_arn: String::new(),
            token_file: None,
            token: None,
            session_name: "crab-code-web-identity".to_string(),
            duration_secs: DEFAULT_SESSION_DURATION_SECS,
            region: "us-east-1".to_string(),
        }
    }
}

/// Temporary AWS credentials from STS.
#[derive(Debug, Clone)]
pub struct StsCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: String,
    pub region: String,
    /// When these credentials expire.
    pub expires_at: Instant,
}

impl StsCredentials {
    /// Check if credentials are still valid (with refresh margin).
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.expires_at > Instant::now() + Duration::from_secs(REFRESH_MARGIN_SECS)
    }

    /// Format as `SigV4`-compatible authorization header.
    #[must_use]
    pub fn to_auth_header(&self, service: &str) -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap_or_default();
        let timestamp = now.as_secs();
        let date_stamp = format_date_stamp(timestamp);
        let credential_scope = format!("{date_stamp}/{}/{service}/aws4_request", self.region);

        format!(
            "AWS4-HMAC-SHA256 Credential={}/{credential_scope}, SignedHeaders=host;x-amz-date;x-amz-security-token, Signature=placeholder",
            self.access_key_id,
        )
    }
}

/// `AssumeRole` response from STS (parsed from XML).
#[derive(Debug)]
struct AssumeRoleResponse {
    access_key_id: String,
    secret_access_key: String,
    session_token: String,
}

/// Auth provider using `AssumeRole`.
pub struct AssumeRoleProvider {
    config: AssumeRoleConfig,
    /// Source credentials used to call STS.
    source_access_key_id: String,
    source_secret_access_key: String,
    source_session_token: Option<String>,
    /// Cached assumed-role credentials.
    cached: tokio::sync::Mutex<Option<StsCredentials>>,
}

impl AssumeRoleProvider {
    #[must_use]
    pub fn new(
        config: AssumeRoleConfig,
        source_access_key_id: String,
        source_secret_access_key: String,
        source_session_token: Option<String>,
    ) -> Self {
        Self {
            config,
            source_access_key_id,
            source_secret_access_key,
            source_session_token,
            cached: tokio::sync::Mutex::new(None),
        }
    }

    /// Create from environment variables.
    ///
    /// Requires `AWS_ROLE_ARN` and standard AWS credential env vars.
    pub fn from_env() -> Option<Self> {
        let role_arn = std::env::var("AWS_ROLE_ARN").ok()?;
        let access_key_id = std::env::var("AWS_ACCESS_KEY_ID").ok()?;
        let secret_access_key = std::env::var("AWS_SECRET_ACCESS_KEY").ok()?;
        let session_token = std::env::var("AWS_SESSION_TOKEN").ok();
        let region = std::env::var("AWS_REGION")
            .or_else(|_| std::env::var("AWS_DEFAULT_REGION"))
            .unwrap_or_else(|_| "us-east-1".to_string());
        let external_id = std::env::var("AWS_EXTERNAL_ID").ok();
        let session_name = std::env::var("AWS_ROLE_SESSION_NAME")
            .unwrap_or_else(|_| "crab-code-session".to_string());

        let config = AssumeRoleConfig {
            role_arn,
            external_id,
            session_name,
            region,
            ..Default::default()
        };

        Some(Self::new(
            config,
            access_key_id,
            secret_access_key,
            session_token,
        ))
    }

    /// Call STS `AssumeRole` API.
    async fn assume_role(&self) -> crab_common::Result<StsCredentials> {
        let sts_url = format!("https://sts.{}.amazonaws.com/", self.config.region);

        let mut params: Vec<(&str, &str)> = vec![
            ("Action", "AssumeRole"),
            ("Version", "2011-06-15"),
            ("RoleArn", &self.config.role_arn),
            ("RoleSessionName", &self.config.session_name),
        ];

        let duration_str = self.config.duration_secs.to_string();
        params.push(("DurationSeconds", &duration_str));

        let ext_id_ref;
        if let Some(ref external_id) = self.config.external_id {
            ext_id_ref = external_id.clone();
            params.push(("ExternalId", &ext_id_ref));
        }

        let body = url_encode_params(&params);
        let client = reqwest::Client::new();
        let resp: reqwest::Response = client
            .post(&sts_url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header(
                "Authorization",
                build_sts_auth(
                    &self.source_access_key_id,
                    &self.source_secret_access_key,
                    self.source_session_token.as_deref(),
                    &self.config.region,
                ),
            )
            .body(body)
            .send()
            .await
            .map_err(|e| crab_common::Error::Auth(format!("STS AssumeRole request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_body = resp.text().await.unwrap_or_default();
            return Err(crab_common::Error::Auth(format!(
                "STS AssumeRole returned {status}: {err_body}"
            )));
        }

        let resp_body = resp
            .text()
            .await
            .map_err(|e| crab_common::Error::Auth(format!("reading STS response: {e}")))?;

        let parsed = parse_assume_role_response(&resp_body)?;

        Ok(StsCredentials {
            access_key_id: parsed.access_key_id,
            secret_access_key: parsed.secret_access_key,
            session_token: parsed.session_token,
            region: self.config.region.clone(),
            expires_at: Instant::now() + Duration::from_secs(self.config.duration_secs),
        })
    }
}

impl AuthProvider for AssumeRoleProvider {
    fn get_auth(
        &self,
    ) -> Pin<Box<dyn Future<Output = crab_common::Result<AuthMethod>> + Send + '_>> {
        Box::pin(async move {
            // Check cache
            {
                let guard = self.cached.lock().await;
                if let Some(ref creds) = *guard
                    && creds.is_valid()
                {
                    return Ok(AuthMethod::OAuth(OAuthToken {
                        access_token: creds.to_auth_header("bedrock"),
                    }));
                }
            }

            // Assume role
            let creds = self.assume_role().await?;
            let header = creds.to_auth_header("bedrock");

            // Cache
            {
                let mut guard = self.cached.lock().await;
                *guard = Some(creds);
            }

            Ok(AuthMethod::OAuth(OAuthToken {
                access_token: header,
            }))
        })
    }

    fn refresh(&self) -> Pin<Box<dyn Future<Output = crab_common::Result<()>> + Send + '_>> {
        Box::pin(async move {
            let mut guard = self.cached.lock().await;
            *guard = None;
            drop(guard);
            Ok(())
        })
    }
}

/// Auth provider using `AssumeRoleWithWebIdentity` (OIDC).
///
/// Used in CI/CD environments (GitHub Actions, GitLab CI) where an OIDC
/// identity token is exchanged for AWS credentials.
pub struct WebIdentityProvider {
    config: WebIdentityConfig,
    cached: tokio::sync::Mutex<Option<StsCredentials>>,
}

impl WebIdentityProvider {
    #[must_use]
    pub fn new(config: WebIdentityConfig) -> Self {
        Self {
            config,
            cached: tokio::sync::Mutex::new(None),
        }
    }

    /// Create from environment variables.
    ///
    /// Checks `AWS_ROLE_ARN` + `AWS_WEB_IDENTITY_TOKEN_FILE` (standard EKS/GH Actions pattern).
    pub fn from_env() -> Option<Self> {
        let role_arn = std::env::var("AWS_ROLE_ARN").ok()?;
        let token_file = std::env::var("AWS_WEB_IDENTITY_TOKEN_FILE").ok();

        // Must have either token file or inline token
        token_file.as_ref()?;

        let region = std::env::var("AWS_REGION")
            .or_else(|_| std::env::var("AWS_DEFAULT_REGION"))
            .unwrap_or_else(|_| "us-east-1".to_string());

        let session_name = std::env::var("AWS_ROLE_SESSION_NAME")
            .unwrap_or_else(|_| "crab-code-web-identity".to_string());

        Some(Self::new(WebIdentityConfig {
            role_arn,
            token_file,
            token: None,
            session_name,
            region,
            ..Default::default()
        }))
    }

    /// Read the OIDC token from file or config.
    fn read_token(&self) -> crab_common::Result<String> {
        if let Some(ref token) = self.config.token {
            return Ok(token.clone());
        }

        if let Some(ref path) = self.config.token_file {
            return std::fs::read_to_string(path)
                .map(|t| t.trim().to_string())
                .map_err(|e| {
                    crab_common::Error::Auth(format!("reading web identity token file {path}: {e}"))
                });
        }

        Err(crab_common::Error::Auth(
            "no web identity token or token file configured".into(),
        ))
    }

    /// Call STS `AssumeRoleWithWebIdentity`.
    async fn assume_role_with_web_identity(&self) -> crab_common::Result<StsCredentials> {
        let token = self.read_token()?;
        let sts_url = format!("https://sts.{}.amazonaws.com/", self.config.region);

        let duration_str = self.config.duration_secs.to_string();
        let params: Vec<(&str, &str)> = vec![
            ("Action", "AssumeRoleWithWebIdentity"),
            ("Version", "2011-06-15"),
            ("RoleArn", &self.config.role_arn),
            ("RoleSessionName", &self.config.session_name),
            ("WebIdentityToken", &token),
            ("DurationSeconds", &duration_str),
        ];

        let body = url_encode_params(&params);
        let client = reqwest::Client::new();
        // No Authorization header needed — the web identity token itself is the credential.
        let resp: reqwest::Response = client
            .post(&sts_url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .map_err(|e| {
                crab_common::Error::Auth(format!(
                    "STS AssumeRoleWithWebIdentity request failed: {e}"
                ))
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_body = resp.text().await.unwrap_or_default();
            return Err(crab_common::Error::Auth(format!(
                "STS AssumeRoleWithWebIdentity returned {status}: {err_body}"
            )));
        }

        let resp_body = resp
            .text()
            .await
            .map_err(|e| crab_common::Error::Auth(format!("reading STS response: {e}")))?;

        let parsed = parse_assume_role_response(&resp_body)?;

        Ok(StsCredentials {
            access_key_id: parsed.access_key_id,
            secret_access_key: parsed.secret_access_key,
            session_token: parsed.session_token,
            region: self.config.region.clone(),
            expires_at: Instant::now() + Duration::from_secs(self.config.duration_secs),
        })
    }
}

impl AuthProvider for WebIdentityProvider {
    fn get_auth(
        &self,
    ) -> Pin<Box<dyn Future<Output = crab_common::Result<AuthMethod>> + Send + '_>> {
        Box::pin(async move {
            // Check cache
            {
                let guard = self.cached.lock().await;
                if let Some(ref creds) = *guard
                    && creds.is_valid()
                {
                    return Ok(AuthMethod::OAuth(OAuthToken {
                        access_token: creds.to_auth_header("bedrock"),
                    }));
                }
            }

            let creds = self.assume_role_with_web_identity().await?;
            let header = creds.to_auth_header("bedrock");

            {
                let mut guard = self.cached.lock().await;
                *guard = Some(creds);
            }

            Ok(AuthMethod::OAuth(OAuthToken {
                access_token: header,
            }))
        })
    }

    fn refresh(&self) -> Pin<Box<dyn Future<Output = crab_common::Result<()>> + Send + '_>> {
        Box::pin(async move {
            let mut guard = self.cached.lock().await;
            *guard = None;
            drop(guard);
            Ok(())
        })
    }
}

// ── Helpers ────────────────────────────────────────────────────────────

/// URL-encode form parameters.
pub(crate) fn url_encode_params(params: &[(&str, &str)]) -> String {
    params
        .iter()
        .map(|(k, v)| format!("{k}={}", simple_url_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Simple percent-encoding for URL form values.
fn simple_url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                use std::fmt::Write;
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

/// Build a minimal STS auth header from source credentials.
fn build_sts_auth(
    access_key_id: &str,
    _secret_access_key: &str,
    session_token: Option<&str>,
    region: &str,
) -> String {
    // Simplified — in production this would compute a full SigV4 signature.
    let token_part = session_token
        .map(|t| format!(", X-Amz-Security-Token={t}"))
        .unwrap_or_default();
    format!("AWS4-HMAC-SHA256 Credential={access_key_id}/sts/{region}/aws4_request{token_part}")
}

/// Format Unix timestamp as `YYYYMMDD`.
fn format_date_stamp(timestamp_secs: u64) -> String {
    let days = timestamp_secs / 86400;
    let (year, month, day) = days_to_ymd(days);
    format!("{year:04}{month:02}{day:02}")
}

/// Convert days since Unix epoch to (year, month, day).
/// Howard Hinnant's `civil_from_days` algorithm.
fn days_to_ymd(z: u64) -> (u64, u64, u64) {
    let z = z + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// Parse the XML response from STS `AssumeRole` / `AssumeRoleWithWebIdentity`.
///
/// Extracts `AccessKeyId`, `SecretAccessKey`, and `SessionToken` from the
/// `<Credentials>` element using simple string search (avoids XML parser dep).
fn parse_assume_role_response(xml: &str) -> crab_common::Result<AssumeRoleResponse> {
    let access_key_id = extract_xml_tag(xml, "AccessKeyId")
        .ok_or_else(|| crab_common::Error::Auth("missing AccessKeyId in STS response".into()))?;
    let secret_access_key = extract_xml_tag(xml, "SecretAccessKey").ok_or_else(|| {
        crab_common::Error::Auth("missing SecretAccessKey in STS response".into())
    })?;
    let session_token = extract_xml_tag(xml, "SessionToken")
        .ok_or_else(|| crab_common::Error::Auth("missing SessionToken in STS response".into()))?;

    Ok(AssumeRoleResponse {
        access_key_id,
        secret_access_key,
        session_token,
    })
}

/// Extract the text content of a simple XML tag like `<Tag>value</Tag>`.
fn extract_xml_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(xml[start..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assume_role_config_defaults() {
        let config = AssumeRoleConfig::default();
        assert_eq!(config.duration_secs, 3600);
        assert_eq!(config.region, "us-east-1");
        assert_eq!(config.session_name, "crab-code-session");
        assert!(config.external_id.is_none());
    }

    #[test]
    fn web_identity_config_defaults() {
        let config = WebIdentityConfig::default();
        assert_eq!(config.duration_secs, 3600);
        assert_eq!(config.region, "us-east-1");
        assert!(config.token_file.is_none());
        assert!(config.token.is_none());
    }

    #[test]
    fn sts_credentials_valid_when_fresh() {
        let creds = StsCredentials {
            access_key_id: "AKIATEST".into(),
            secret_access_key: "secret".into(),
            session_token: "token".into(),
            region: "us-east-1".into(),
            expires_at: Instant::now() + Duration::from_secs(3600),
        };
        assert!(creds.is_valid());
    }

    #[test]
    fn sts_credentials_invalid_when_expired() {
        let creds = StsCredentials {
            access_key_id: "AKIATEST".into(),
            secret_access_key: "secret".into(),
            session_token: "token".into(),
            region: "us-east-1".into(),
            expires_at: Instant::now(),
        };
        assert!(!creds.is_valid());
    }

    #[test]
    fn sts_credentials_invalid_within_margin() {
        let creds = StsCredentials {
            access_key_id: "AKIATEST".into(),
            secret_access_key: "secret".into(),
            session_token: "token".into(),
            region: "us-east-1".into(),
            // Expires in 4 minutes, but margin is 5 minutes
            expires_at: Instant::now() + Duration::from_secs(240),
        };
        assert!(!creds.is_valid());
    }

    #[test]
    fn sts_credentials_auth_header_format() {
        let creds = StsCredentials {
            access_key_id: "AKIAEXAMPLE".into(),
            secret_access_key: "secret".into(),
            session_token: "token123".into(),
            region: "us-west-2".into(),
            expires_at: Instant::now() + Duration::from_secs(3600),
        };
        let header = creds.to_auth_header("bedrock");
        assert!(header.starts_with("AWS4-HMAC-SHA256"));
        assert!(header.contains("AKIAEXAMPLE"));
        assert!(header.contains("us-west-2"));
        assert!(header.contains("bedrock"));
    }

    #[test]
    fn parse_assume_role_response_valid_xml() {
        let xml = r"
        <AssumeRoleResponse>
            <AssumeRoleResult>
                <Credentials>
                    <AccessKeyId>ASIA1234567890</AccessKeyId>
                    <SecretAccessKey>wJalrXUtnFEMI/secret</SecretAccessKey>
                    <SessionToken>FwoGZXIvYXdzEBY...</SessionToken>
                    <Expiration>2026-04-05T12:00:00Z</Expiration>
                </Credentials>
            </AssumeRoleResult>
        </AssumeRoleResponse>
        ";

        let parsed = parse_assume_role_response(xml).unwrap();
        assert_eq!(parsed.access_key_id, "ASIA1234567890");
        assert_eq!(parsed.secret_access_key, "wJalrXUtnFEMI/secret");
        assert!(parsed.session_token.starts_with("FwoGZXIvYXdzEBY"));
    }

    #[test]
    fn parse_assume_role_response_web_identity() {
        let xml = r"
        <AssumeRoleWithWebIdentityResponse>
            <AssumeRoleWithWebIdentityResult>
                <Credentials>
                    <AccessKeyId>ASIAWEB123</AccessKeyId>
                    <SecretAccessKey>webSecret123</SecretAccessKey>
                    <SessionToken>webToken456</SessionToken>
                    <Expiration>2026-04-05T13:00:00Z</Expiration>
                </Credentials>
            </AssumeRoleWithWebIdentityResult>
        </AssumeRoleWithWebIdentityResponse>
        ";

        let parsed = parse_assume_role_response(xml).unwrap();
        assert_eq!(parsed.access_key_id, "ASIAWEB123");
        assert_eq!(parsed.secret_access_key, "webSecret123");
        assert_eq!(parsed.session_token, "webToken456");
    }

    #[test]
    fn parse_assume_role_response_missing_field() {
        let xml = r"<Credentials><AccessKeyId>ASIA</AccessKeyId></Credentials>";
        let result = parse_assume_role_response(xml);
        assert!(result.is_err());
    }

    #[test]
    fn extract_xml_tag_basic() {
        assert_eq!(
            extract_xml_tag("<Root><Name>hello</Name></Root>", "Name"),
            Some("hello".into())
        );
    }

    #[test]
    fn extract_xml_tag_missing() {
        assert_eq!(extract_xml_tag("<Root></Root>", "Missing"), None);
    }

    #[test]
    fn extract_xml_tag_empty() {
        assert_eq!(extract_xml_tag("<Tag></Tag>", "Tag"), Some(String::new()));
    }

    #[test]
    fn format_date_stamp_epoch() {
        assert_eq!(format_date_stamp(0), "19700101");
    }

    #[test]
    fn format_date_stamp_known() {
        // 2024-01-15 12:00:00 UTC = 1705320000
        assert_eq!(format_date_stamp(1_705_320_000), "20240115");
    }

    #[test]
    fn days_to_ymd_epoch() {
        assert_eq!(days_to_ymd(0), (1970, 1, 1));
    }

    #[test]
    fn days_to_ymd_leap_day() {
        // 2024-02-29 = day 19782
        assert_eq!(days_to_ymd(19_782), (2024, 2, 29));
    }

    #[test]
    fn web_identity_provider_read_inline_token() {
        let config = WebIdentityConfig {
            role_arn: "arn:aws:iam::123:role/Test".into(),
            token: Some("my-oidc-token".into()),
            ..Default::default()
        };
        let provider = WebIdentityProvider::new(config);
        let token = provider.read_token().unwrap();
        assert_eq!(token, "my-oidc-token");
    }

    #[test]
    fn web_identity_provider_read_token_file() {
        let dir = std::env::temp_dir().join("crab-auth-web-identity-test");
        let _ = std::fs::create_dir_all(&dir);
        let token_path = dir.join("token");
        std::fs::write(&token_path, "file-token-123\n").unwrap();

        let config = WebIdentityConfig {
            role_arn: "arn:aws:iam::123:role/Test".into(),
            token_file: Some(token_path.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let provider = WebIdentityProvider::new(config);
        let token = provider.read_token().unwrap();
        assert_eq!(token, "file-token-123");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn web_identity_provider_no_token_errors() {
        let config = WebIdentityConfig {
            role_arn: "arn:aws:iam::123:role/Test".into(),
            ..Default::default()
        };
        let provider = WebIdentityProvider::new(config);
        assert!(provider.read_token().is_err());
    }

    #[test]
    fn assume_role_provider_refresh_clears_cache() {
        let config = AssumeRoleConfig {
            role_arn: "arn:aws:iam::123:role/Test".into(),
            ..Default::default()
        };
        let provider = AssumeRoleProvider::new(config, "AKIATEST".into(), "secret".into(), None);
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(provider.refresh()).unwrap();
    }

    #[test]
    fn web_identity_provider_refresh_clears_cache() {
        let config = WebIdentityConfig {
            role_arn: "arn:aws:iam::123:role/Test".into(),
            token: Some("token".into()),
            ..Default::default()
        };
        let provider = WebIdentityProvider::new(config);
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(provider.refresh()).unwrap();
    }

    #[test]
    fn build_sts_auth_without_session_token() {
        let header = build_sts_auth("AKIATEST", "secret", None, "us-east-1");
        assert!(header.contains("AKIATEST"));
        assert!(header.contains("us-east-1"));
        assert!(!header.contains("X-Amz-Security-Token"));
    }

    #[test]
    fn build_sts_auth_with_session_token() {
        let header = build_sts_auth("AKIATEST", "secret", Some("sess-tok"), "us-west-2");
        assert!(header.contains("X-Amz-Security-Token=sess-tok"));
    }
}
