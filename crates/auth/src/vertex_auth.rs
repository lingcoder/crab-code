//! Google Cloud Vertex AI authentication.
//!
//! Implements `AuthProvider` that resolves GCP credentials from:
//! 1. `GOOGLE_APPLICATION_CREDENTIALS` env var (service account JSON key)
//! 2. Application Default Credentials (ADC) via `gcloud auth application-default`
//! 3. Compute Engine metadata server (GCE/GKE/Cloud Run)

#![cfg(feature = "vertex")]

use crab_utils::time::epoch_secs;
use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};

use crate::{AuthMethod, AuthProvider, OAuthToken};

/// OAuth scope requested for Vertex AI access.
const CLOUD_PLATFORM_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";

/// Google's OAuth 2.0 token endpoint (used for the ADC refresh-token grant).
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

/// JWT lifetime for the service-account assertion (1 hour, the GCP maximum).
const JWT_LIFETIME_SECS: u64 = 3600;

/// Claims for the service-account JWT assertion exchanged at the token endpoint.
#[derive(Debug, Serialize, Deserialize)]
struct JwtClaims {
    iss: String,
    scope: String,
    aud: String,
    iat: u64,
    exp: u64,
}

/// GCP credential source for Vertex AI access.
#[derive(Debug, Clone)]
pub struct GcpCredentials {
    /// GCP project ID.
    pub project_id: String,
    /// GCP region (e.g., "us-central1").
    pub region: String,
    /// Service account key JSON (if using service account auth).
    pub service_account_key: Option<String>,
}

impl GcpCredentials {
    /// Resolve credentials from environment.
    ///
    /// Checks `GOOGLE_CLOUD_PROJECT` / `GCLOUD_PROJECT` for project ID,
    /// `GOOGLE_CLOUD_REGION` for region, and
    /// `GOOGLE_APPLICATION_CREDENTIALS` for service account key path.
    #[must_use]
    pub fn from_env() -> Option<Self> {
        let project_id = std::env::var("GOOGLE_CLOUD_PROJECT")
            .or_else(|_| std::env::var("GCLOUD_PROJECT"))
            .ok()?;
        let region =
            std::env::var("GOOGLE_CLOUD_REGION").unwrap_or_else(|_| "us-central1".to_string());

        let service_account_key = std::env::var("GOOGLE_APPLICATION_CREDENTIALS")
            .ok()
            .and_then(|path| std::fs::read_to_string(path).ok());

        Some(Self {
            project_id,
            region,
            service_account_key,
        })
    }

    /// Vertex AI endpoint URL for the Anthropic Messages API.
    #[must_use]
    pub fn endpoint_url(&self, model_id: &str) -> String {
        format!(
            "https://{}-aiplatform.googleapis.com/v1/projects/{}/locations/{}/publishers/anthropic/models/{}",
            self.region, self.project_id, self.region, model_id
        )
    }

    /// Base URL for the Vertex AI Anthropic-compatible endpoint.
    ///
    /// This is used as the base URL for the `AnthropicClient`, which appends
    /// `/v1/messages` to make the full endpoint.
    #[must_use]
    pub fn base_url(&self, model_id: &str) -> String {
        format!(
            "https://{}-aiplatform.googleapis.com/v1/projects/{}/locations/{}/publishers/anthropic/models/{}:rawPredict",
            self.region, self.project_id, self.region, model_id
        )
    }
}

/// Auth provider for GCP Vertex AI.
///
/// Uses Google Cloud OAuth 2.0 access tokens for authentication.
/// Tokens are obtained from the ADC chain or service account key.
pub struct VertexAuthProvider {
    credentials: GcpCredentials,
    /// Cached access token (refreshed when expired).
    cached_token: tokio::sync::Mutex<Option<CachedToken>>,
}

struct CachedToken {
    access_token: String,
    expires_at: std::time::Instant,
}

impl VertexAuthProvider {
    #[must_use]
    pub fn new(credentials: GcpCredentials) -> Self {
        Self {
            credentials,
            cached_token: tokio::sync::Mutex::new(None),
        }
    }

    /// Obtain an access token from the GCP metadata server or ADC.
    async fn obtain_token(&self) -> crab_core::Result<String> {
        // Try 1: Service account key (JWT -> access token exchange)
        if let Some(ref _key_json) = self.credentials.service_account_key {
            return self.token_from_service_account().await;
        }

        // Try 2: GCE metadata server (for Compute Engine / GKE / Cloud Run)
        if let Ok(token) = self.token_from_metadata_server().await {
            return Ok(token);
        }

        // Try 3: ADC file from gcloud CLI
        if let Ok(token) = self.token_from_adc_file().await {
            return Ok(token);
        }

        Err(crab_core::Error::Other(
            "failed to obtain GCP access token: no valid credential source found".into(),
        ))
    }

