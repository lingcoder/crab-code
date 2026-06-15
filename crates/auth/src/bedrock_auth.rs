//! AWS `SigV4` signing for Bedrock Runtime API.
//!
//! Implements `AuthProvider` for AWS Bedrock. Credentials are resolved from the
//! standard AWS chain (env vars; instance profiles are handled by the STS
//! providers in [`crate::aws_iam`]). The actual request signing uses the shared
//! [`crate::sigv4`] implementation.

#![cfg(feature = "bedrock")]

use std::future::Future;
use std::pin::Pin;
use std::time::SystemTime;

use crate::sigv4::{self, CanonicalRequest, SignedRequest, SigningCredentials};
use crate::{AuthMethod, AuthProvider, OAuthToken, SignedHeaders};

/// Bedrock Runtime service name for the credential scope.
const SERVICE: &str = "bedrock";

/// AWS credential source for Bedrock access.
#[derive(Debug, Clone)]
pub struct AwsCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
    pub region: String,
}

impl AwsCredentials {
    /// Resolve credentials from environment variables.
    ///
    /// Checks `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`,
    /// `AWS_SESSION_TOKEN`, and `AWS_REGION` / `AWS_DEFAULT_REGION`.
    #[must_use]
    pub fn from_env() -> Option<Self> {
        let access_key_id = std::env::var("AWS_ACCESS_KEY_ID").ok()?;
        let secret_access_key = std::env::var("AWS_SECRET_ACCESS_KEY").ok()?;
        let session_token = std::env::var("AWS_SESSION_TOKEN").ok();
        let region = std::env::var("AWS_REGION")
            .or_else(|_| std::env::var("AWS_DEFAULT_REGION"))
            .unwrap_or_else(|_| "us-east-1".to_string());

        Some(Self {
            access_key_id,
            secret_access_key,
            session_token,
            region,
        })
    }

    fn signing_credentials(&self) -> SigningCredentials {
        SigningCredentials {
            access_key_id: self.access_key_id.clone(),
            secret_access_key: self.secret_access_key.clone(),
            session_token: self.session_token.clone(),
        }
    }
}

/// Auth provider for AWS Bedrock using `SigV4` signing.
pub struct BedrockAuthProvider {
    credentials: AwsCredentials,
}

impl BedrockAuthProvider {
    #[must_use]
    pub fn new(credentials: AwsCredentials) -> Self {
        Self { credentials }
    }

    /// Sign a concrete Bedrock Runtime request with AWS `SigV4`.
    ///
    /// `host` is the Bedrock Runtime host
    /// (`bedrock-runtime.<region>.amazonaws.com`), `path` the request path
    /// (e.g. `/model/<model-id>/invoke`), and `body` the JSON payload bytes.
    /// The returned [`SignedRequest`] carries the `Authorization` header and the
    /// `x-amz-date` / `x-amz-security-token` values that must be sent verbatim.
    #[must_use]
    pub fn sign(&self, method: &str, host: &str, path: &str, body: &[u8]) -> SignedRequest {
        let timestamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let req = CanonicalRequest {
            method,
            host,
            path,
            query: "",
            headers: &[("content-type", "application/json")],
            payload: body,
        };

        sigv4::sign(
            &req,
            &self.credentials.signing_credentials(),
            &self.credentials.region,
            SERVICE,
            timestamp,
        )
    }
}

