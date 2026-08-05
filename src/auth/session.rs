// Copyright (c) The Social Proof Foundation, LLC.
// SPDX-License-Identifier: Apache-2.0

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::http::HeaderMap;
use jsonwebtoken::{decode, decode_header, jwk::JwkSet, DecodingKey, Validation};
use serde::Deserialize;
use tokio::sync::RwLock;

use crate::error::ServiceError;

/// Default salt-service issuer used by DripDrop / MySocial session JWTs (EdDSA).
const DEFAULT_SALT_ISSUER: &str = "https://salt.testnet.mysocial.network";
const DEFAULT_SALT_JWKS_URI: &str =
    "https://salt.testnet.mysocial.network/.well-known/jwks.json";

#[derive(Debug, Clone, Deserialize)]
pub struct SessionClaims {
    pub sub: String,
    #[serde(default)]
    pub address: Option<String>,
    /// Salt-service session JWTs use `wallet_address` (not `address`).
    #[serde(default)]
    pub wallet_address: Option<String>,
    pub iss: Option<String>,
    pub exp: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct AuthenticatedSession {
    pub wallet_address: String,
    pub subject: String,
}

pub struct SessionValidator {
    client: reqwest::Client,
    hs256_key: Option<Vec<u8>>,
    jwks_uris: HashMap<String, String>,
    jwks_cache: Arc<RwLock<HashMap<String, (JwkSet, std::time::Instant)>>>,
}

impl SessionValidator {
    pub fn new(
        jwt_signing_key: Option<String>,
        mysocial_auth_issuer: Option<String>,
        mysocial_auth_jwks_uri: Option<String>,
    ) -> Self {
        let hs256_key = jwt_signing_key.and_then(|k| {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD
                .decode(k.trim())
                .ok()
                .filter(|b| b.len() >= 32)
        });

        let mut jwks_uris = HashMap::new();
        if let (Some(iss), Some(jwks)) = (mysocial_auth_issuer, mysocial_auth_jwks_uri) {
            if !iss.is_empty() && !jwks.is_empty() {
                jwks_uris.insert(iss, jwks);
            }
        }

        // Ecosystem apps (DripDrop iOS) authenticate with salt-service EdDSA JWTs.
        jwks_uris
            .entry(DEFAULT_SALT_ISSUER.to_string())
            .or_insert_with(|| DEFAULT_SALT_JWKS_URI.to_string());

        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .expect("http client"),
            hs256_key,
            jwks_uris,
            jwks_cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn validate_headers(
        &self,
        headers: &HeaderMap,
    ) -> Result<AuthenticatedSession, ServiceError> {
        let token = extract_bearer(headers).ok_or_else(|| {
            ServiceError::unauthorized("missing Authorization bearer token")
        })?;
        self.validate_token(&token).await
    }

    pub async fn validate_token(&self, token: &str) -> Result<AuthenticatedSession, ServiceError> {
        if let Some(key) = &self.hs256_key {
            let mut validation = Validation::new(jsonwebtoken::Algorithm::HS256);
            validation.validate_exp = true;
            validation.validate_aud = false;
            if let Ok(data) = decode::<SessionClaims>(
                token,
                &DecodingKey::from_secret(key),
                &validation,
            ) {
                return claims_to_session(data.claims);
            }
        }

        let header = decode_header(token)
            .map_err(|e| ServiceError::unauthorized(format!("invalid token header: {e}")))?;
        let kid = header
            .kid
            .ok_or_else(|| ServiceError::unauthorized("token missing kid"))?;

        let iss = peek_iss(token)?;
        let jwks_uri = self
            .jwks_uris
            .get(&iss)
            .ok_or_else(|| ServiceError::unauthorized(format!("unknown issuer {iss}")))?;

        let jwk_set = self.fetch_jwks(jwks_uri).await?;
        let jwk = jwk_set
            .find(&kid)
            .ok_or_else(|| ServiceError::unauthorized("jwk not found"))?;
        let decoding_key = DecodingKey::from_jwk(jwk)
            .map_err(|e| ServiceError::unauthorized(format!("jwk decode error: {e}")))?;

        // Salt issues EdDSA; auth backends may use RS256 — follow the JWT header.
        let mut validation = Validation::new(header.alg);
        validation.validate_exp = true;
        validation.validate_aud = false;
        validation.set_issuer(&[iss.as_str()]);

        let data = decode::<SessionClaims>(token, &decoding_key, &validation)
            .map_err(|e| ServiceError::unauthorized(format!("token validation failed: {e}")))?;

        claims_to_session(data.claims)
    }

    async fn fetch_jwks(&self, uri: &str) -> Result<JwkSet, ServiceError> {
        {
            let cache = self.jwks_cache.read().await;
            if let Some((set, fetched)) = cache.get(uri) {
                if fetched.elapsed() < Duration::from_secs(3600) {
                    return Ok(set.clone());
                }
            }
        }

        let resp = self
            .client
            .get(uri)
            .send()
            .await
            .map_err(|e| ServiceError::Upstream(format!("jwks fetch failed: {e}")))?
            .error_for_status()
            .map_err(|e| ServiceError::Upstream(format!("jwks fetch status: {e}")))?
            .json::<JwkSet>()
            .await
            .map_err(|e| ServiceError::Upstream(format!("jwks parse failed: {e}")))?;

        self.jwks_cache
            .write()
            .await
            .insert(uri.to_string(), (resp.clone(), std::time::Instant::now()));

        Ok(resp)
    }

    #[cfg(test)]
    async fn insert_jwks_cache_for_test(&self, uri: &str, set: JwkSet) {
        self.jwks_cache
            .write()
            .await
            .insert(uri.to_string(), (set, std::time::Instant::now()));
    }
}

fn extract_bearer(headers: &HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::to_string)
}

fn peek_iss(token: &str) -> Result<String, ServiceError> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(ServiceError::unauthorized("invalid jwt format"));
    }
    use base64::Engine;
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts[1])
        .map_err(|e| ServiceError::unauthorized(format!("jwt payload decode: {e}")))?;
    let v: serde_json::Value = serde_json::from_slice(&payload)
        .map_err(|e| ServiceError::unauthorized(format!("jwt payload json: {e}")))?;
    v.get("iss")
        .and_then(|i| i.as_str())
        .map(str::to_string)
        .ok_or_else(|| ServiceError::unauthorized("jwt missing iss"))
}

