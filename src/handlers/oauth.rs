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
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct CallbackResponse {
    pub status: &'static str,
    pub tx_digest: String,
    pub x_username: String,
}

pub fn parse_callback_params(query: &CallbackQuery) -> Result<(String, String), ServiceError> {
    if let Some(error) = &query.error {
        let detail = query.error_description.as_deref().unwrap_or("");
        return Err(ServiceError::bad_request(format!(
            "x oauth denied: {error}{}",
            if detail.is_empty() {
                String::new()
            } else {
                format!(" — {detail}")
            }
        )));
    }

    let code = query
        .code
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            ServiceError::bad_request(
                "missing authorization code — start from POST /oauth/x/connect and approve the X app",
            )
        })?;

    let state = query
        .state
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ServiceError::bad_request("missing oauth state"))?;

    Ok((code.to_string(), state.to_string()))
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
    let (code, state_jwt) = parse_callback_params(&query)?;
    let oauth_state = state.x_oauth.decode_state(&state_jwt)?;
    let token = state
        .x_oauth
        .exchange_code(&code, &oauth_state.code_verifier)
        .await?;
    let x_user = state.x_api.get_authenticated_user(&token.access_token).await?;

    let refresh_token = token.refresh_token.as_deref().ok_or_else(|| {
        ServiceError::Upstream(
            "x oauth did not return refresh_token — ensure offline.access scope is granted"
                .into(),
        )
    })?;

    let mut redis = state.redis.clone();
    crate::x_tokens::XTokenStore::save(
        &mut redis,
        &oauth_state.wallet_address,
        &state.config.oauth_state_secret,
        &token.access_token,
        refresh_token,
        token.expires_in,
        &x_user.id,
        &x_user.username,
    )
    .await?;

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

#[cfg(test)]
mod tests {
    use super::*;

    fn query(code: Option<&str>, state: Option<&str>) -> CallbackQuery {
        CallbackQuery {
            code: code.map(str::to_string),
            state: state.map(str::to_string),
            error: None,
            error_description: None,
        }
    }

    #[test]
    fn parse_callback_params_success() {
        let result = parse_callback_params(&query(Some("abc123"), Some("state-jwt")));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), ("abc123".into(), "state-jwt".into()));
    }

    #[test]
    fn parse_callback_params_x_error() {
        let query = CallbackQuery {
            code: None,
            state: Some("state-jwt".into()),
            error: Some("access_denied".into()),
            error_description: Some("User cancelled".into()),
        };
        let err = parse_callback_params(&query).unwrap_err();
        assert!(matches!(err, ServiceError::BadRequest(msg) if msg.contains("x oauth denied: access_denied — User cancelled")));
    }

    #[test]
    fn parse_callback_params_missing_code() {
        let err = parse_callback_params(&query(None, Some("state-jwt"))).unwrap_err();
        assert!(matches!(err, ServiceError::BadRequest(msg) if msg.contains("missing authorization code")));
    }

    #[test]
    fn parse_callback_params_empty_query() {
        let err = parse_callback_params(&query(None, None)).unwrap_err();
        assert!(matches!(err, ServiceError::BadRequest(msg) if msg.contains("missing authorization code")));
    }

    #[test]
    fn parse_callback_params_missing_state() {
        let err = parse_callback_params(&query(Some("abc123"), None)).unwrap_err();
        assert!(matches!(err, ServiceError::BadRequest(msg) if msg == "missing oauth state"));
    }
}
