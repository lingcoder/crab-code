//! Credential chain — automatic multi-strategy authentication.
//!
//! Tries multiple auth methods in priority order, returning the first
//! that succeeds. The chain also caches the winning provider for
//! subsequent calls, re-probing only on refresh.
//!
//! Default resolution order:
//! 1. Environment variable API key (`CRAB_API_KEY` / `ANTHROPIC_API_KEY` / etc.)
//! 2. Credential file (shared credentials / config file)
//! 3. Instance metadata (GCE, ECS, IMDS)
//! 4. IAM role assumption / workload identity

use std::future::Future;
use std::pin::Pin;

use crate::{AuthMethod, AuthProvider};

/// A named provider in the chain.
struct ChainEntry {
    name: &'static str,
    provider: Box<dyn AuthProvider>,
}

/// Auth provider that tries multiple strategies in order.
///
/// On the first successful `get_auth()`, the winning provider is cached.
/// Calling `refresh()` clears the cache, causing the next `get_auth()`
/// to re-probe the full chain.
pub struct CredentialChain {
    providers: Vec<ChainEntry>,
    /// Index of the last successful provider (cached).
    cached_index: tokio::sync::Mutex<Option<usize>>,
}

impl CredentialChain {
    /// Create an empty chain. Use [`CredentialChainBuilder`] for ergonomic construction.
    #[must_use]
    pub fn new(providers: Vec<(&'static str, Box<dyn AuthProvider>)>) -> Self {
        Self {
            providers: providers
                .into_iter()
                .map(|(name, provider)| ChainEntry { name, provider })
                .collect(),
            cached_index: tokio::sync::Mutex::new(None),
        }
    }

    /// Number of providers in the chain.
    #[must_use]
    pub fn len(&self) -> usize {
        self.providers.len()
    }

    /// Whether the chain is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    /// Names of the providers in the chain (for diagnostics).
    pub fn provider_names(&self) -> Vec<&'static str> {
        self.providers.iter().map(|e| e.name).collect()
    }
}

impl AuthProvider for CredentialChain {
    fn get_auth(
        &self,
    ) -> Pin<Box<dyn Future<Output = crab_common::Result<AuthMethod>> + Send + '_>> {
        Box::pin(async move {
            // Try cached provider first
            {
                let guard = self.cached_index.lock().await;
                if let Some(idx) = *guard
                    && let Some(entry) = self.providers.get(idx)
                    && let Ok(auth) = entry.provider.get_auth().await
                {
                    return Ok(auth);
                }
                // Cached provider failed — fall through to full chain
            }

            // Probe all providers in order
            let mut last_error = None;
            for (idx, entry) in self.providers.iter().enumerate() {
                match entry.provider.get_auth().await {
                    Ok(auth) => {
                        // Cache the winning provider
                        {
                            let mut guard = self.cached_index.lock().await;
                            *guard = Some(idx);
                        }
                        return Ok(auth);
                    }
                    Err(e) => {
                        last_error = Some(e);
                    }
                }
            }

            Err(last_error
                .unwrap_or_else(|| crab_common::Error::Auth("credential chain is empty".into())))
        })
    }

    fn refresh(&self) -> Pin<Box<dyn Future<Output = crab_common::Result<()>> + Send + '_>> {
        Box::pin(async move {
            // Clear cached index so next get_auth re-probes
            let mut guard = self.cached_index.lock().await;
            *guard = None;
            drop(guard);

            // Refresh all providers
            for entry in &self.providers {
                let _ = entry.provider.refresh().await;
            }
            Ok(())
        })
    }
}

