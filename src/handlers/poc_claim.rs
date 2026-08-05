// Copyright (c) The Social Proof Foundation, LLC.
// SPDX-License-Identifier: Apache-2.0

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::error::ServiceError;
use crate::poc_claim::{build_attestation, validate_handle_matches_identity_hash};
use crate::poc_tokens::{PocClaimTokenRecord, PocClaimTokenStore};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct PocClaimConnectRequest {
    pub identity_hash: String,
    pub beneficiary_id: String,
    pub wallet: String,
}

#[derive(Debug, Serialize)]
pub struct PocClaimConnectResponse {
    pub authorize_url: String,
}

#[derive(Debug, Deserialize)]
pub struct PocClaimAttestRequest {
    pub identity_hash: String,
    pub beneficiary_id: String,
    pub wallet: String,
}

#[derive(Debug, Serialize)]
pub struct PocClaimAttestResponse {
    pub attested_x_handle: String,
    pub identity_hash: String,
    pub evidence_hash: String,
    pub verifier: &'static str,
    pub verified_at: i64,
}

#[derive(Debug, Deserialize)]
pub struct PocClaimStatusQuery {
    pub identity_hash: String,
    pub wallet: String,
}

#[derive(Debug, Serialize)]
pub struct PocClaimStatusResponse {
    pub oauth_required: bool,
    pub oauth_complete: bool,
    pub attested_x_handle: Option<String>,
    pub identity_hash: String,
    pub wallet: String,
    pub authorize_url: Option<String>,
}

fn ensure_poc_claim_enabled(state: &AppState) -> Result<(), ServiceError> {
    if !state.config.allow_poc_claim_attestation {
        return Err(ServiceError::bad_request(
            "PoC claim attestation is disabled on this deployment",
        ));
    }
    Ok(())
}

async fn authorize_session_or_service(
    state: &AppState,
    headers: &HeaderMap,
    wallet: &str,
) -> Result<Option<String>, ServiceError> {
    if let Some(secret) = headers
        .get("x-poc-service-secret")
        .and_then(|v| v.to_str().ok())
    {
        if state
            .config
            .poc_service_secret
            .as_deref()
            .is_some_and(|expected| secret == expected)
        {
            return Ok(None);
        }
        return Err(ServiceError::Unauthorized(
            "invalid X-PoC-Service-Secret".into(),
        ));
    }

    let session = state.sessions.validate_headers(headers).await?;
    if session.wallet_address.to_lowercase() != wallet.to_lowercase() {
        return Err(ServiceError::bad_request("wallet does not match session"));
    }
    Ok(Some(session.wallet_address))
}

pub async fn poc_claim_connect(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<PocClaimConnectRequest>,
) -> Result<Json<PocClaimConnectResponse>, ServiceError> {
    ensure_poc_claim_enabled(&state)?;
    let _session_wallet = authorize_session_or_service(&state, &headers, &body.wallet).await?;

    let (authorize_url, _verifier) = state.x_oauth.build_authorize_url_for_poc_claim(
        &body.wallet,
        &body.identity_hash,
        &body.beneficiary_id,
    )?;

    Ok(Json(PocClaimConnectResponse { authorize_url }))
}

pub async fn poc_claim_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<PocClaimStatusQuery>,
) -> Result<Json<PocClaimStatusResponse>, ServiceError> {
    ensure_poc_claim_enabled(&state)?;
    let _ = authorize_session_or_service(&state, &headers, &query.wallet).await?;

    let mut redis = state.redis.clone();
    let record = PocClaimTokenStore::get(&mut redis, &query.wallet, &query.identity_hash).await?;

    Ok(Json(PocClaimStatusResponse {
        oauth_required: record.is_none(),
        oauth_complete: record.is_some(),
        attested_x_handle: record.as_ref().map(|r| r.x_username.clone()),
        identity_hash: query.identity_hash,
        wallet: query.wallet,
        authorize_url: None,
    }))
}

