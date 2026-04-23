//! macOS Keychain integration for securely storing the AWS API key.
//!
//! Uses the Security.framework via the `security-framework` crate to store
//! and retrieve the bearer token in the user's login keychain.

use security_framework::passwords::{
    delete_generic_password, get_generic_password, set_generic_password,
};

const SERVICE: &str = "com.marmaro.aws";
const ACCOUNT: &str = "api_key";

/// Retrieve the stored API key from the macOS Keychain.
/// Returns None if no key is stored or on error.
pub fn get_api_key() -> Option<String> {
    match get_generic_password(SERVICE, ACCOUNT) {
        Ok(bytes) => String::from_utf8(bytes.to_vec()).ok(),
        Err(_) => None,
    }
}

/// Store the API key in the macOS Keychain.
/// Overwrites any existing value.
pub fn set_api_key(key: &str) -> Result<(), String> {
    // Try to set directly first; if it already exists, delete and retry
    match set_generic_password(SERVICE, ACCOUNT, key.as_bytes()) {
        Ok(()) => Ok(()),
        Err(_) => {
            let _ = delete_generic_password(SERVICE, ACCOUNT);
            set_generic_password(SERVICE, ACCOUNT, key.as_bytes())
                .map_err(|e| format!("Keychain write failed: {}", e))
        }
    }
}

/// Delete the stored API key from the macOS Keychain.
#[allow(dead_code)]
pub fn delete_api_key() -> Result<(), String> {
    delete_generic_password(SERVICE, ACCOUNT).map_err(|e| format!("Keychain delete failed: {}", e))
}
