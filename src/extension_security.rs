//! Signing helpers for trusted web-dashboard bundles.

use crate::{Error, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use std::env;

pub const EXTENSION_SIGNING_KEY_ENV: &str = "TABKIT_EXTENSION_SIGNING_KEY";
pub const EXTENSION_SIGNING_KEY_BYTES: usize = 32;

type HmacSha256 = Hmac<Sha256>;

pub fn signing_key_from_env(name: &str) -> Result<Option<Vec<u8>>> {
    let value = match env::var(name) {
        Ok(value) if !value.trim().is_empty() => value,
        Ok(_) | Err(env::VarError::NotPresent) => return Ok(None),
        Err(error) => return Err(Error::new("CONFIG", error.to_string())),
    };
    let key = BASE64.decode(value.trim()).map_err(|_| {
        Error::new(
            "CONFIG",
            format!("{name} must contain base64 for exactly {EXTENSION_SIGNING_KEY_BYTES} bytes"),
        )
    })?;
    if key.len() != EXTENSION_SIGNING_KEY_BYTES {
        return Err(Error::new(
            "CONFIG",
            format!("{name} must decode to exactly {EXTENSION_SIGNING_KEY_BYTES} bytes"),
        ));
    }
    Ok(Some(key))
}

pub fn sign_bundle(bundle: &[u8], key: &[u8]) -> Result<String> {
    let mut mac = HmacSha256::new_from_slice(key)
        .map_err(|_| Error::new("CONFIG", "Invalid extension signing key"))?;
    mac.update(bundle);
    Ok(BASE64.encode(mac.finalize().into_bytes()))
}

pub fn verify_bundle(bundle: &[u8], signature_b64: &str, key: &[u8]) -> Result<bool> {
    let signature = match BASE64.decode(signature_b64) {
        Ok(value) => value,
        Err(_) => return Ok(false),
    };
    let mut mac = HmacSha256::new_from_slice(key)
        .map_err(|_| Error::new("CONFIG", "Invalid extension signing key"))?;
    mac.update(bundle);
    Ok(mac.verify_slice(&signature).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signs_and_verifies_bundle() -> Result<()> {
        let key = [7_u8; EXTENSION_SIGNING_KEY_BYTES];
        let bundle = br#"{"runtime_version":1}"#;
        let signature = sign_bundle(bundle, &key)?;
        assert!(verify_bundle(bundle, &signature, &key)?);
        assert!(!verify_bundle(b"changed", &signature, &key)?);
        Ok(())
    }
}