pub async fn poc_claim_attest(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<PocClaimAttestRequest>,
) -> Result<Json<PocClaimAttestResponse>, ServiceError> {
    ensure_poc_claim_enabled(&state)?;
    let _ = authorize_session_or_service(&state, &headers, &body.wallet).await?;

    let mut redis = state.redis.clone();
    let record = PocClaimTokenStore::get(&mut redis, &body.wallet, &body.identity_hash)
        .await?
        .ok_or_else(|| {
            ServiceError::bad_request(
                "X OAuth not completed for this claim — start POST /oauth/x/connect-for-poc-claim",
            )
        })?;

    if record.beneficiary_id != body.beneficiary_id {
        return Err(ServiceError::bad_request("beneficiary_id mismatch"));
    }

    let access_token =
        get_valid_poc_claim_access_token(&state, &body.wallet, &body.identity_hash, &record).await?;
    let x_user = state.x_api.get_authenticated_user(&access_token).await?;
    let handle =
        validate_handle_matches_identity_hash(&x_user.username, &body.identity_hash)?;

    let (evidence_hash, verified_at) = build_attestation(
        &body.beneficiary_id,
        &body.identity_hash,
        &handle,
        &body.wallet,
    )?;

    Ok(Json(PocClaimAttestResponse {
        attested_x_handle: handle,
        identity_hash: body.identity_hash,
        evidence_hash,
        verifier: "myso-identity-verification",
        verified_at,
    }))
}

pub async fn complete_poc_claim_oauth(
    state: &AppState,
    oauth_state: &crate::oauth::x::OAuthStateClaims,
    access_token: &str,
    refresh_token: &str,
    expires_in: Option<u64>,
    x_user_id: &str,
    x_username: &str,
) -> Result<Json<serde_json::Value>, ServiceError> {
    let identity_hash = oauth_state.identity_hash.as_deref().ok_or_else(|| {
        ServiceError::bad_request("missing identity_hash in poc claim oauth state")
    })?;
    let beneficiary_id = oauth_state.beneficiary_id.as_deref().ok_or_else(|| {
        ServiceError::bad_request("missing beneficiary_id in poc claim oauth state")
    })?;

    validate_handle_matches_identity_hash(x_username, identity_hash)?;

    let mut redis = state.redis.clone();
    PocClaimTokenStore::save(
        &mut redis,
        &oauth_state.wallet_address,
        identity_hash,
        &state.config.oauth_state_secret,
        access_token,
        refresh_token,
        expires_in,
        x_user_id,
        x_username,
        beneficiary_id,
    )
    .await?;

    Ok(Json(serde_json::json!({
        "status": "oauth_complete",
        "x_username": x_username,
        "identity_hash": identity_hash,
    })))
}

async fn get_valid_poc_claim_access_token(
    state: &AppState,
    wallet: &str,
    identity_hash: &str,
    record: &PocClaimTokenRecord,
) -> Result<String, ServiceError> {
    use chrono::Duration as ChronoDuration;

    let needs_refresh = Utc::now() + ChronoDuration::seconds(60) >= record.expires_at;
    if !needs_refresh {
        return Ok(record.access_token.clone());
    }

    let refresh_token =
        crate::x_tokens::decrypt_token(&record.refresh_token_encrypted, &state.config.oauth_state_secret)?;
    let token_resp = state.x_oauth.refresh_access_token(&refresh_token).await?;
    let new_refresh = token_resp
        .refresh_token
        .as_deref()
        .unwrap_or(refresh_token.as_str());

    let mut redis = state.redis.clone();
    PocClaimTokenStore::save(
        &mut redis,
        wallet,
        identity_hash,
        &state.config.oauth_state_secret,
        &token_resp.access_token,
        new_refresh,
        token_resp.expires_in,
        &record.x_user_id,
        &record.x_username,
        &record.beneficiary_id,
    )
    .await?;

    Ok(token_resp.access_token)
}

use chrono::Utc;