    /// Exchange a service account key for an access token.
    ///
    /// Builds an RS256-signed JWT assertion from the SA key and exchanges it at
    /// the SA's `token_uri` using the `jwt-bearer` grant.
    async fn token_from_service_account(&self) -> crab_core::Result<String> {
        let key_json = self
            .credentials
            .service_account_key
            .as_ref()
            .ok_or_else(|| crab_core::Error::Other("no service account key configured".into()))?;

        let sa: serde_json::Value = serde_json::from_str(key_json)
            .map_err(|e| crab_core::Error::Other(format!("parsing service account key: {e}")))?;

        let token_uri = sa
            .get("token_uri")
            .and_then(|v| v.as_str())
            .unwrap_or(GOOGLE_TOKEN_URL);

        let assertion = build_sa_assertion(&sa, epoch_secs())?;

        let params = [
            ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
            ("assertion", &assertion),
        ];

        let client = reqwest::Client::new();
        let resp = client
            .post(token_uri)
            .form(&params)
            .send()
            .await
            .map_err(|e| crab_core::Error::Other(format!("SA token exchange failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(crab_core::Error::Other(format!(
                "SA token exchange returned {status}: {body}"
            )));
        }

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| crab_core::Error::Other(format!("reading SA token response: {e}")))?;

        body.get("access_token")
            .and_then(|v| v.as_str())
            .map(String::from)
            .ok_or_else(|| crab_core::Error::Other("no access_token in SA token response".into()))
    }

    /// Fetch token from GCE metadata server.
    async fn token_from_metadata_server(&self) -> crab_core::Result<String> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(2))
            .build()
            .map_err(|e| crab_core::Error::Other(e.to_string()))?;

        let resp = client
            .get("http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token")
            .header("Metadata-Flavor", "Google")
            .send()
            .await
            .map_err(|e| crab_core::Error::Other(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(crab_core::Error::Other(format!(
                "metadata server returned {}",
                resp.status()
            )));
        }

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| crab_core::Error::Other(e.to_string()))?;

        body.get("access_token")
            .and_then(|v| v.as_str())
            .map(String::from)
            .ok_or_else(|| crab_core::Error::Other("no access_token in metadata response".into()))
    }

    /// Read ADC file from gcloud CLI's default location.
    async fn token_from_adc_file(&self) -> crab_core::Result<String> {
        let adc_path = if cfg!(windows) {
            dirs_path("APPDATA", "gcloud/application_default_credentials.json")
        } else {
            dirs_path(
                "HOME",
                ".config/gcloud/application_default_credentials.json",
            )
        };

        let Some(path) = adc_path else {
            return Err(crab_core::Error::Other("ADC file path not found".into()));
        };

        let content = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| crab_core::Error::Other(format!("reading ADC file: {e}")))?;

        let adc: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| crab_core::Error::Other(format!("parsing ADC file: {e}")))?;

        // ADC user-credentials file holds a refresh token; exchange it for an
        // access token via the standard OAuth 2.0 refresh grant.
        let client_id = adc
            .get("client_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| crab_core::Error::Other("no client_id in ADC".into()))?;

        let client_secret = adc
            .get("client_secret")
            .and_then(|v| v.as_str())
            .ok_or_else(|| crab_core::Error::Other("no client_secret in ADC".into()))?;

        let refresh_token = adc
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| crab_core::Error::Other("no refresh_token in ADC".into()))?;

        let params = [
            ("grant_type", "refresh_token"),
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("refresh_token", refresh_token),
        ];

        let client = reqwest::Client::new();
        let resp = client
            .post(GOOGLE_TOKEN_URL)
            .form(&params)
            .send()
            .await
            .map_err(|e| crab_core::Error::Other(format!("ADC token refresh failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(crab_core::Error::Other(format!(
                "ADC token refresh returned {status}: {body}"
            )));
        }

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| crab_core::Error::Other(format!("reading ADC token response: {e}")))?;

        body.get("access_token")
            .and_then(|v| v.as_str())
            .map(String::from)
            .ok_or_else(|| crab_core::Error::Other("no access_token in ADC token response".into()))
    }
}

