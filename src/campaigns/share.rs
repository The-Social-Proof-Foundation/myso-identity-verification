// Copyright (c) The Social Proof Foundation, LLC.
// SPDX-License-Identifier: Apache-2.0

use chrono::{Duration, Utc};
use tracing::{info, warn};

use crate::campaigns::pending_queue::{PendingQueue, PendingShareCampaign};
use crate::error::ServiceError;
use crate::state::AppState;
use crate::x_api::{parse_tweet_id, tweet_contains_profile_link};

pub const BADGE_EARLY_ADOPTER: &str = "early_adopter";
pub const BADGE_AMBASSADOR: &str = "ambassador";

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
    profile_id: &str,
    badge_name: &str,
    tweet_url: &str,
    x_access_token: Option<&str>,
) -> Result<ShareCampaignStatus, ServiceError> {
    validate_badge_name(badge_name, state.config.is_early_access_active())?;

    let profile = state
        .indexer
        .get_profile_by_address(wallet_address)
        .await?
        .ok_or_else(|| ServiceError::not_found("profile not found"))?;
    crate::indexer::assert_profile_owner(&profile, wallet_address)?;

    let on_chain_profile_id = profile
        .profile_id
        .clone()
        .ok_or_else(|| ServiceError::bad_request("profile missing profile_id"))?;

    if on_chain_profile_id != profile_id {
        return Err(ServiceError::bad_request("profile_id mismatch"));
    }

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

    let tweet = state
        .x_api
        .get_tweet(tweet_url, x_access_token)
        .await?;

    if !tweet_contains_profile_link(&tweet.text, &profile_url, &username) {
        return Err(ServiceError::bad_request(
            "tweet does not contain profile link",
        ));
    }

    let check_after = tweet.created_at + Duration::hours(24);
    let shared_version = state
        .relayer
        .fetch_profile_shared_version(&on_chain_profile_id)
        .await
        .map_err(|e| ServiceError::Internal(e.into()))?;

    let job = PendingShareCampaign {
        profile_id: on_chain_profile_id.clone(),
        wallet_address: wallet_address.to_string(),
        profile_shared_version: shared_version,
        badge_name: badge_name.to_string(),
        tweet_url: tweet_url.to_string(),
        tweet_id: parse_tweet_id(tweet_url)?,
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

    let profile = state
        .indexer
        .get_profile_by_address(wallet_address)
        .await?
        .ok_or_else(|| ServiceError::not_found("profile not found"))?;

    let profile_id = profile
        .profile_id
        .ok_or_else(|| ServiceError::bad_request("profile missing profile_id"))?;

    let mut redis = state.redis.clone();
    if let Some(job) = PendingQueue::get(&mut redis, &profile_id, badge_name).await? {
        if Utc::now() >= job.check_after {
            return Ok(ShareCampaignStatus::pending(job.check_after));
        }
        return Ok(ShareCampaignStatus::pending(job.check_after));
    }

    Ok(ShareCampaignStatus::not_started())
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
                warn!(error = %e, profile_id = %job.profile_id, "campaign job failed");
                summary.failed += 1;
            }
        }
    }

    info!(
        completed = summary.completed,
        failed = summary.failed,
        skipped = summary.skipped,
        "processed pending share campaigns"
    );
    Ok(summary)
}

#[derive(Debug, Default, serde::Serialize)]
pub struct ProcessSummary {
    pub completed: u32,
    pub failed: u32,
    pub skipped: u32,
}

async fn process_single_job(
    state: &AppState,
    job: &PendingShareCampaign,
) -> Result<ShareCampaignStatus, ServiceError> {
    if Utc::now() < job.check_after {
        return Ok(ShareCampaignStatus::pending(job.check_after));
    }

    if state
        .indexer
        .has_ecosystem_badge(&job.wallet_address, &job.badge_name)
        .await?
    {
        let mut redis = state.redis.clone();
        PendingQueue::delete(&mut redis, &job.profile_id, &job.badge_name).await?;
        return Ok(ShareCampaignStatus::completed(None));
    }

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

    let tweet = match state.x_api.get_tweet(&job.tweet_url, None).await {
        Ok(t) => t,
        Err(ServiceError::NotFound(_)) => {
            let mut redis = state.redis.clone();
            PendingQueue::delete(&mut redis, &job.profile_id, &job.badge_name).await?;
            return Ok(ShareCampaignStatus::Failed {
                reason: "tweet deleted".into(),
            });
        }
        Err(e) => return Err(e),
    };

    if !tweet_contains_profile_link(&tweet.text, &profile_url, &username) {
        let mut redis = state.redis.clone();
        PendingQueue::delete(&mut redis, &job.profile_id, &job.badge_name).await?;
        return Ok(ShareCampaignStatus::Failed {
            reason: "tweet no longer contains profile link".into(),
        });
    }

    let resp = state
        .relayer
        .assign_share_badge(
            &job.profile_id,
            job.profile_shared_version,
            &job.badge_name,
            &state.config.badge_assets,
        )
        .await
        .map_err(|e| ServiceError::Internal(e.into()))?;

    let tx_digest = resp.digest.to_string();
    let mut redis = state.redis.clone();
    PendingQueue::delete(&mut redis, &job.profile_id, &job.badge_name).await?;

    Ok(ShareCampaignStatus::completed(Some(tx_digest)))
}
