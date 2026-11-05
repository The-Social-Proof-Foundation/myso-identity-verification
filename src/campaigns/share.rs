// Copyright (c) The Social Proof Foundation, LLC.
// SPDX-License-Identifier: Apache-2.0

use chrono::{Duration, Utc};
use tracing::{info, warn};

use crate::campaigns::pending_queue::{PendingQueue, PendingShareCampaign};
use crate::error::ServiceError;
use crate::state::AppState;
use crate::x_api::{parse_tweet_id, tweet_contains_profile_link};
use crate::x_tokens::XTokenStore;

pub const BADGE_EARLY_ADOPTER: &str = "early_adopter";
pub const BADGE_AMBASSADOR: &str = "ambassador";

const JOB_LOCK_TTL_SECS: u64 = 120;

pub fn validate_badge_name(badge: &str, early_access_active: bool) -> Result<(), ServiceError> {
    match badge {
        BADGE_EARLY_ADOPTER if early_access_active => Ok(()),
        BADGE_EARLY_ADOPTER => Err(ServiceError::bad_request(
            "early adopter campaign has ended",
        )),
        BADGE_AMBASSADOR if !early_access_active => Ok(()),
        BADGE_AMBASSADOR => Err(ServiceError::bad_request(
            "ambassador campaign has not started yet",
        )),
        _ => Err(ServiceError::bad_request(format!("unsupported badge: {badge}"))),
    }
}

pub async fn start_share_campaign(
    state: &AppState,
    wallet_address: &str,
    badge_name: &str,
    tweet_url: &str,
) -> Result<ShareCampaignStatus, ServiceError> {
    validate_badge_name(badge_name, state.config.is_early_access_active())?;

    let access_token = XTokenStore::get_valid_access_token(state, wallet_address).await?;
    let x_user_id = XTokenStore::get_x_user_id(state, wallet_address).await?;

    let profile = state
        .indexer
        .get_profile_by_address(wallet_address)
        .await?
        .ok_or_else(|| ServiceError::not_found("profile not found"))?;
    // The profile is only needed for the username the tweet must link to; the badge
    // itself is minted into the wallet.
    crate::indexer::assert_profile_owner(&profile, wallet_address)?;

    if state
        .indexer
        .has_ecosystem_badge(wallet_address, badge_name)
        .await?
    {
        return Ok(ShareCampaignStatus::completed(None));
    }

    let username = profile
        .username
        .clone()
        .ok_or_else(|| ServiceError::bad_request("profile missing username"))?;
    let profile_url = state
        .config
        .profile_url_template
        .replace("{username}", &username);

    let tweet = state.x_api.get_tweet(tweet_url, &access_token).await?;

    verify_tweet_author(&tweet, &x_user_id)?;

    if !tweet_contains_profile_link(&tweet.text, &profile_url, &username) {
        return Err(ServiceError::bad_request(
            "tweet does not contain profile link",
        ));
    }

    let check_after = tweet.created_at
        + Duration::hours(state.config.share_campaign_check_delay_hours as i64);

    let job = PendingShareCampaign {
        wallet_address: wallet_address.to_lowercase(),
        badge_name: badge_name.to_string(),
        tweet_url: tweet_url.to_string(),
        tweet_id: parse_tweet_id(tweet_url)?,
        x_user_id,
        check_after,
        enqueued_at: Utc::now(),
    };

    let mut redis = state.redis.clone();
    PendingQueue::enqueue(&mut redis, &job).await?;

    if Utc::now() >= check_after {
        return process_single_job(state, &job).await;
    }

    Ok(ShareCampaignStatus::pending(check_after))
}

pub async fn get_share_status(
    state: &AppState,
    wallet_address: &str,
    badge_name: &str,
) -> Result<ShareCampaignStatus, ServiceError> {
    if state
        .indexer
        .has_ecosystem_badge(wallet_address, badge_name)
        .await?
    {
        return Ok(ShareCampaignStatus::completed(None));
    }

    let mut redis = state.redis.clone();
    if let Some(job) = PendingQueue::get(&mut redis, wallet_address, badge_name).await? {
        if Utc::now() >= job.check_after {
            return process_single_job(state, &job).await;
        }
        return Ok(ShareCampaignStatus::pending(job.check_after));
    }

    Ok(ShareCampaignStatus::not_started())
}

pub async fn get_all_share_statuses(
    state: &AppState,
    wallet_address: &str,
) -> Result<(ShareCampaignStatus, ShareCampaignStatus), ServiceError> {
    let early_adopter = get_share_status(state, wallet_address, BADGE_EARLY_ADOPTER).await?;
    let ambassador = get_share_status(state, wallet_address, BADGE_AMBASSADOR).await?;
    Ok((early_adopter, ambassador))
}

#[derive(Debug, serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ShareCampaignStatus {
    NotStarted,
    Pending { check_after: chrono::DateTime<Utc> },
    Completed { tx_digest: Option<String> },
    Failed { reason: String },
}

impl ShareCampaignStatus {
    fn pending(check_after: chrono::DateTime<Utc>) -> Self {
        Self::Pending { check_after }
    }

    fn completed(tx_digest: Option<String>) -> Self {
        Self::Completed { tx_digest }
    }

    fn not_started() -> Self {
        Self::NotStarted
    }
}

