// Copyright (c) The Social Proof Foundation, LLC.
// SPDX-License-Identifier: Apache-2.0

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::Deserialize;

use crate::error::ServiceError;
use crate::indexer::assert_profile_owner;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct ConnectRequest {
    pub profile_id: String,
}

#[derive(Debug, serde::Serialize)]
pub struct ConnectResponse {
    pub authorize_url: String,
}

#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    pub code: String,
    pub state: String,
}

#[derive(Debug, serde::Serialize)]
pub struct CallbackResponse {
    pub status: &'static str,
    pub tx_digest: String,
    pub x_username: String,
}

pub async fn x_connect(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ConnectRequest>,
) -> Result<Json<ConnectResponse>, ServiceError> {
    let session = state.sessions.validate_headers(&headers).await?;
    let profile = state
        .indexer
        .get_profile_by_address(&session.wallet_address)
        .await?
        .ok_or_else(|| ServiceError::not_found("profile not found"))?;
    assert_profile_owner(&profile, &session.wallet_address)?;

    let on_chain_id = profile
        .profile_id
        .ok_or_else(|| ServiceError::bad_request("profile missing profile_id"))?;
    if on_chain_id != body.profile_id {
        return Err(ServiceError::bad_request("profile_id mismatch"));
    }

    let (authorize_url, _verifier) = state.x_oauth.build_authorize_url(
        &body.profile_id,
        &session.wallet_address,
    )?;

    Ok(Json(ConnectResponse { authorize_url }))
}

pub async fn x_callback(
    State(state): State<AppState>,
    Query(query): Query<CallbackQuery>,
) -> Result<Json<CallbackResponse>, ServiceError> {
    let oauth_state = state.x_oauth.decode_state(&query.state)?;
    let token = state
        .x_oauth
        .exchange_code(&query.code, &oauth_state.code_verifier)
        .await?;
    let x_user = state.x_api.get_authenticated_user(&token.access_token).await?;

    if state
        .indexer
        .x_username_taken(&x_user.username, Some(&oauth_state.wallet_address))
        .await?
    {
        return Err(ServiceError::conflict(format!(
            "X account @{} already linked to another profile",
            x_user.username
        )));
    }

    let shared_version = state
        .relayer
        .fetch_profile_shared_version(&oauth_state.profile_id)
        .await
        .map_err(|e| ServiceError::Internal(e.into()))?;

    let resp = state
        .relayer
        .verify_x_account(
            &oauth_state.profile_id,
            shared_version,
            &x_user.username,
        )
        .await
        .map_err(|e| ServiceError::Internal(e.into()))?;

    Ok(Json(CallbackResponse {
        status: "verified",
        tx_digest: resp.digest.to_string(),
        x_username: x_user.username,
    }))
}
