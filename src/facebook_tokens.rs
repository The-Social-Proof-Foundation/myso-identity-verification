// Copyright (c) The Social Proof Foundation, LLC.
// SPDX-License-Identifier: Apache-2.0

use chrono::{DateTime, Duration, Utc};
use redis::aio::ConnectionManager;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};

use crate::error::ServiceError;
use crate::x_tokens::encrypt_token;

const TOKEN_PREFIX: &str = "facebook_tokens:";
const ID_INDEX_PREFIX: &str = "facebook_wallet:";
const DEFAULT_TOKEN_TTL_SECS: i64 = 60 * 60 * 24 * 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FacebookTokenRecord {
    pub access_token_encrypted: String,
    pub expires_at: DateTime<Utc>,
    pub facebook_id: String,
    pub facebook_name: String,
}

pub struct FacebookTokenStore;

impl FacebookTokenStore {
    pub fn redis_key(wallet_address: &str) -> String {
        format!("{TOKEN_PREFIX}{}", wallet_address.to_lowercase())
    }

    pub fn id_index_key(facebook_id: &str) -> String {
        format!("{ID_INDEX_PREFIX}{facebook_id}")
    }

    pub async fn save(
        redis: &mut ConnectionManager,
        wallet_address: &str,
        encryption_secret: &str,
        access_token: &str,
        expires_in_secs: Option<u64>,
        facebook_id: &str,
        facebook_name: &str,
    ) -> Result<(), ServiceError> {
        let ttl = expires_in_secs.unwrap_or(DEFAULT_TOKEN_TTL_SECS as u64) as i64;
        let record = FacebookTokenRecord {
            access_token_encrypted: encrypt_token(access_token, encryption_secret)?,
            expires_at: Utc::now() + Duration::seconds(ttl),
            facebook_id: facebook_id.to_string(),
            facebook_name: facebook_name.to_string(),
        };
        let payload =
            serde_json::to_string(&record).map_err(|e| ServiceError::Internal(e.into()))?;
        redis
            .set::<_, _, ()>(Self::redis_key(wallet_address), payload)
            .await
            .map_err(|e| ServiceError::Upstream(format!("redis set facebook token failed: {e}")))?;
        redis
            .set::<_, _, ()>(
                Self::id_index_key(facebook_id),
                wallet_address.to_lowercase(),
            )
            .await
            .map_err(|e| ServiceError::Upstream(format!("redis set facebook index failed: {e}")))?;
        Ok(())
    }

    pub async fn get(
        redis: &mut ConnectionManager,
        wallet_address: &str,
    ) -> Result<Option<FacebookTokenRecord>, ServiceError> {
        let raw: Option<String> = redis
            .get(Self::redis_key(wallet_address))
            .await
            .map_err(|e| ServiceError::Upstream(format!("redis get facebook token failed: {e}")))?;
        raw.map(|s| {
            serde_json::from_str(&s)
                .map_err(|e| ServiceError::Upstream(format!("facebook token parse: {e}")))
        })
        .transpose()
    }

    pub async fn wallet_for_facebook_id(
        redis: &mut ConnectionManager,
        facebook_id: &str,
    ) -> Result<Option<String>, ServiceError> {
        redis
            .get(Self::id_index_key(facebook_id))
            .await
            .map_err(|e| ServiceError::Upstream(format!("redis get facebook index failed: {e}")))
    }

    pub async fn delete(
        redis: &mut ConnectionManager,
        wallet_address: &str,
        facebook_id: &str,
    ) -> Result<(), ServiceError> {
        redis
            .del::<_, ()>(Self::redis_key(wallet_address))
            .await
            .map_err(|e| ServiceError::Upstream(format!("redis del facebook token failed: {e}")))?;
        redis
            .del::<_, ()>(Self::id_index_key(facebook_id))
            .await
            .map_err(|e| ServiceError::Upstream(format!("redis del facebook index failed: {e}")))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redis_keys_normalize_wallet() {
        assert_eq!(
            FacebookTokenStore::redis_key("0xABC"),
            "facebook_tokens:0xabc"
        );
        assert_eq!(FacebookTokenStore::id_index_key("99"), "facebook_wallet:99");
    }
}
