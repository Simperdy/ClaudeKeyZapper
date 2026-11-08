use crate::errors::{Error, Result};
use crate::models::OidcTokenType;
use crate::paths;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// SSO tokens of one profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenSet {
    pub refresh_token: String,
    /// Bearer token for the gateway: the access token, or the ID token for `OidcTokenType::Id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,
    /// Unix seconds at which that token expires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    /// Kind of `access_token`; a profile whose token type changed never gets the old kind.
    #[serde(default, skip_serializing_if = "OidcTokenType::is_access")]
    pub token_type: OidcTokenType,
    /// Whether `expires_at` came from a positive `expires_in` or a future JWT `exp` claim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expiry_verified: Option<bool>,
}

/// SSO tokens by profile ID in `sso-tokens.json` next to `keys.json`, same protection (owner-only file).
/// Never part of `keys.json` and never exported into backups.
#[derive(Clone, Debug)]
pub struct TokenStore {
    pub file: PathBuf,
}

impl Default for TokenStore {
    fn default() -> Self {
        Self { file: paths::data_dir().join("sso-tokens.json") }
    }
}

impl TokenStore {
    pub fn all(&self) -> Result<BTreeMap<String, TokenSet>> {
        match std::fs::read(&self.file) {
            Ok(data) => serde_json::from_slice(&data).map_err(|e| Error::KeyStore(e.to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(e) => Err(Error::KeyStore(e.to_string())),
        }
    }

    pub fn read(&self, id: &str) -> Result<Option<TokenSet>> {
        Ok(self.all()?.remove(id))
    }

    pub fn exists(&self, id: &str) -> Result<bool> {
        Ok(self.all()?.get(id).is_some_and(|t| !t.refresh_token.is_empty()))
    }

    pub fn write(&self, id: &str, tokens: &TokenSet) -> Result<()> {
        let mut all = self.all()?;
        all.insert(id.to_string(), tokens.clone());
        self.save(&all)
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        let mut all = self.all()?;
        if all.remove(id).is_some() {
            self.save(&all)?;
        }
        Ok(())
    }

    /// Lock file serialising refreshes across processes (helper instances and the app).
    pub fn lock_file(&self) -> PathBuf {
        self.file.with_extension("lock")
    }

    fn save(&self, tokens: &BTreeMap<String, TokenSet>) -> Result<()> {
        if let Some(dir) = self.file.parent() {
            paths::create_private_dir(dir).map_err(|e| Error::KeyStore(e.to_string()))?;
        }
        let text = serde_json::to_string_pretty(tokens).map_err(|e| Error::KeyStore(e.to_string()))?;
        paths::write_atomic(&self.file, text.as_bytes(), true).map_err(|e| Error::KeyStore(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::TokenSet;

    #[test]
    fn missing_expiry_provenance_defaults_to_unverified_and_is_omitted() {
        let mut legacy = serde_json::from_str::<TokenSet>(r#"{"refreshToken":"R","accessToken":"A","expiresAt":1000}"#).unwrap();
        assert_eq!(legacy.expiry_verified, None);
        assert!(serde_json::to_value(&legacy).unwrap().get("expiryVerified").is_none());
        legacy.expiry_verified = Some(false);
        assert_eq!(serde_json::to_value(&legacy).unwrap()["expiryVerified"], false);
    }
}