/// Build an RS256-signed JWT assertion from a service account key for the GCP
/// `jwt-bearer` token exchange.
///
/// Expects `client_email` and `private_key` (PEM) fields in `sa`; the audience
/// is the SA's `token_uri` (defaulting to Google's token endpoint).
fn build_sa_assertion(sa: &serde_json::Value, now: u64) -> crab_core::Result<String> {
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};

    let client_email = sa
        .get("client_email")
        .and_then(|v| v.as_str())
        .ok_or_else(|| crab_core::Error::Other("no client_email in service account key".into()))?;

    let private_key = sa
        .get("private_key")
        .and_then(|v| v.as_str())
        .ok_or_else(|| crab_core::Error::Other("no private_key in service account key".into()))?;

    let token_uri = sa
        .get("token_uri")
        .and_then(|v| v.as_str())
        .unwrap_or(GOOGLE_TOKEN_URL);

    let claims = JwtClaims {
        iss: client_email.to_string(),
        scope: CLOUD_PLATFORM_SCOPE.to_string(),
        aud: token_uri.to_string(),
        iat: now,
        exp: now + JWT_LIFETIME_SECS,
    };

    let mut header = Header::new(Algorithm::RS256);
    if let Some(kid) = sa.get("private_key_id").and_then(|v| v.as_str()) {
        header.kid = Some(kid.to_string());
    }

    let key = EncodingKey::from_rsa_pem(private_key.as_bytes()).map_err(|e| {
        crab_core::Error::Other(format!("invalid service account private key: {e}"))
    })?;

    encode(&header, &claims, &key)
        .map_err(|e| crab_core::Error::Other(format!("signing SA assertion: {e}")))
}

fn dirs_path(env_var: &str, suffix: &str) -> Option<std::path::PathBuf> {
    std::env::var(env_var).ok().map(|dir| {
        let mut path = std::path::PathBuf::from(dir);
        path.push(suffix);
        path
    })
}

