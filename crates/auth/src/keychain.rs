use crate::error::AuthError;

/// Default service name for keychain entries.
const SERVICE: &str = "crab-code";

/// Retrieve a password from the system keychain.
///
/// # Errors
///
/// Returns `AuthError::Keychain` if the entry cannot be accessed or does not exist.
pub fn get(service: &str, key: &str) -> Result<String, AuthError> {
    let entry = keyring::Entry::new(service, key).map_err(|e| AuthError::Keychain {
        message: format!("keychain init failed: {e}"),
    })?;
    entry.get_password().map_err(|e| AuthError::Keychain {
        message: format!("keychain read failed: {e}"),
    })
}

/// Store a password in the system keychain.
///
/// # Errors
///
/// Returns `AuthError::Keychain` if the entry cannot be written.
pub fn set(service: &str, key: &str, value: &str) -> Result<(), AuthError> {
    let entry = keyring::Entry::new(service, key).map_err(|e| AuthError::Keychain {
        message: format!("keychain init failed: {e}"),
    })?;
    entry.set_password(value).map_err(|e| AuthError::Keychain {
        message: format!("keychain write failed: {e}"),
    })
}

/// Delete a password from the system keychain.
///
/// # Errors
///
/// Returns `AuthError::Keychain` if the entry cannot be deleted.
pub fn delete(service: &str, key: &str) -> Result<(), AuthError> {
    let entry = keyring::Entry::new(service, key).map_err(|e| AuthError::Keychain {
        message: format!("keychain init failed: {e}"),
    })?;
    entry.delete_credential().map_err(|e| AuthError::Keychain {
        message: format!("keychain delete failed: {e}"),
    })
}

/// Get an API key from the default keychain location.
pub fn get_api_key() -> Result<String, AuthError> {
    get(SERVICE, "api-key")
}

/// Store an API key in the default keychain location.
pub fn set_api_key(value: &str) -> Result<(), AuthError> {
    set(SERVICE, "api-key", value)
}

/// Delete the API key from the default keychain location.
pub fn delete_api_key() -> Result<(), AuthError> {
    delete(SERVICE, "api-key")
}