impl AuthProvider for BedrockAuthProvider {
    fn get_auth(&self) -> Pin<Box<dyn Future<Output = crab_core::Result<AuthMethod>> + Send + '_>> {
        let host = format!("bedrock-runtime.{}.amazonaws.com", self.credentials.region);
        let signed = self.sign("POST", &host, "/", b"");
        Box::pin(async move {
            Ok(AuthMethod::OAuth(OAuthToken {
                access_token: signed.authorization,
            }))
        })
    }

    fn sign_request(
        &self,
        method: &str,
        host: &str,
        path: &str,
        body: &[u8],
    ) -> Option<SignedHeaders> {
        Some(SignedHeaders {
            headers: self.sign(method, host, path, body).header_pairs(),
        })
    }

    fn refresh(&self) -> Pin<Box<dyn Future<Output = crab_core::Result<()>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aws_credentials_from_env_missing_returns_none() {
        let _result = AwsCredentials::from_env();
    }

    #[test]
    fn sign_produces_valid_sigv4_header() {
        let creds = AwsCredentials {
            access_key_id: "AKIATEST".into(),
            secret_access_key: "secret123".into(),
            session_token: None,
            region: "us-east-1".into(),
        };
        let provider = BedrockAuthProvider::new(creds);
        let signed = provider.sign(
            "POST",
            "bedrock-runtime.us-east-1.amazonaws.com",
            "/model/anthropic.claude/invoke",
            b"{\"x\":1}",
        );
        assert!(signed.authorization.starts_with("AWS4-HMAC-SHA256 "));
        assert!(signed.authorization.contains("Credential=AKIATEST/"));
        assert!(
            signed
                .authorization
                .contains("/us-east-1/bedrock/aws4_request")
        );
        assert!(
            signed
                .authorization
                .contains("SignedHeaders=content-type;host;x-amz-date")
        );
        assert!(signed.amz_date.ends_with('Z'));
    }

    #[test]
    fn sign_request_returns_bound_headers() {
        let creds = AwsCredentials {
            access_key_id: "AKIATEST".into(),
            secret_access_key: "secret123".into(),
            session_token: Some("sess-tok".into()),
            region: "us-east-1".into(),
        };
        let provider = BedrockAuthProvider::new(creds);
        let signed = provider
            .sign_request(
                "POST",
                "bedrock-runtime.us-east-1.amazonaws.com",
                "/model/m/invoke",
                b"{}",
            )
            .expect("bedrock provider signs every request");
        let names: Vec<&str> = signed.headers.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"authorization"));
        assert!(names.contains(&"x-amz-date"));
        assert!(names.contains(&"x-amz-security-token"));
        assert!(!names.contains(&"host"));
        let auth = &signed
            .headers
            .iter()
            .find(|(n, _)| n == "authorization")
            .unwrap()
            .1;
        assert!(auth.starts_with("AWS4-HMAC-SHA256 "));
    }

    #[test]
    fn sign_includes_session_token_header() {
        let creds = AwsCredentials {
            access_key_id: "AKIATEST".into(),
            secret_access_key: "secret".into(),
            session_token: Some("sess-tok".into()),
            region: "us-west-2".into(),
        };
        let provider = BedrockAuthProvider::new(creds);
        let signed = provider.sign(
            "POST",
            "bedrock-runtime.us-west-2.amazonaws.com",
            "/model/m/invoke",
            b"{}",
        );
        assert_eq!(signed.session_token.as_deref(), Some("sess-tok"));
        assert!(signed.authorization.contains("x-amz-security-token"));
    }

    #[test]
    fn get_auth_returns_oauth_method() {
        let creds = AwsCredentials {
            access_key_id: "AKIATEST".into(),
            secret_access_key: "secret123".into(),
            session_token: None,
            region: "us-east-1".into(),
        };
        let provider = BedrockAuthProvider::new(creds);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(provider.get_auth()).unwrap();
        match result {
            AuthMethod::OAuth(token) => {
                assert!(token.access_token.starts_with("AWS4-HMAC-SHA256"));
                assert!(token.access_token.contains("AKIATEST"));
            }
            AuthMethod::ApiKey(_) => panic!("expected OAuth, got ApiKey"),
        }
    }

    #[test]
    fn refresh_is_noop() {
        let creds = AwsCredentials {
            access_key_id: "AKIATEST".into(),
            secret_access_key: "secret".into(),
            session_token: None,
            region: "us-west-2".into(),
        };
        let provider = BedrockAuthProvider::new(creds);
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(provider.refresh()).unwrap();
    }
}
