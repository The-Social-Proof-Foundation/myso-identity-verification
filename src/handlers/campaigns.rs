// Copyright (c) The Social Proof Foundation, LLC.
// SPDX-License-Identifier: Apache-2.0

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::campaigns::share::{
    get_all_share_statuses, get_share_status, start_share_campaign, validate_badge_name,
    ShareCampaignStatus,
};
use crate::error::ServiceError;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct ShareStartRequest {
    pub profile_id: String,
    pub badge: String,
    pub tweet_url: String,
}

#[derive(Debug, Deserialize)]
pub struct ShareStatusQuery {
    pub address: String,
    #[serde(default)]
    pub badge: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum ShareStatusResponse {
    Single(ShareCampaignStatus),
    All {
        early_adopter: ShareCampaignStatus,
        ambassador: ShareCampaignStatus,
    },
}

pub async fn share_start(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ShareStartRequest>,
) -> Result<Json<ShareCampaignStatus>, ServiceError> {
    let session = state.sessions.validate_headers(&headers).await?;
    let status = start_share_campaign(
        &state,
        &session.wallet_address,
        &body.profile_id,
        &body.badge,
        &body.tweet_url,
    )
    .await?;
    Ok(Json(status))
}

pub async fn share_status(
    State(state): State<AppState>,
    Query(query): Query<ShareStatusQuery>,
) -> Result<Json<ShareStatusResponse>, ServiceError> {
    match query.badge {
        Some(badge) => {
            validate_badge_name(&badge, state.config.is_early_access_active())?;
            let status = get_share_status(&state, &query.address, &badge).await?;
            Ok(Json(ShareStatusResponse::Single(status)))
        }
        None => {
            let (early_adopter, ambassador) =
                get_all_share_statuses(&state, &query.address).await?;
            Ok(Json(ShareStatusResponse::All {
                early_adopter,
                ambassador,
            }))
        }
    }
}
