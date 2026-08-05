// Copyright (c) The Social Proof Foundation, LLC.
// SPDX-License-Identifier: Apache-2.0

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use chrono::{DateTime, Duration, Utc};
use rand::RngCore;
use redis::aio::ConnectionManager;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::ServiceError;
use crate::state::AppState;

const X_TOKEN_PREFIX: &str = "x_tokens:";
const TOKEN_REFRESH_BUFFER_SECS: i64 = 60;
const DEFAULT_ACCESS_TOKEN_TTL_SECS: i64 = 7200;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XTokenRecord {
    pub access_token: String,
    pub refresh_token_encrypted: String,
    pub expires_at: DateTime<Utc>,
    pub x_user_id: String,
    pub x_username: String,
}

pub struct XTokenStore;

impl XTokenStore {
    pub fn redis_key(wallet_address: &str) -> String {
        format!("{X_TOKEN_PREFIX}{}", wallet_address.to_lowercase())
    }

    pub async fn save(
        redis: &mut ConnectionManager,
        wallet_address: &str,
        encryption_secret: &str,
        access_token: &str,
        refresh_token: &str,
        expires_in_secs: Option<u64>,
        x_user_id: &str,
        x_username: &str,
    ) -> Result<(), ServiceError> {
        let ttl = expires_in_secs.unwrap_or(DEFAULT_ACCESS_TOKEN_TTL_SECS as u64) as i64;
        let record = XTokenRecord {
            access_token: access_token.to_string(),
            refresh_token_encrypted: encrypt(refresh_token, encryption_secret)?,
            expires_at: Utc::now() + Duration::seconds(ttl),
            x_user_id: x_user_id.to_string(),
            x_username: x_username.to_string(),
        };

        let payload = serde_json::to_string(&record)
            .map_err(|e| ServiceError::Internal(e.into()))?;
        redis
            .set::<_, _, ()>(Self::redis_key(wallet_address), payload)
            .await
            .map_err(|e| ServiceError::Upstream(format!("redis set x token failed: {e}")))?;
        Ok(())
    }

    pub async fn get(
        redis: &mut ConnectionManager,
        wallet_address: &str,
    ) -> Result<Option<XTokenRecord>, ServiceError> {
        let raw: Option<String> = redis
            .get(Self::redis_key(wallet_address))
            .await
            .map_err(|e| ServiceError::Upstream(format!("redis get x token failed: {e}")))?;
        raw.map(|s| {
            serde_json::from_str(&s)
                .map_err(|e| ServiceError::Upstream(format!("x token parse: {e}")))
        })
        .transpose()
    }

    pub async fn get_valid_access_token(
        state: &AppState,
        wallet_address: &str,
    ) -> Result<String, ServiceError> {
        let mut redis = state.redis.clone();
        let Some(record) = Self::get(&mut redis, wallet_address).await? else {
            return Err(ServiceError::bad_request(
                "X account not connected — complete OAuth at /oauth/x/connect first",
            ));
        };

        let needs_refresh = Utc::now() + Duration::seconds(TOKEN_REFRESH_BUFFER_SECS)
            >= record.expires_at;

        if !needs_refresh {
            return Ok(record.access_token);
        }

        let refresh_token =
            decrypt(&record.refresh_token_encrypted, &state.config.oauth_state_secret)?;

        let token_resp = state
            .x_oauth
            .refresh_access_token(&refresh_token)
            .await?;

        let new_refresh = token_resp
            .refresh_token
            .as_deref()
            .unwrap_or(refresh_token.as_str());

        Self::save(
            &mut redis,
            wallet_address,
            &state.config.oauth_state_secret,
            &token_resp.access_token,
            new_refresh,
            token_resp.expires_in,
            &record.x_user_id,
            &record.x_username,
        )
        .await?;

        Ok(token_resp.access_token)
    }

    pub async fn get_x_user_id(
        state: &AppState,
        wallet_address: &str,
    ) -> Result<String, ServiceError> {
        let mut redis = state.redis.clone();
        let record = Self::get(&mut redis, wallet_address)
            .await?
            .ok_or_else(|| {
                ServiceError::bad_request(
                    "X account not connected — complete OAuth at /oauth/x/connect first",
                )
            })?;
        Ok(record.x_user_id)
    }
}

fn derive_key(secret: &str) -> [u8; 32] {
    let digest = Sha256::digest(secret.as_bytes());
    let mut key = [0u8; 32];
    key.copy_from_slice(&digest);
    key
}

pub fn encrypt_token(plaintext: &str, secret: &str) -> Result<String, ServiceError> {
    encrypt(plaintext, secret)
}

pub fn decrypt_token(encoded: &str, secret: &str) -> Result<String, ServiceError> {
    decrypt(encoded, secret)
}

fn encrypt(plaintext: &str, secret: &str) -> Result<String, ServiceError> {
    let key = derive_key(secret);
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| ServiceError::Internal(anyhow::anyhow!("cipher init: {e}")))?;

    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_bytes())
        .map_err(|e| ServiceError::Internal(anyhow::anyhow!("encrypt failed: {e}")))?;

    let mut out = nonce_bytes.to_vec();
    out.extend(ciphertext);
    Ok(base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        out,
    ))
}

fn decrypt(encoded: &str, secret: &str) -> Result<String, ServiceError> {
    let key = derive_key(secret);
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| ServiceError::Internal(anyhow::anyhow!("cipher init: {e}")))?;

    let bytes = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        encoded,
    )
    .map_err(|e| ServiceError::Internal(anyhow::anyhow!("base64 decode: {e}")))?;

    if bytes.len() < 12 {
        return Err(ServiceError::Internal(anyhow::anyhow!(
            "invalid encrypted token payload"
        )));
    }

    let (nonce_bytes, ciphertext) = bytes.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);

    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| ServiceError::Internal(anyhow::anyhow!("decrypt failed: {e}")))?;

    String::from_utf8(plaintext)
        .map_err(|e| ServiceError::Internal(anyhow::anyhow!("utf8 decode: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let secret = "change-me-min-32-bytes-secret-value";
        let encrypted = encrypt("refresh-token-abc", secret).unwrap();
        let decrypted = decrypt(&encrypted, secret).unwrap();
        assert_eq!(decrypted, "refresh-token-abc");
    }
}
