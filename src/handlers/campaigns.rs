// Copyright (c) The Social Proof Foundation, LLC.
// SPDX-License-Identifier: Apache-2.0

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::Deserialize;

use crate::campaigns::share::{get_share_status, start_share_campaign, ShareCampaignStatus};
use crate::error::ServiceError;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct ShareStartRequest {
    pub profile_id: String,
    pub badge: String,
    pub tweet_url: String,
    #[serde(default)]
    pub x_access_token: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ShareStatusQuery {
    pub address: String,
    pub badge: String,
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
        body.x_access_token.as_deref(),
    )
    .await?;
    Ok(Json(status))
}

pub async fn share_status(
    State(state): State<AppState>,
    Query(query): Query<ShareStatusQuery>,
) -> Result<Json<ShareCampaignStatus>, ServiceError> {
    let status = get_share_status(&state, &query.address, &query.badge).await?;
    Ok(Json(status))
}