fn claims_to_session(claims: SessionClaims) -> Result<AuthenticatedSession, ServiceError> {
    let wallet = claims
        .wallet_address
        .filter(|s| !s.trim().is_empty())
        .or_else(|| claims.address.filter(|s| !s.trim().is_empty()))
        .or_else(|| claims.sub.strip_prefix("wallet:").map(str::to_string))
        .ok_or_else(|| ServiceError::unauthorized("token missing wallet address"))?;
    Ok(AuthenticatedSession {
        wallet_address: wallet.to_lowercase(),
        subject: claims.sub,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose, Engine as _};
    use ed25519_dalek::{Signer, SigningKey};

    fn mint_salt_style_token(seed: &[u8; 32], issuer: &str, wallet: &str, kid: &str) -> (String, JwkSet) {
        let signing_key = SigningKey::from_bytes(seed);
        let verifying = signing_key.verifying_key();
        let now = chrono::Utc::now().timestamp();
        let header = serde_json::json!({
            "alg": "EdDSA",
            "typ": "JWT",
            "kid": kid,
        });
        let claims = serde_json::json!({
            "iss": issuer,
            "aud": "dripdrop",
            "sub": "https://accounts.google.com:test-user",
            "wallet_address": wallet,
            "provider": "google",
            "iat": now,
            "exp": now + 1800,
            "jti": "test-jti",
        });
        let header_b64 = general_purpose::URL_SAFE_NO_PAD.encode(header.to_string());
        let claims_b64 = general_purpose::URL_SAFE_NO_PAD.encode(claims.to_string());
        let signing_input = format!("{header_b64}.{claims_b64}");
        let sig = signing_key.sign(signing_input.as_bytes());
        let token = format!(
            "{signing_input}.{}",
            general_purpose::URL_SAFE_NO_PAD.encode(sig.to_bytes())
        );

        let jwks: JwkSet = serde_json::from_value(serde_json::json!({
            "keys": [{
                "kty": "OKP",
                "crv": "Ed25519",
                "use": "sig",
                "alg": "EdDSA",
                "kid": kid,
                "x": general_purpose::URL_SAFE_NO_PAD.encode(verifying.as_bytes()),
            }]
        }))
        .expect("jwks");

        (token, jwks)
    }

    #[test]
    fn claims_prefer_wallet_address() {
        let session = claims_to_session(SessionClaims {
            sub: "https://accounts.google.com:user".into(),
            address: Some("0xaaaa".into()),
            wallet_address: Some("0xBbBb".into()),
            iss: Some(DEFAULT_SALT_ISSUER.into()),
            exp: None,
        })
        .unwrap();
        assert_eq!(session.wallet_address, "0xbbbb");
    }

    #[tokio::test]
    async fn validates_salt_eddsa_jwt() {
        let issuer = DEFAULT_SALT_ISSUER;
        let kid = "mysocial-salt";
        let wallet = "0xABC123";
        let (token, jwks) = mint_salt_style_token(&[9u8; 32], issuer, wallet, kid);

        let validator = SessionValidator::new(None, None, None);
        // Default salt issuer maps to DEFAULT_SALT_JWKS_URI — prime cache (no network).
        validator
            .insert_jwks_cache_for_test(DEFAULT_SALT_JWKS_URI, jwks)
            .await;

        let session = validator.validate_token(&token).await.expect("validate");
        assert_eq!(session.wallet_address, wallet.to_lowercase());
        assert_eq!(session.subject, "https://accounts.google.com:test-user");
    }
}
