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
    // Wallet-keyed: the soulbound Verified X badge lives in the wallet, profile or not.
    let badges = state.indexer.get_wallet_badges(&query.address).await?;
    let verified_id = crate::indexer::IndexerClient::ecosystem_badge_id(
        crate::relayer::VERIFIED_X_BADGE,
    );
    let verified = badges.iter().any(|b| b.badge_id == verified_id);
    let x_username = if verified {
        state.indexer.wallet_x_username(&query.address).await?
    } else {
        None
    };

    Ok(Json(VerificationResponse {
        x_username,
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
