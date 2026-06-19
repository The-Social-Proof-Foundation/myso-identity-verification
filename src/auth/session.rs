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

#[derive(Debug, Clone, Deserialize)]
pub struct SessionClaims {
    pub sub: String,
    #[serde(default)]
    pub address: Option<String>,
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

    pub async fn validate_headers(&self, headers: &HeaderMap) -> Result<AuthenticatedSession, ServiceError> {
        let token = extract_bearer(headers).ok_or_else(|| {
            ServiceError::unauthorized("missing Authorization bearer token")
        })?;
        self.validate_token(&token).await
    }

    pub async fn validate_token(&self, token: &str) -> Result<AuthenticatedSession, ServiceError> {
        if let Some(key) = &self.hs256_key {
            let mut validation = Validation::new(jsonwebtoken::Algorithm::HS256);
            validation.validate_exp = true;
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

        let mut validation = Validation::new(jsonwebtoken::Algorithm::RS256);
        validation.validate_exp = true;
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
        .address
        .or_else(|| claims.sub.strip_prefix("wallet:").map(str::to_string))
        .ok_or_else(|| ServiceError::unauthorized("token missing wallet address"))?;
    Ok(AuthenticatedSession {
        wallet_address: wallet.to_lowercase(),
        subject: claims.sub,
    })
}
