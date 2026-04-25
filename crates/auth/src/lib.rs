pub mod aws_iam;
#[cfg(feature = "bedrock")]
pub mod bedrock_auth;
pub mod credential_chain;
pub mod error;
pub mod gcp_identity;
pub mod keychain;
pub mod oauth;
pub mod resolver;
#[cfg(feature = "vertex")]
pub mod vertex_auth;

pub use credential_chain::{CredentialChain, CredentialChainBuilder, build_default_chain};
pub use error::AuthError;
pub use resolver::resolve_auth_key;

use std::future::Future;
use std::pin::Pin;

/// Authentication method resolved for an API request.
#[derive(Debug)]
pub enum AuthMethod {
    /// Raw API key (Anthropic `x-api-key` or `OpenAI` `Authorization: Bearer`).
    ApiKey(String),
    /// `OAuth2` access token.
    OAuth(OAuthToken),
}

/// `OAuth2` token container.
#[derive(Debug, Clone)]
pub struct OAuthToken {
    pub access_token: String,
}

/// Provides authentication credentials for LLM API calls.
///
/// Implementations must be `Send + Sync` for use across async tasks.
/// Methods return `Pin<Box<dyn Future>>` for object safety (`Box<dyn AuthProvider>`).
pub trait AuthProvider: Send + Sync {
    /// Resolve the current auth credentials.
    fn get_auth(
        &self,
    ) -> Pin<Box<dyn Future<Output = crab_common::Result<AuthMethod>> + Send + '_>>;

    /// Refresh credentials (no-op for API key providers).
    fn refresh(&self) -> Pin<Box<dyn Future<Output = crab_common::Result<()>> + Send + '_>>;
}

/// API key auth provider — resolves key once at construction, returns it on each call.
pub struct ApiKeyProvider {
    key: String,
}

impl ApiKeyProvider {
    #[must_use]
    pub fn new(key: String) -> Self {
        Self { key }
    }
}

impl AuthProvider for ApiKeyProvider {
    fn get_auth(
        &self,
    ) -> Pin<Box<dyn Future<Output = crab_common::Result<AuthMethod>> + Send + '_>> {
        let key = self.key.clone();
        Box::pin(async move { Ok(AuthMethod::ApiKey(key)) })
    }

    fn refresh(&self) -> Pin<Box<dyn Future<Output = crab_common::Result<()>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }
}

/// Create an auth provider from application settings.
///
/// Resolves the credential via the out-of-chain auth pipeline
/// (`resolve_auth_key`): env vars → `apiKeyHelper` script → keychain →
/// OAuth tokens. Secrets never round-trip through `Config`.
///
/// Falls back to an empty key if resolution fails (the API call will error with 401).
#[must_use]
pub fn create_auth_provider(settings: &crab_config::Config) -> Box<dyn AuthProvider> {
    let key = resolve_auth_key(settings).unwrap_or_default();
    Box::new(ApiKeyProvider::new(key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_method_api_key() {
        let method = AuthMethod::ApiKey("sk-test".into());
        match method {
            AuthMethod::ApiKey(k) => assert_eq!(k, "sk-test"),
            AuthMethod::OAuth(_) => panic!("expected ApiKey"),
        }
    }

    #[test]
    fn auth_method_oauth() {
        let method = AuthMethod::OAuth(OAuthToken {
            access_token: "token-123".into(),
        });
        match method {
            AuthMethod::OAuth(t) => assert_eq!(t.access_token, "token-123"),
            AuthMethod::ApiKey(_) => panic!("expected OAuth"),
        }
    }

    #[test]
    fn api_key_provider_returns_key() {
        let provider = ApiKeyProvider::new("my-key".into());
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(provider.get_auth()).unwrap();
        match result {
            AuthMethod::ApiKey(k) => assert_eq!(k, "my-key"),
            AuthMethod::OAuth(_) => panic!("expected ApiKey"),
        }
    }

    #[test]
    fn api_key_provider_refresh_is_noop() {
        let provider = ApiKeyProvider::new("key".into());
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(provider.refresh()).unwrap();
    }

    #[test]
    fn auth_provider_is_object_safe() {
        let provider: Box<dyn AuthProvider> = Box::new(ApiKeyProvider::new("test".into()));
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(provider.get_auth()).unwrap();
        match result {
            AuthMethod::ApiKey(k) => assert_eq!(k, "test"),
            AuthMethod::OAuth(_) => panic!("expected ApiKey"),
        }
    }

    #[test]
    fn auth_provider_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ApiKeyProvider>();
    }

    #[test]
    fn create_auth_provider_with_default_settings() {
        // Default settings have no api_key_helper; the provider resolves to
        // whatever env/keychain/tokens.json yields (or empty fallback).
        let settings = crab_config::Config::default();
        let provider = create_auth_provider(&settings);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(provider.get_auth()).unwrap();
        assert!(matches!(result, AuthMethod::ApiKey(_)));
    }
}
