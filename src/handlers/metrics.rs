// Copyright (c) The Social Proof Foundation, LLC.
// SPDX-License-Identifier: Apache-2.0

use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;

use crate::dripdrop::SocialAccountMetrics;
use crate::error::ServiceError;
use crate::state::AppState;
use crate::x_api::XUser;
use crate::x_tokens::XTokenStore;

const PROVIDER_X: &str = "x";

async fn push_x_metrics(state: &AppState, wallet_address: &str, x_user: &XUser) -> Result<(), ServiceError> {
    let (Some(dripdrop), Some(followers)) = (state.dripdrop.as_ref(), x_user.followers_count()) else {
        tracing::warn!(
            wallet = %wallet_address,
            x_username = %x_user.username,
            followers = ?x_user.followers_count(),
            "x metrics not reported (dripdrop client or follower count missing)"
        );
        return Ok(());
    };
    tracing::info!(
        wallet = %wallet_address,
        x_username = %x_user.username,
        followers,
        "reporting x followers to dripdrop"
    );
    dripdrop
        .report_social_metrics(SocialAccountMetrics {
            wallet_address: &wallet_address.to_lowercase(),
            provider: PROVIDER_X,
            external_account_id: &x_user.id,
            handle: &x_user.username,
            followers_count: followers,
        })
        .await
}

/// Send the follower count X just returned to DripDrop (Creator Partner eligibility). Best-effort.
pub(crate) fn report_x_metrics(state: &AppState, wallet_address: &str, x_user: &XUser) {
    let state = state.clone();
    let wallet = wallet_address.to_string();
    let user = x_user.clone();
    tokio::spawn(async move {
        if let Err(err) = push_x_metrics(&state, &wallet, &user).await {
            tracing::warn!(error = %err, "x metrics report to dripdrop failed");
        }
    });
}

/// `POST /verification/x/metrics` — the signed-in wallet asks for its X follower count to be
/// re-read from X (with its own OAuth grant) and reported to DripDrop.
pub async fn refresh_x_metrics(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ServiceError> {
    let session = state.sessions.validate_headers(&headers).await?;
    let wallet = session.wallet_address.to_lowercase();

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
    push_x_metrics(&state, &wallet, &user).await?;

    Ok(Json(serde_json::json!({
        "provider": PROVIDER_X,
        "x_username": user.username,
        "followers_count": followers,
    })))
}
