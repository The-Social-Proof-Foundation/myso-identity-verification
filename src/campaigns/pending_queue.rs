// Copyright (c) The Social Proof Foundation, LLC.
// SPDX-License-Identifier: Apache-2.0

use chrono::{DateTime, Utc};
use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use serde::{Deserialize, Serialize};

use crate::error::ServiceError;

pub const PENDING_INDEX_KEY: &str = "pending_share_campaigns:index";
pub const PENDING_JOB_PREFIX: &str = "pending_share_campaign:";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingShareCampaign {
    pub profile_id: String,
    pub wallet_address: String,
    pub profile_shared_version: u64,
    pub badge_name: String,
    pub tweet_url: String,
    pub tweet_id: String,
    pub check_after: DateTime<Utc>,
    pub enqueued_at: DateTime<Utc>,
}

impl PendingShareCampaign {
    pub fn redis_key(&self) -> String {
        job_key(&self.profile_id, &self.badge_name)
    }
}

pub fn job_key(profile_id: &str, badge_name: &str) -> String {
    format!("{PENDING_JOB_PREFIX}{profile_id}:{badge_name}")
}

pub struct PendingQueue;

impl PendingQueue {
    pub async fn enqueue(
        redis: &mut ConnectionManager,
        job: &PendingShareCampaign,
    ) -> Result<(), ServiceError> {
        let key = job.redis_key();
        let payload = serde_json::to_string(job)
            .map_err(|e| ServiceError::Internal(e.into()))?;
        redis
            .set::<_, _, ()>(&key, payload)
            .await
            .map_err(|e| ServiceError::Upstream(format!("redis set failed: {e}")))?;
        redis
            .sadd::<_, _, ()>(PENDING_INDEX_KEY, &key)
            .await
            .map_err(|e| ServiceError::Upstream(format!("redis sadd failed: {e}")))?;
        Ok(())
    }

    pub async fn get(
        redis: &mut ConnectionManager,
        profile_id: &str,
        badge_name: &str,
    ) -> Result<Option<PendingShareCampaign>, ServiceError> {
        let key = job_key(profile_id, badge_name);
        let raw: Option<String> = redis
            .get(&key)
            .await
            .map_err(|e| ServiceError::Upstream(format!("redis get failed: {e}")))?;
        raw.map(|s| {
            serde_json::from_str(&s).map_err(|e| ServiceError::Upstream(format!("job parse: {e}")))
        })
        .transpose()
    }

    pub async fn delete(
        redis: &mut ConnectionManager,
        profile_id: &str,
        badge_name: &str,
    ) -> Result<(), ServiceError> {
        let key = job_key(profile_id, badge_name);
        redis
            .del::<_, ()>(&key)
            .await
            .map_err(|e| ServiceError::Upstream(format!("redis del failed: {e}")))?;
        redis
            .srem::<_, _, ()>(PENDING_INDEX_KEY, &key)
            .await
            .map_err(|e| ServiceError::Upstream(format!("redis srem failed: {e}")))?;
        Ok(())
    }

    pub async fn list_due(
        redis: &mut ConnectionManager,
        now: DateTime<Utc>,
    ) -> Result<Vec<PendingShareCampaign>, ServiceError> {
        let keys: Vec<String> = redis
            .smembers(PENDING_INDEX_KEY)
            .await
            .map_err(|e| ServiceError::Upstream(format!("redis smembers failed: {e}")))?;

        let mut due = Vec::new();
        for key in keys {
            let raw: Option<String> = redis
                .get(&key)
                .await
                .map_err(|e| ServiceError::Upstream(format!("redis get failed: {e}")))?;
            let Some(raw) = raw else {
                let _: () = redis
                    .srem(PENDING_INDEX_KEY, &key)
                    .await
                    .map_err(|e| ServiceError::Upstream(format!("redis srem failed: {e}")))?;
                continue;
            };
            let job: PendingShareCampaign = serde_json::from_str(&raw)
                .map_err(|e| ServiceError::Upstream(format!("job parse: {e}")))?;
            if job.check_after <= now {
                due.push(job);
            }
        }
        Ok(due)
    }
}