/// Builder for constructing a credential chain.
pub struct CredentialChainBuilder {
    providers: Vec<(&'static str, Box<dyn AuthProvider>)>,
}

impl CredentialChainBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
        }
    }

    /// Add a provider to the chain.
    #[must_use]
    pub fn with(mut self, name: &'static str, provider: Box<dyn AuthProvider>) -> Self {
        self.providers.push((name, provider));
        self
    }

    /// Add a provider only if the condition is true.
    #[must_use]
    pub fn with_if(
        self,
        condition: bool,
        name: &'static str,
        provider_fn: impl FnOnce() -> Box<dyn AuthProvider>,
    ) -> Self {
        if condition {
            self.with(name, provider_fn())
        } else {
            self
        }
    }

    /// Add a provider if `Option` is `Some`.
    #[must_use]
    pub fn with_optional(
        self,
        name: &'static str,
        provider: Option<Box<dyn AuthProvider>>,
    ) -> Self {
        match provider {
            Some(p) => self.with(name, p),
            None => self,
        }
    }

    /// Build the credential chain.
    #[must_use]
    pub fn build(self) -> CredentialChain {
        CredentialChain::new(self.providers)
    }
}

impl Default for CredentialChainBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Build the default credential chain for a given provider.
///
/// Resolves a credential via the out-of-chain auth pipeline
/// (`resolve_auth_key`: env → `apiKeyHelper` → keychain → tokens.json) and,
/// when present, registers it as a single-source provider. Cloud-specific
/// providers (Bedrock `SigV4` / Vertex `OAuth2`) are intended to layer on
/// top by extending the returned chain.
#[must_use]
pub fn build_default_chain(settings: &crab_config::Config) -> CredentialChain {
    let mut builder = CredentialChainBuilder::new();

    if let Some(key) = crate::resolver::resolve_auth_key(settings)
        && !key.is_empty()
    {
        builder = builder.with("resolved-key", Box::new(crate::ApiKeyProvider::new(key)));
    }

    builder.build()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A test provider that always succeeds.
    struct SuccessProvider(String);

    impl AuthProvider for SuccessProvider {
        fn get_auth(
            &self,
        ) -> Pin<Box<dyn Future<Output = crab_common::Result<AuthMethod>> + Send + '_>> {
            let key = self.0.clone();
            Box::pin(async move { Ok(AuthMethod::ApiKey(key)) })
        }

        fn refresh(&self) -> Pin<Box<dyn Future<Output = crab_common::Result<()>> + Send + '_>> {
            Box::pin(async { Ok(()) })
        }
    }

    /// A test provider that always fails.
    struct FailProvider;

    impl AuthProvider for FailProvider {
        fn get_auth(
            &self,
        ) -> Pin<Box<dyn Future<Output = crab_common::Result<AuthMethod>> + Send + '_>> {
            Box::pin(async { Err(crab_common::Error::Auth("fail".into())) })
        }

        fn refresh(&self) -> Pin<Box<dyn Future<Output = crab_common::Result<()>> + Send + '_>> {
            Box::pin(async { Ok(()) })
        }
    }

    #[test]
    fn empty_chain_errors() {
        let chain = CredentialChain::new(vec![]);
        assert!(chain.is_empty());
        assert_eq!(chain.len(), 0);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(chain.get_auth());
        assert!(result.is_err());
    }

    #[test]
    fn single_success_provider() {
        let chain = CredentialChain::new(vec![("test", Box::new(SuccessProvider("key-1".into())))]);
        assert_eq!(chain.len(), 1);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(chain.get_auth()).unwrap();
        match result {
            AuthMethod::ApiKey(k) => assert_eq!(k, "key-1"),
            AuthMethod::OAuth(_) => panic!("expected ApiKey"),
        }
    }

    #[test]
    fn falls_through_to_second_provider() {
        let chain = CredentialChain::new(vec![
            ("fail", Box::new(FailProvider)),
            ("success", Box::new(SuccessProvider("key-2".into()))),
        ]);
        assert_eq!(chain.len(), 2);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(chain.get_auth()).unwrap();
        match result {
            AuthMethod::ApiKey(k) => assert_eq!(k, "key-2"),
            AuthMethod::OAuth(_) => panic!("expected ApiKey"),
        }
    }

    #[test]
    fn all_fail_returns_last_error() {
        let chain = CredentialChain::new(vec![
            ("fail1", Box::new(FailProvider)),
            ("fail2", Box::new(FailProvider)),
        ]);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(chain.get_auth());
        assert!(result.is_err());
    }

    #[test]
    fn caches_winning_provider() {
        let chain = CredentialChain::new(vec![
            ("fail", Box::new(FailProvider)),
            ("success", Box::new(SuccessProvider("cached".into()))),
        ]);

        let rt = tokio::runtime::Runtime::new().unwrap();

        // First call probes chain
        let _ = rt.block_on(chain.get_auth()).unwrap();

        // Second call should use cached index (provider 1)
        let result = rt.block_on(chain.get_auth()).unwrap();
        match result {
            AuthMethod::ApiKey(k) => assert_eq!(k, "cached"),
            AuthMethod::OAuth(_) => panic!("expected ApiKey"),
        }
    }

    #[test]
    fn refresh_clears_cache() {
        let chain = CredentialChain::new(vec![("test", Box::new(SuccessProvider("key".into())))]);

        let rt = tokio::runtime::Runtime::new().unwrap();

        // Populate cache
        let _ = rt.block_on(chain.get_auth()).unwrap();

        // Refresh clears it
        rt.block_on(chain.refresh()).unwrap();

        // Should still work (re-probes)
        let result = rt.block_on(chain.get_auth()).unwrap();
        assert!(matches!(result, AuthMethod::ApiKey(_)));
    }

    #[test]
    fn provider_names() {
        let chain = CredentialChain::new(vec![
            ("env-key", Box::new(SuccessProvider("a".into()))),
            ("keychain", Box::new(SuccessProvider("b".into()))),
        ]);
        assert_eq!(chain.provider_names(), vec!["env-key", "keychain"]);
    }

    #[test]
    fn builder_basic() {
        let chain = CredentialChainBuilder::new()
            .with("a", Box::new(SuccessProvider("1".into())))
            .with("b", Box::new(SuccessProvider("2".into())))
            .build();
        assert_eq!(chain.len(), 2);
    }

    #[test]
    fn builder_with_if_true() {
        let chain = CredentialChainBuilder::new()
            .with_if(true, "cond", || Box::new(SuccessProvider("yes".into())))
            .build();
        assert_eq!(chain.len(), 1);
    }

    #[test]
    fn builder_with_if_false() {
        let chain = CredentialChainBuilder::new()
            .with_if(false, "cond", || Box::new(SuccessProvider("no".into())))
            .build();
        assert_eq!(chain.len(), 0);
    }

    #[test]
    fn builder_with_optional_some() {
        let chain = CredentialChainBuilder::new()
            .with_optional("opt", Some(Box::new(SuccessProvider("s".into()))))
            .build();
        assert_eq!(chain.len(), 1);
    }

    #[test]
    fn builder_with_optional_none() {
        let chain = CredentialChainBuilder::new()
            .with_optional("opt", None)
            .build();
        assert_eq!(chain.len(), 0);
    }

    #[test]
    fn builder_default() {
        let builder = CredentialChainBuilder::default();
        let chain = builder.build();
        assert!(chain.is_empty());
    }

    #[test]
    fn build_default_chain_no_settings() {
        let settings = crab_config::Config::default();
        let chain = build_default_chain(&settings);
        // May or may not have providers depending on env/keychain/tokens.json
        let _ = chain.len();
    }

    #[test]
    fn first_provider_wins() {
        let chain = CredentialChain::new(vec![
            ("first", Box::new(SuccessProvider("winner".into()))),
            ("second", Box::new(SuccessProvider("loser".into()))),
        ]);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(chain.get_auth()).unwrap();
        match result {
            AuthMethod::ApiKey(k) => assert_eq!(k, "winner"),
            AuthMethod::OAuth(_) => panic!("expected ApiKey"),
        }
    }
}