pub async fn process_pending_campaigns(state: &AppState) -> Result<ProcessSummary, ServiceError> {
    let now = Utc::now();
    let mut redis = state.redis.clone();
    let jobs = PendingQueue::list_due(&mut redis, now).await?;

    let mut summary = ProcessSummary::default();
    for job in jobs {
        match process_single_job(state, &job).await {
            Ok(status) => {
                if matches!(status, ShareCampaignStatus::Completed { .. }) {
                    summary.completed += 1;
                } else if matches!(status, ShareCampaignStatus::Failed { .. }) {
                    summary.failed += 1;
                } else {
                    summary.skipped += 1;
                }
            }
            Err(e) => {
                warn!(error = %e, wallet = %job.wallet_address, "campaign job failed");
                summary.errors += 1;
            }
        }
    }

    info!(
        completed = summary.completed,
        failed = summary.failed,
        skipped = summary.skipped,
        errors = summary.errors,
        "processed pending share campaigns"
    );
    Ok(summary)
}

#[derive(Debug, Default, serde::Serialize)]
pub struct ProcessSummary {
    pub completed: u32,
    pub failed: u32,
    pub skipped: u32,
    pub errors: u32,
}

async fn process_single_job(
    state: &AppState,
    job: &PendingShareCampaign,
) -> Result<ShareCampaignStatus, ServiceError> {
    if Utc::now() < job.check_after {
        return Ok(ShareCampaignStatus::pending(job.check_after));
    }

    let job_key = job.redis_key();
    let mut redis = state.redis.clone();
    let acquired = PendingQueue::try_acquire_job_lock(&mut redis, &job_key, JOB_LOCK_TTL_SECS)
        .await?;
    if !acquired {
        return Ok(ShareCampaignStatus::pending(job.check_after));
    }

    let result = process_single_job_inner(state, job).await;

    let mut redis = state.redis.clone();
    let _ = PendingQueue::release_job_lock(&mut redis, &job_key).await;

    result
}

async fn process_single_job_inner(
    state: &AppState,
    job: &PendingShareCampaign,
) -> Result<ShareCampaignStatus, ServiceError> {
    if state
        .indexer
        .has_ecosystem_badge(&job.wallet_address, &job.badge_name)
        .await?
    {
        let mut redis = state.redis.clone();
        PendingQueue::delete(&mut redis, &job.wallet_address, &job.badge_name).await?;
        return Ok(ShareCampaignStatus::completed(None));
    }

    let access_token = XTokenStore::get_valid_access_token(state, &job.wallet_address).await?;

    let profile = state
        .indexer
        .get_profile_by_address(&job.wallet_address)
        .await?
        .ok_or_else(|| ServiceError::not_found("profile not found"))?;
    let username = profile
        .username
        .ok_or_else(|| ServiceError::bad_request("profile missing username"))?;
    let profile_url = state
        .config
        .profile_url_template
        .replace("{username}", &username);

    let tweet = match state
        .x_api
        .get_tweet(&job.tweet_url, &access_token)
        .await
    {
        Ok(t) => t,
        Err(ServiceError::NotFound(_)) => {
            let mut redis = state.redis.clone();
            PendingQueue::delete(&mut redis, &job.wallet_address, &job.badge_name).await?;
            return Ok(ShareCampaignStatus::Failed {
                reason: "tweet deleted".into(),
            });
        }
        Err(e) => return Err(e),
    };

    let x_user_id = if job.x_user_id.is_empty() {
        XTokenStore::get_x_user_id(state, &job.wallet_address).await?
    } else {
        job.x_user_id.clone()
    };

    verify_tweet_author(&tweet, &x_user_id)?;

    if !tweet_contains_profile_link(&tweet.text, &profile_url, &username) {
        let mut redis = state.redis.clone();
        PendingQueue::delete(&mut redis, &job.wallet_address, &job.badge_name).await?;
        return Ok(ShareCampaignStatus::Failed {
            reason: "tweet no longer contains profile link".into(),
        });
    }

    let tx_digest = match state
        .relayer
        .mint_share_badge(&job.wallet_address, &job.badge_name, &state.config.badge_assets)
        .await
    {
        Ok(resp) => Some(resp.digest.to_string()),
        // Already minted (e.g. a concurrent run): the badge exists, so the job is done.
        Err(crate::relayer::BadgeMintError::AlreadyIssued) => None,
        Err(e) => return Err(ServiceError::Internal(anyhow::anyhow!(e))),
    };
    let mut redis = state.redis.clone();
    PendingQueue::delete(&mut redis, &job.wallet_address, &job.badge_name).await?;

    Ok(ShareCampaignStatus::completed(tx_digest))
}

fn verify_tweet_author(tweet: &crate::x_api::XTweet, x_user_id: &str) -> Result<(), ServiceError> {
    match &tweet.author_id {
        Some(author_id) if author_id == x_user_id => Ok(()),
        Some(_) => Err(ServiceError::bad_request(
            "tweet was not posted by the connected X account",
        )),
        None => Err(ServiceError::Upstream(
            "x api did not return tweet author_id".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use crate::x_api::XTweet;

    #[test]
    fn verify_tweet_author_matches() {
        let tweet = XTweet {
            id: "1".into(),
            text: "hello".into(),
            author_id: Some("42".into()),
            created_at: Utc::now(),
        };
        assert!(verify_tweet_author(&tweet, "42").is_ok());
    }

    #[test]
    fn verify_tweet_author_rejects_mismatch() {
        let tweet = XTweet {
            id: "1".into(),
            text: "hello".into(),
            author_id: Some("99".into()),
            created_at: Utc::now(),
        };
        assert!(verify_tweet_author(&tweet, "42").is_err());
    }
}
