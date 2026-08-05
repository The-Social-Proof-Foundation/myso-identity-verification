// Copyright (c) The Social Proof Foundation, LLC.
// SPDX-License-Identifier: Apache-2.0

use chrono::{DateTime, Duration, Utc};
use redis::aio::ConnectionManager;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};

use crate::error::ServiceError;

const POC_CLAIM_TOKEN_PREFIX: &str = "poc_claim:x_token:";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PocClaimTokenRecord {
    pub access_token: String,
    pub refresh_token_encrypted: String,
    pub expires_at: DateTime<Utc>,
    pub x_user_id: String,
    pub x_username: String,
    pub identity_hash: String,
    pub beneficiary_id: String,
}

pub struct PocClaimTokenStore;

impl PocClaimTokenStore {
    pub fn redis_key(wallet: &str, identity_hash: &str) -> String {
        format!(
            "{POC_CLAIM_TOKEN_PREFIX}{}:{}",
            wallet.to_lowercase(),
            identity_hash.to_lowercase()
        )
    }

    pub async fn save(
        redis: &mut ConnectionManager,
        wallet: &str,
        identity_hash: &str,
        encryption_secret: &str,
        access_token: &str,
        refresh_token: &str,
        expires_in_secs: Option<u64>,
        x_user_id: &str,
        x_username: &str,
        beneficiary_id: &str,
    ) -> Result<(), ServiceError> {
        let ttl = expires_in_secs.unwrap_or(7200) as i64;
        let record = PocClaimTokenRecord {
            access_token: access_token.to_string(),
            refresh_token_encrypted: crate::x_tokens::encrypt_token(refresh_token, encryption_secret)?,
            expires_at: Utc::now() + Duration::seconds(ttl),
            x_user_id: x_user_id.to_string(),
            x_username: x_username.to_string(),
            identity_hash: identity_hash.to_lowercase(),
            beneficiary_id: beneficiary_id.to_string(),
        };
        let payload = serde_json::to_string(&record)
            .map_err(|e| ServiceError::Internal(e.into()))?;
        redis
            .set::<_, _, ()>(Self::redis_key(wallet, identity_hash), payload)
            .await
            .map_err(|e| ServiceError::Upstream(format!("redis set poc claim token failed: {e}")))?;
        Ok(())
    }

    pub async fn get(
        redis: &mut ConnectionManager,
        wallet: &str,
        identity_hash: &str,
    ) -> Result<Option<PocClaimTokenRecord>, ServiceError> {
        let raw: Option<String> = redis
            .get(Self::redis_key(wallet, identity_hash))
            .await
            .map_err(|e| ServiceError::Upstream(format!("redis get poc claim token failed: {e}")))?;
        raw.map(|s| {
            serde_json::from_str(&s)
                .map_err(|e| ServiceError::Upstream(format!("poc claim token parse: {e}")))
        })
        .transpose()
    }
}
