// Copyright (c) The Social Proof Foundation, LLC.
// SPDX-License-Identifier: Apache-2.0

use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;
use serde::Deserialize;

use crate::dripdrop::SocialAccountMetrics;
use crate::error::ServiceError;
use crate::state::AppState;
use crate::x_api::XUser;
use crate::x_tokens::XTokenStore;

const PROVIDER_X: &str = "x";

/// Send the follower count X just returned to DripDrop (Creator Partner eligibility). Best-effort.
pub(crate) fn report_x_metrics(state: &AppState, wallet_address: &str, x_user: &XUser) {
    let (Some(dripdrop), Some(followers)) = (state.dripdrop.clone(), x_user.followers_count()) else {
        return;
    };
    let wallet = wallet_address.to_lowercase();
    let user = x_user.clone();
    tokio::spawn(async move {
        let body = SocialAccountMetrics {
            wallet_address: &wallet,
            provider: PROVIDER_X,
            external_account_id: &user.id,
            handle: &user.username,
            followers_count: followers,
        };
        if let Err(err) = dripdrop.report_social_metrics(body).await {
            tracing::warn!(error = %err, "x metrics report to dripdrop failed");
        }
    });
}

#[derive(Debug, Deserialize)]
pub struct InternalMetricsRequest {
    pub wallet_address: String,
}

fn authorize_internal(state: &AppState, headers: &HeaderMap) -> Result<(), ServiceError> {
    let Some(expected) = state.config.dripdrop_internal_api_key.as_deref() else {
        return Err(ServiceError::unauthorized("internal api key not configured"));
    };
    let provided = headers
        .get("x-internal-api-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    if provided.is_empty() || provided != expected {
        return Err(ServiceError::unauthorized("invalid internal api key"));
    }
    Ok(())
}

/// `POST /internal/x/metrics` — live follower count for the X account the wallet connected.
/// Uses the wallet's own OAuth grant (refreshing it if needed), so a revoked grant fails here.
pub async fn internal_x_metrics(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<InternalMetricsRequest>,
) -> Result<Json<serde_json::Value>, ServiceError> {
    authorize_internal(&state, &headers)?;
    let wallet = req.wallet_address.trim().to_lowercase();
    if wallet.is_empty() {
        return Err(ServiceError::bad_request("wallet_address is required"));
    }

    let mut redis = state.redis.clone();
    let Some(record) = XTokenStore::get(&mut redis, &wallet).await? else {
        return Err(ServiceError::not_found("no X account connected for this wallet"));
    };
    let token = XTokenStore::get_valid_access_token(&state, &wallet).await?;
    let user = state.x_api.get_authenticated_user(&token).await?;
    if user.id != record.x_user_id {
        return Err(ServiceError::conflict("X grant no longer matches the verified account"));
    }
    let followers = user
        .followers_count()
        .ok_or_else(|| ServiceError::Upstream("x users/me returned no public_metrics".into()))?;

    Ok(Json(serde_json::json!({
        "provider": PROVIDER_X,
        "x_user_id": user.id,
        "x_username": user.username,
        "followers_count": followers,
    })))
}
