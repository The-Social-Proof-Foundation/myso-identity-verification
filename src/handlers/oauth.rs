// Copyright (c) The Social Proof Foundation, LLC.
// SPDX-License-Identifier: Apache-2.0

use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::Json;
use serde::Deserialize;

use crate::chain::normalize_object_id;
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

fn wants_json(headers: &HeaderMap) -> bool {
    let accept = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let accepts_json = accept
        .split(',')
        .any(|part| part.trim().starts_with("application/json"));
    let accepts_html = accept
        .split(',')
        .any(|part| part.trim().starts_with("text/html"));
    accepts_json && !accepts_html
}

fn map_profile_fetch_error(err: anyhow::Error) -> ServiceError {
    let msg = err.to_string();
    if msg.contains("on-chain profile not found")
        || msg.contains("profile object missing")
        || msg.contains("is not a shared object")
        || msg.contains("missing owner field")
        || msg.contains("invalid object id")
    {
        ServiceError::Upstream(msg)
    } else {
        ServiceError::Internal(err)
    }
}

fn escape_html(raw: &str) -> String {
    raw.chars()
        .map(|c| match c {
            '&' => "&amp;".to_string(),
            '<' => "&lt;".to_string(),
            '>' => "&gt;".to_string(),
            '"' => "&quot;".to_string(),
            '\'' => "&#39;".to_string(),
            other => other.to_string(),
        })
        .collect()
}

fn callback_html_page(title: &str, heading: &str, detail: &str, ok: bool) -> Html<String> {
    let accent = if ok { "#22c55e" } else { "#ef4444" };
    let safe_title = escape_html(title);
    let safe_heading = escape_html(heading);
    let safe_detail = escape_html(detail);
    Html(format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>{safe_title}</title>
  <style>
    :root {{ color-scheme: dark; }}
    body {{
      margin: 0; min-height: 100vh; display: grid; place-items: center;
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
      background: #0b0b0f; color: #f5f5f7; padding: 24px;
    }}
    main {{
      max-width: 420px; width: 100%; text-align: center;
      border: 1px solid #222; border-radius: 16px; padding: 28px 22px;
      background: #121218;
    }}
    .dot {{
      width: 12px; height: 12px; border-radius: 999px; background: {accent};
      display: inline-block; margin-bottom: 14px;
    }}
    h1 {{ font-size: 1.25rem; margin: 0 0 10px; font-weight: 650; }}
    p {{ margin: 0; line-height: 1.45; color: #b0b0b8; font-size: 0.95rem; }}
    .hint {{ margin-top: 18px; font-size: 0.85rem; color: #7d7d88; }}
  </style>
</head>
<body>
  <main>
    <span class="dot" aria-hidden="true"></span>
    <h1>{safe_heading}</h1>
    <p>{safe_detail}</p>
    <p class="hint">You can close this window and return to the app.</p>
  </main>
</body>
</html>"#
    ))
}

fn callback_response(
    headers: &HeaderMap,
    status: StatusCode,
    body: serde_json::Value,
) -> Response {
    if wants_json(headers) {
        return (status, Json(body)).into_response();
    }

    let (heading, detail, ok) = if status.is_success() {
        let username = body
            .get("x_username")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        (
            "You’re verified on X".to_string(),
            if username.is_empty() {
                "Your X account is linked. Return to the app.".to_string()
            } else {
                format!("@{username} is linked. Return to the app.")
            },
            true,
        )
    } else {
        let err = body
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("Verification failed.");
        (
            "Verification failed".to_string(),
            err.to_string(),
            false,
        )
    };

    let mut response = callback_html_page(
        if ok {
            "X verification complete"
        } else {
            "X verification failed"
        },
        &heading,
        &detail,
        ok,
    )
    .into_response();
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    );
    response
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
        .as_deref()
        .ok_or_else(|| ServiceError::bad_request("profile missing profile_id"))?;
    let indexed_id = normalize_object_id(on_chain_id)
        .map_err(|e| ServiceError::bad_request(format!("indexer profile_id invalid: {e}")))?;
    let request_id = normalize_object_id(&body.profile_id)
        .map_err(|e| ServiceError::bad_request(format!("profile_id invalid: {e}")))?;
    if indexed_id != request_id {
        return Err(ServiceError::bad_request("profile_id mismatch"));
    }

    // Fail before sending the user to X if RPC cannot see this shared profile.
    state
        .relayer
        .fetch_profile_shared_version(&request_id)
        .await
        .map_err(map_profile_fetch_error)?;

    let (authorize_url, _verifier) = state
        .x_oauth
        .build_authorize_url(&request_id, &session.wallet_address)?;

    Ok(Json(ConnectResponse { authorize_url }))
}

pub async fn x_callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<CallbackQuery>,
) -> Response {
    match x_callback_inner(&state, query).await {
        Ok(body) => callback_response(&headers, StatusCode::OK, body),
        Err(err) => {
            let (status, message) = match &err {
                ServiceError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg.clone()),
                ServiceError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
                ServiceError::Conflict(msg) => (StatusCode::CONFLICT, msg.clone()),
                ServiceError::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
                ServiceError::Upstream(msg) => (StatusCode::BAD_GATEWAY, msg.clone()),
                ServiceError::Internal(inner) => {
                    (StatusCode::INTERNAL_SERVER_ERROR, inner.to_string())
                }
            };
            callback_response(
                &headers,
                status,
                serde_json::json!({ "error": message }),
            )
        }
    }
}

async fn x_callback_inner(
    state: &AppState,
    query: CallbackQuery,
) -> Result<serde_json::Value, ServiceError> {
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

    if oauth_state.flow == "poc_claim" {
        let Json(body) = crate::handlers::poc_claim::complete_poc_claim_oauth(
            state,
            &oauth_state,
            &token.access_token,
            refresh_token,
            token.expires_in,
            &x_user.id,
            &x_user.username,
        )
        .await?;
        return Ok(body);
    }

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

    let profile_id = normalize_object_id(&oauth_state.profile_id)
        .map_err(|e| ServiceError::bad_request(format!("oauth state profile_id invalid: {e}")))?;

    let shared_version = state
        .relayer
        .fetch_profile_shared_version(&profile_id)
        .await
        .map_err(map_profile_fetch_error)?;

    let resp = state
        .relayer
        .verify_x_account(&profile_id, shared_version, &x_user.username)
        .await
        .map_err(|e| ServiceError::Internal(e.into()))?;

    Ok(serde_json::json!({
        "status": "verified",
        "tx_digest": resp.digest.to_string(),
        "x_username": x_user.username,
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

    #[test]
    fn wants_json_when_accept_is_json_only() {
        let mut headers = HeaderMap::new();
        headers.insert(header::ACCEPT, HeaderValue::from_static("application/json"));
        assert!(wants_json(&headers));
    }

    #[test]
    fn wants_html_for_browser_accept() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ACCEPT,
            HeaderValue::from_static(
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            ),
        );
        assert!(!wants_json(&headers));
    }

    #[test]
    fn map_profile_fetch_error_is_upstream() {
        let err = map_profile_fetch_error(anyhow::anyhow!(
            "on-chain profile not found for 0xabc — check MYSO_RPC_URL matches the indexer network"
        ));
        assert!(matches!(err, ServiceError::Upstream(_)));
    }
}
