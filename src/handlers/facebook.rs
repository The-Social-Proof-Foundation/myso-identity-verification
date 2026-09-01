// Copyright (c) The Social Proof Foundation, LLC.
// SPDX-License-Identifier: Apache-2.0

use axum::extract::{Form, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::Json;
use redis::AsyncCommands;
use serde::Deserialize;

use crate::chain::normalize_object_id;
use crate::error::ServiceError;
use crate::facebook_signed_request::parse_signed_request;
use crate::facebook_tokens::FacebookTokenStore;
use crate::handlers::oauth::{CallbackQuery, ConnectRequest, ConnectResponse};
use crate::handlers::verification::VerificationQuery;
use crate::indexer::assert_profile_owner;
use crate::state::AppState;

#[derive(Debug, serde::Serialize)]
pub struct FacebookVerificationResponse {
    pub connected: bool,
    pub facebook_id: Option<String>,
    pub facebook_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct FacebookDataDeletionForm {
    pub signed_request: String,
}

#[derive(Debug, Deserialize)]
pub struct FacebookDeletionStatusQuery {
    pub code: Option<String>,
}

fn facebook_ready(state: &AppState) -> Result<(), ServiceError> {
    if state.facebook_oauth.is_none() || state.facebook_api.is_none() {
        return Err(ServiceError::Upstream(
            "Facebook Login is not configured — set FACEBOOK_APP_ID, FACEBOOK_APP_SECRET, FACEBOOK_CALLBACK_URL"
                .into(),
        ));
    }
    Ok(())
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

fn callback_response(headers: &HeaderMap, status: StatusCode, body: serde_json::Value) -> Response {
    if wants_json(headers) {
        return (status, Json(body)).into_response();
    }

    let (heading, detail, ok) = if status.is_success() {
        let name = body
            .get("facebook_name")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        (
            "Facebook connected".to_string(),
            if name.is_empty() {
                "Your Facebook account is linked. Return to the app.".to_string()
            } else {
                format!("{name} is linked. Return to the app.")
            },
            true,
        )
    } else {
        let err = body
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("Facebook connect failed.");
        ("Couldn’t connect Facebook".to_string(), err.to_string(), false)
    };

    let mut response = callback_html_page(
        if ok {
            "Facebook connected"
        } else {
            "Facebook connect failed"
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

pub async fn facebook_connect(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ConnectRequest>,
) -> Result<Json<ConnectResponse>, ServiceError> {
    facebook_ready(&state)?;
    let facebook_oauth = state.facebook_oauth.as_ref().expect("checked");

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

    let authorize_url = facebook_oauth.build_authorize_url(&request_id, &session.wallet_address)?;
    Ok(Json(ConnectResponse { authorize_url }))
}

pub async fn facebook_callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<CallbackQuery>,
) -> Response {
    match facebook_callback_inner(&state, query).await {
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
            callback_response(&headers, status, serde_json::json!({ "error": message }))
        }
    }
}

async fn facebook_callback_inner(
    state: &AppState,
    query: CallbackQuery,
) -> Result<serde_json::Value, ServiceError> {
    facebook_ready(state)?;
    let facebook_oauth = state.facebook_oauth.as_ref().expect("checked");
    let facebook_api = state.facebook_api.as_ref().expect("checked");

    let (code, state_jwt) = parse_facebook_callback_params(&query)?;
    let oauth_state = facebook_oauth.decode_state(&state_jwt)?;
    let short_lived = facebook_oauth.exchange_code(&code).await?;
    let token = match facebook_oauth
        .exchange_long_lived(&short_lived.access_token)
        .await
    {
        Ok(long) => long,
        Err(err) => {
            tracing::warn!(error = %err, "facebook long-lived exchange failed; using short-lived token");
            short_lived
        }
    };
    let me = facebook_api.get_me(&token.access_token).await?;
    let friend_ids = facebook_api.list_friend_ids(&token.access_token).await?;

    let mut redis = state.redis.clone();
    if let Some(existing) =
        FacebookTokenStore::wallet_for_facebook_id(&mut redis, &me.id).await?
    {
        if !existing.eq_ignore_ascii_case(&oauth_state.wallet_address) {
            return Err(ServiceError::conflict(
                "Facebook account already linked to another profile",
            ));
        }
    }

    if let Some(dripdrop) = &state.dripdrop {
        dripdrop
            .link_facebook(crate::dripdrop::FacebookLinkRequest {
                wallet_address: &oauth_state.wallet_address,
                facebook_id: &me.id,
                facebook_name: &me.name,
                friend_facebook_ids: &friend_ids,
            })
            .await?;
    } else {
        tracing::warn!("DRIPDROP_INTERNAL_URL unset — Facebook friends will not rank until configured");
    }

    FacebookTokenStore::save(
        &mut redis,
        &oauth_state.wallet_address,
        &state.config.oauth_state_secret,
        &token.access_token,
        token.expires_in,
        &me.id,
        &me.name,
    )
    .await?;

    Ok(serde_json::json!({
        "status": "connected",
        "facebook_id": me.id,
        "facebook_name": me.name,
    }))
}

pub fn parse_facebook_callback_params(
    query: &CallbackQuery,
) -> Result<(String, String), ServiceError> {
    if let Some(error) = &query.error {
        let detail = query.error_description.as_deref().unwrap_or("");
        return Err(ServiceError::bad_request(format!(
            "facebook oauth denied: {error}{}",
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
                "missing authorization code — start from POST /oauth/facebook/connect and approve Facebook Login",
            )
        })?;

    let state = query
        .state
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ServiceError::bad_request("missing oauth state"))?;

    Ok((code.to_string(), state.to_string()))
}

pub async fn get_facebook_verification(
    State(state): State<AppState>,
    Query(query): Query<VerificationQuery>,
) -> Result<Json<FacebookVerificationResponse>, ServiceError> {
    let mut redis = state.redis.clone();
    let record = FacebookTokenStore::get(&mut redis, &query.address).await?;
    Ok(Json(match record {
        Some(record) => FacebookVerificationResponse {
            connected: true,
            facebook_id: Some(record.facebook_id),
            facebook_name: Some(record.facebook_name).filter(|n| !n.is_empty()),
        },
        None => FacebookVerificationResponse {
            connected: false,
            facebook_id: None,
            facebook_name: None,
        },
    }))
}

pub async fn facebook_data_deletion(
    State(state): State<AppState>,
    Form(form): Form<FacebookDataDeletionForm>,
) -> Result<Json<serde_json::Value>, ServiceError> {
    facebook_ready(&state)?;
    let secret = state
        .config
        .facebook_app_secret
        .as_deref()
        .ok_or_else(|| ServiceError::Upstream("Facebook Login is not configured".into()))?;
    let parsed = parse_signed_request(&form.signed_request, secret)?;
    let facebook_id = parsed
        .user_id
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ServiceError::bad_request("signed_request missing user_id"))?;

    let mut redis = state.redis.clone();
    if let Some(wallet) =
        FacebookTokenStore::wallet_for_facebook_id(&mut redis, &facebook_id).await?
    {
        FacebookTokenStore::delete(&mut redis, &wallet, &facebook_id).await?;
    }
    if let Some(dripdrop) = &state.dripdrop {
        dripdrop.unlink_facebook(&facebook_id).await?;
    }

    let confirmation_code = format!("fbdel_{facebook_id}");
    let _: () = redis
        .set_ex(
            format!("facebook_deletion:{confirmation_code}"),
            facebook_id.as_str(),
            60 * 60 * 24 * 90,
        )
        .await
        .map_err(|e| ServiceError::Upstream(format!("redis set deletion receipt failed: {e}")))?;

    let status_url = format!(
        "{}/facebook/data-deletion?code={confirmation_code}",
        public_base_url(&state)
    );
    Ok(Json(serde_json::json!({
        "url": status_url,
        "confirmation_code": confirmation_code,
    })))
}

pub async fn facebook_data_deletion_status(
    State(state): State<AppState>,
    Query(query): Query<FacebookDeletionStatusQuery>,
) -> Html<String> {
    let code = query.code.unwrap_or_default();
    let mut redis = state.redis.clone();
    let found: Option<String> = if code.is_empty() {
        None
    } else {
        redis
            .get(format!("facebook_deletion:{code}"))
            .await
            .ok()
            .flatten()
    };
    let (heading, detail, ok) = if found.is_some() {
        (
            "Facebook data deleted",
            "We removed the Facebook link and friend edges for this request.",
            true,
        )
    } else {
        (
            "Deletion request not found",
            "If you just submitted a request, wait a moment and refresh. Otherwise contact support.",
            false,
        )
    };
    callback_html_page("Facebook data deletion", heading, detail, ok)
}

fn public_base_url(state: &AppState) -> String {
    state
        .config
        .facebook_callback_url
        .as_deref()
        .and_then(|url| url.split("/oauth/facebook/callback").next())
        .map(|s| s.trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "https://identity-verification.testnet.mysocial.network".into())
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
    fn parse_facebook_callback_params_success() {
        let result = parse_facebook_callback_params(&query(Some("abc"), Some("state")));
        assert_eq!(result.unwrap(), ("abc".into(), "state".into()));
    }

    #[test]
    fn parse_facebook_callback_params_denied() {
        let q = CallbackQuery {
            code: None,
            state: Some("state".into()),
            error: Some("access_denied".into()),
            error_description: Some("User cancelled".into()),
        };
        let err = parse_facebook_callback_params(&q).unwrap_err();
        assert!(matches!(
            err,
            ServiceError::BadRequest(msg) if msg.contains("facebook oauth denied: access_denied")
        ));
    }

}
