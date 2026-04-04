#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("no API key found: {hint}")]
    NoApiKey { hint: String },

    #[error("keychain error: {message}")]
    Keychain { message: String },

    #[error("auth error: {message}")]
    Auth { message: String },

    #[error(transparent)]
    Common(#[from] crab_common::Error),
}

impl From<AuthError> for crab_common::Error {
    fn from(err: AuthError) -> Self {
        match err {
            AuthError::NoApiKey { hint } => Self::Auth(hint),
            AuthError::Keychain { message } | AuthError::Auth { message } => Self::Auth(message),
            AuthError::Common(e) => e,
        }
    }
}
