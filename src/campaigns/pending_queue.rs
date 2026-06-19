// Copyright (c) The Social Proof Foundation, LLC.
// SPDX-License-Identifier: Apache-2.0

use chrono::{DateTime, Utc};
use redis::aio::ConnectionManager;
use redis::{AsyncCommands};
use serde::{Deserialize, Serialize};

use crate::error::ServiceError;

pub const PENDING_DUE_KEY: &str = "pending_share_campaigns:due";
pub const PENDING_INDEX_KEY: &str = "pending_share_campaigns:index";
pub const PENDING_JOB_PREFIX: &str = "pending_share_campaign:";
pub const SCHEDULER_LOCK_KEY: &str = "pending_share_campaigns:scheduler_lock";
pub const JOB_LOCK_PREFIX: &str = "pending_share_campaign:lock:";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingShareCampaign {
    pub profile_id: String,
    pub wallet_address: String,
    pub profile_shared_version: u64,
    pub badge_name: String,
    pub tweet_url: String,
    pub tweet_id: String,
    #[serde(default)]
    pub x_user_id: String,
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

pub fn job_lock_key(job_key: &str) -> String {
    format!("{JOB_LOCK_PREFIX}{job_key}")
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
        let score = job.check_after.timestamp() as f64;

        redis
            .set::<_, _, ()>(&key, payload)
            .await
            .map_err(|e| ServiceError::Upstream(format!("redis set failed: {e}")))?;
        redis
            .zadd::<_, _, _, ()>(PENDING_DUE_KEY, key.clone(), score)
            .await
            .map_err(|e| ServiceError::Upstream(format!("redis zadd failed: {e}")))?;
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
            .zrem::<_, _, ()>(PENDING_DUE_KEY, &key)
            .await
            .map_err(|e| ServiceError::Upstream(format!("redis zrem failed: {e}")))?;
        Ok(())
    }

    pub async fn list_due(
        redis: &mut ConnectionManager,
        now: DateTime<Utc>,
    ) -> Result<Vec<PendingShareCampaign>, ServiceError> {
        let now_score = now.timestamp() as f64;
        let keys: Vec<String> = redis
            .zrangebyscore(PENDING_DUE_KEY, "-inf", now_score)
            .await
            .map_err(|e| ServiceError::Upstream(format!("redis zrangebyscore failed: {e}")))?;

        let mut due = Vec::new();
        for key in keys {
            let Some(job) = Self::load_job(redis, &key).await? else {
                let _: () = redis
                    .zrem(PENDING_DUE_KEY, &key)
                    .await
                    .map_err(|e| ServiceError::Upstream(format!("redis zrem failed: {e}")))?;
                continue;
            };
            if job.check_after <= now {
                due.push(job);
            }
        }
        Ok(due)
    }

    pub async fn next_check_after(
        redis: &mut ConnectionManager,
    ) -> Result<Option<DateTime<Utc>>, ServiceError> {
        let entries: Vec<(String, f64)> = redis
            .zrange_withscores(PENDING_DUE_KEY, 0, 0)
            .await
            .map_err(|e| ServiceError::Upstream(format!("redis zrange failed: {e}")))?;

        let Some((_, score)) = entries.into_iter().next() else {
            return Ok(None);
        };

        let ts = score.trunc() as i64;
        Ok(DateTime::from_timestamp(ts, 0))
    }

    pub async fn migrate_legacy_index(redis: &mut ConnectionManager) -> Result<u32, ServiceError> {
        let keys: Vec<String> = redis
            .smembers(PENDING_INDEX_KEY)
            .await
            .map_err(|e| ServiceError::Upstream(format!("redis smembers failed: {e}")))?;

        let mut migrated = 0u32;
        for key in keys {
            let Some(job) = Self::load_job(redis, &key).await? else {
                let _: () = redis
                    .srem(PENDING_INDEX_KEY, &key)
                    .await
                    .map_err(|e| ServiceError::Upstream(format!("redis srem failed: {e}")))?;
                continue;
            };

            let score = job.check_after.timestamp() as f64;
            redis
                .zadd::<_, _, _, ()>(PENDING_DUE_KEY, key.clone(), score)
                .await
                .map_err(|e| ServiceError::Upstream(format!("redis zadd failed: {e}")))?;
            redis
                .srem::<_, _, ()>(PENDING_INDEX_KEY, &key)
                .await
                .map_err(|e| ServiceError::Upstream(format!("redis srem failed: {e}")))?;
            migrated += 1;
        }
        Ok(migrated)
    }

    pub async fn try_acquire_job_lock(
        redis: &mut ConnectionManager,
        job_key: &str,
        ttl_secs: u64,
    ) -> Result<bool, ServiceError> {
        let lock_key = job_lock_key(job_key);
        let acquired: bool = redis
            .set_nx(&lock_key, "1")
            .await
            .map_err(|e| ServiceError::Upstream(format!("redis set nx failed: {e}")))?;
        if acquired {
            redis
                .expire::<_, ()>(&lock_key, ttl_secs as i64)
                .await
                .map_err(|e| ServiceError::Upstream(format!("redis expire failed: {e}")))?;
        }
        Ok(acquired)
    }

    pub async fn release_job_lock(
        redis: &mut ConnectionManager,
        job_key: &str,
    ) -> Result<(), ServiceError> {
        let lock_key = job_lock_key(job_key);
        redis
            .del::<_, ()>(&lock_key)
            .await
            .map_err(|e| ServiceError::Upstream(format!("redis del failed: {e}")))?;
        Ok(())
    }

    pub async fn try_acquire_scheduler_lock(
        redis: &mut ConnectionManager,
        ttl_secs: u64,
    ) -> Result<bool, ServiceError> {
        let acquired: bool = redis
            .set_nx(SCHEDULER_LOCK_KEY, "1")
            .await
            .map_err(|e| ServiceError::Upstream(format!("redis set nx failed: {e}")))?;
        if acquired {
            redis
                .expire::<_, ()>(SCHEDULER_LOCK_KEY, ttl_secs as i64)
                .await
                .map_err(|e| ServiceError::Upstream(format!("redis expire failed: {e}")))?;
        }
        Ok(acquired)
    }

    pub async fn renew_scheduler_lock(
        redis: &mut ConnectionManager,
        ttl_secs: u64,
    ) -> Result<(), ServiceError> {
        redis
            .expire::<_, ()>(SCHEDULER_LOCK_KEY, ttl_secs as i64)
            .await
            .map_err(|e| ServiceError::Upstream(format!("redis expire failed: {e}")))?;
        Ok(())
    }

    pub async fn release_scheduler_lock(
        redis: &mut ConnectionManager,
    ) -> Result<(), ServiceError> {
        redis
            .del::<_, ()>(SCHEDULER_LOCK_KEY)
            .await
            .map_err(|e| ServiceError::Upstream(format!("redis del failed: {e}")))?;
        Ok(())
    }

    async fn load_job(
        redis: &mut ConnectionManager,
        key: &str,
    ) -> Result<Option<PendingShareCampaign>, ServiceError> {
        let raw: Option<String> = redis
            .get(key)
            .await
            .map_err(|e| ServiceError::Upstream(format!("redis get failed: {e}")))?;
        raw.map(|s| {
            serde_json::from_str(&s).map_err(|e| ServiceError::Upstream(format!("job parse: {e}")))
        })
        .transpose()
    }
}