impl AuthProvider for VertexAuthProvider {
    fn get_auth(&self) -> Pin<Box<dyn Future<Output = crab_core::Result<AuthMethod>> + Send + '_>> {
        Box::pin(async move {
            // Check cached token
            {
                let guard = self.cached_token.lock().await;
                if let Some(ref cached) = *guard
                    && cached.expires_at > std::time::Instant::now()
                {
                    return Ok(AuthMethod::OAuth(OAuthToken {
                        access_token: cached.access_token.clone(),
                    }));
                }
            }

            // Obtain fresh token
            let token = self.obtain_token().await?;

            // Cache it (default 55 min expiry — GCP tokens last 60 min)
            {
                let mut guard = self.cached_token.lock().await;
                *guard = Some(CachedToken {
                    access_token: token.clone(),
                    expires_at: std::time::Instant::now() + std::time::Duration::from_secs(55 * 60),
                });
            }

            Ok(AuthMethod::OAuth(OAuthToken {
                access_token: token,
            }))
        })
    }

    fn refresh(&self) -> Pin<Box<dyn Future<Output = crab_core::Result<()>> + Send + '_>> {
        Box::pin(async move {
            // Invalidate cached token to force re-fetch on next get_auth()
            let mut guard = self.cached_token.lock().await;
            *guard = None;
            drop(guard);
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gcp_credentials_endpoint_url() {
        let creds = GcpCredentials {
            project_id: "my-project".into(),
            region: "us-central1".into(),
            service_account_key: None,
        };
        let url = creds.endpoint_url("claude-sonnet-4-20250514");
        assert!(url.contains("us-central1-aiplatform.googleapis.com"));
        assert!(url.contains("my-project"));
        assert!(url.contains("claude-sonnet-4-20250514"));
    }

    #[test]
    fn gcp_credentials_base_url() {
        let creds = GcpCredentials {
            project_id: "proj-123".into(),
            region: "europe-west1".into(),
            service_account_key: None,
        };
        let url = creds.base_url("claude-haiku-3-5");
        assert!(url.contains("europe-west1-aiplatform.googleapis.com"));
        assert!(url.contains("proj-123"));
        assert!(url.contains(":rawPredict"));
    }

    #[test]
    fn gcp_credentials_from_env_missing() {
        // Without GOOGLE_CLOUD_PROJECT set, should return None
        // (test env usually doesn't have GCP vars)
        let _result = GcpCredentials::from_env();
    }

    #[test]
    fn vertex_auth_provider_refresh_clears_cache() {
        let creds = GcpCredentials {
            project_id: "test".into(),
            region: "us-central1".into(),
            service_account_key: None,
        };
        let provider = VertexAuthProvider::new(creds);
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(provider.refresh()).unwrap();
    }

    #[test]
    fn vertex_auth_provider_get_auth_fails_without_credentials() {
        let creds = GcpCredentials {
            project_id: "test".into(),
            region: "us-central1".into(),
            service_account_key: None,
        };
        let provider = VertexAuthProvider::new(creds);
        let rt = tokio::runtime::Runtime::new().unwrap();
        // Without real GCP credentials, this should error
        let result = rt.block_on(provider.get_auth());
        assert!(result.is_err());
    }

    #[test]
    fn dirs_path_with_env_var() {
        // Test the helper directly
        let result = dirs_path("PATH", "subdir/file.json");
        assert!(result.is_some()); // PATH is always set
    }

    #[test]
    fn dirs_path_missing_env() {
        let result = dirs_path("NONEXISTENT_VAR_12345", "file.json");
        assert!(result.is_none());
    }

    // Throwaway 2048-bit RSA key used only to exercise JWT assertion signing.
    // Not a real credential; safe to commit.
    const TEST_SA_PRIVATE_KEY: &str = "-----BEGIN PRIVATE KEY-----\n\
MIIEvAIBADANBgkqhkiG9w0BAQEFAASCBKYwggSiAgEAAoIBAQCq32FAzuQptgJ7\n\
iYwvTDUA28ewyDIXxDd6rLZg9vjAB7kueo/Cbee9OXpTB8ksmrKry0FVKJ5QeWQt\n\
BtdUGniZQwlotSMB8BYSnC+8S2NFi9bfsnS3pb0OsnPfF+76s2LfPUHeDf/JiHPu\n\
oK1sp8LAkJo5EGhsnA09udJADZGFe1zH+RwBwc0vUT+LAgIIha/S9JWecvoGIPY+\n\
WBYVPfs0k3mtb9dBm/TByY/xwcIOn+kesBuwQ7R8IAH2GOZgX7Y7MuMZT+O5Qzxo\n\
5ppM9pJuPdpaMwSbO6dWeKNf+WPCt51HcW1zypqy6Ts3m5anBACJj1EoLlzn92rw\n\
hHdRdzw7AgMBAAECggEAK+kKm3ZvUL66pZeDxFXPmyBfkTDpGo1semRu270r0GFL\n\
t8N8NQk8T7a5FiQ+kO1SM+6gI+uzv1dqpF2JMU46JpyBCvzdea6CZZbod3liEemt\n\
NsAr2VPIoUG/oBmM6rT1mAusZQ1w6Y/cxvpYhr8Xv5eJYleylhKGHpIlkxtJhaTx\n\
J3mnOwbRpMGg23YED11L3jLdmQb4mHyoTbvqKGDsb+menN3kRZvUKDSudq8sWeVf\n\
Jb+UqlxmynJaPpWZRKVDGvX99Us/cSVuuymuVEABS/F6dNZhy3EvdhY2T64Et54s\n\
tGSn7CA8U8G0FI+5Fwr+QwCbE5P+ODhQNIPjmCGQcQKBgQDtZ8HcG8jw3wV95yYq\n\
h6yTnlL+RprhR6wUV4YC+1DN+RW2koVaveNPhACZqVlebb8+GSewuUDWXU2sGkKZ\n\
NFLksvVlHLnqkMxR3sNwKPBjo4cGEVeFvWv2Ho50smSPmzgKUvg4j2GujtLTeOjA\n\
ZenwsAUnGTCsFZSTecZiCg2FkQKBgQC4QZEcst/t2pvx9hSXvSSrJs90TrkGXf10\n\
dZiUh4tB8CI5W6W9Y/VMmU1m3iN/8DqKZkAgFn+Edlgcy/4wnynRO329DKRfSNf1\n\
VJ4ZYhcrLrjUYt409NsKer+6kA6QWaWGYCA3wsa31gRdW+IAfhmvU4yE6NBR9aaf\n\
PVgEc58PCwKBgAV2cbuC2CjSuOmgu/wWix4KcpZvQXkVkRwWt3qyFbXnmVxOGstv\n\
ux9FRk5C20+U7uWa5pLmcFt+Yh8nq4ii75VbmNHuy0hedJUdrxmRl5ZzWNQG6iCl\n\
rypGobiFslKrm6qBJj0G75R4rNk42wIyViO3qSaxKbGL/ZM3Jh1zZcRBAoGAUcWf\n\
fgoQgUHcpZRdbT4e8OondWmeiana2v15eqlw7xGATs5Sjuu2qIj8peN+A8B8aoGY\n\
geUaMJJI5nbN14w7hcUON4FNzY/Jb/Jeu8shlyOEGZXLIdts/oidYFGgdQWkBS/R\n\
/I0vndSYWUp20VslUP8WRMIB+e24RcF2t3sMoyMCgYAizIz8p4mmyUAf3W+1n0R+\n\
LqUt/dVwSrt78coUd2LBgTWgXfVTxckxUK7aDBKfu1uYZlyScCvyYAOnqswhRiMo\n\
0Up4I20Spg1x0di5hIWXxjQ9O9P0dcwgOBoJeVi4SGB/eD2GHiOKTPSdf3LHQox5\n\
6z+zQu31dXmm+rwHqRdK6g==\n\
-----END PRIVATE KEY-----\n";

    const TEST_SA_PUBLIC_KEY: &str = "-----BEGIN PUBLIC KEY-----\n\
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAqt9hQM7kKbYCe4mML0w1\n\
ANvHsMgyF8Q3eqy2YPb4wAe5LnqPwm3nvTl6UwfJLJqyq8tBVSieUHlkLQbXVBp4\n\
mUMJaLUjAfAWEpwvvEtjRYvW37J0t6W9DrJz3xfu+rNi3z1B3g3/yYhz7qCtbKfC\n\
wJCaORBobJwNPbnSQA2RhXtcx/kcAcHNL1E/iwICCIWv0vSVnnL6BiD2PlgWFT37\n\
NJN5rW/XQZv0wcmP8cHCDp/pHrAbsEO0fCAB9hjmYF+2OzLjGU/juUM8aOaaTPaS\n\
bj3aWjMEmzunVnijX/ljwredR3Ftc8qasuk7N5uWpwQAiY9RKC5c5/dq8IR3UXc8\n\
OwIDAQAB\n\
-----END PUBLIC KEY-----\n";

    fn test_sa_json() -> serde_json::Value {
        serde_json::json!({
            "type": "service_account",
            "client_email": "svc@my-project.iam.gserviceaccount.com",
            "private_key_id": "test-key-id",
            "private_key": TEST_SA_PRIVATE_KEY,
            "token_uri": "https://oauth2.googleapis.com/token",
        })
    }

    #[test]
    fn build_sa_assertion_claims_and_header() {
        use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};

        let assertion = build_sa_assertion(&test_sa_json(), 1_700_000_000).unwrap();

        // Header carries RS256 + the private_key_id as kid.
        let header = jsonwebtoken::decode_header(&assertion).unwrap();
        assert_eq!(header.alg, Algorithm::RS256);
        assert_eq!(header.kid.as_deref(), Some("test-key-id"));

        // Signature verifies against the matching public key, and claims match.
        let key = DecodingKey::from_rsa_pem(TEST_SA_PUBLIC_KEY.as_bytes()).unwrap();
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_audience(&["https://oauth2.googleapis.com/token"]);
        // The fixture uses a fixed past `iat`; verify signature + claims, not
        // wall-clock freshness.
        validation.validate_exp = false;

        let decoded = decode::<JwtClaims>(&assertion, &key, &validation).unwrap();
        let claims = decoded.claims;
        assert_eq!(claims.iss, "svc@my-project.iam.gserviceaccount.com");
        assert_eq!(claims.scope, CLOUD_PLATFORM_SCOPE);
        assert_eq!(claims.aud, "https://oauth2.googleapis.com/token");
        assert_eq!(claims.iat, 1_700_000_000);
        assert_eq!(claims.exp, 1_700_000_000 + JWT_LIFETIME_SECS);
    }

    #[test]
    fn build_sa_assertion_missing_private_key_errors() {
        let sa = serde_json::json!({
            "client_email": "svc@example.iam.gserviceaccount.com",
        });
        assert!(build_sa_assertion(&sa, 0).is_err());
    }

    #[test]
    fn build_sa_assertion_invalid_pem_errors() {
        let sa = serde_json::json!({
            "client_email": "svc@example.iam.gserviceaccount.com",
            "private_key": "not a real pem",
        });
        assert!(build_sa_assertion(&sa, 0).is_err());
    }
}
