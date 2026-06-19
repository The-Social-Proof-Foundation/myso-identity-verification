// Copyright (c) The Social Proof Foundation, LLC.
// SPDX-License-Identifier: Apache-2.0

use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;

use crate::error::ServiceError;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct VerificationQuery {
    pub address: String,
}

#[derive(Debug, serde::Serialize)]
pub struct VerificationResponse {
    pub x_username: Option<String>,
    pub verified_x_account: bool,
    pub badges: Vec<BadgeSummary>,
}

#[derive(Debug, serde::Serialize)]
pub struct BadgeSummary {
    pub badge_id: String,
    pub badge_name: String,
}

pub async fn get_x_verification(
    State(state): State<AppState>,
    Query(query): Query<VerificationQuery>,
) -> Result<Json<VerificationResponse>, ServiceError> {
    let profile = state
        .indexer
        .get_profile_by_address(&query.address)
        .await?;

    let badges = state.indexer.get_profile_badges(&query.address).await?;
    let verified = state
        .indexer
        .has_ecosystem_badge(&query.address, "verified_x_account")
        .await?;

    Ok(Json(VerificationResponse {
        x_username: profile.and_then(|p| p.x_username),
        verified_x_account: verified,
        badges: badges
            .into_iter()
            .map(|b| BadgeSummary {
                badge_id: b.badge_id,
                badge_name: b.badge_name,
            })
            .collect(),
    }))
}