/// Pure helper for scheduler sleep duration (testable without Redis).
pub fn scheduler_sleep_duration(
    now: DateTime<Utc>,
    next: Option<DateTime<Utc>>,
    idle_sleep_secs: u64,
) -> std::time::Duration {
    match next {
        None => std::time::Duration::from_secs(idle_sleep_secs),
        Some(t) if t <= now => std::time::Duration::from_secs(0),
        Some(t) => t
            .signed_duration_since(now)
            .to_std()
            .unwrap_or(std::time::Duration::from_secs(0)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheduler_sleep_when_no_jobs() {
        let now = Utc::now();
        let d = scheduler_sleep_duration(now, None, 60);
        assert_eq!(d, std::time::Duration::from_secs(60));
    }

    #[test]
    fn scheduler_sleep_when_overdue() {
        let now = Utc::now();
        let past = now - chrono::Duration::minutes(5);
        let d = scheduler_sleep_duration(now, Some(past), 60);
        assert_eq!(d, std::time::Duration::from_secs(0));
    }

    #[test]
    fn scheduler_sleep_until_future_deadline() {
        let now = Utc::now();
        let future = now + chrono::Duration::seconds(30);
        let d = scheduler_sleep_duration(now, Some(future), 60);
        assert!(d >= std::time::Duration::from_secs(29));
        assert!(d <= std::time::Duration::from_secs(31));
    }
}
