// Copyright (c) The Social Proof Foundation, LLC.
// SPDX-License-Identifier: Apache-2.0

use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;

use crate::campaigns::share::process_pending_campaigns;
use crate::error::ServiceError;
use crate::state::AppState;

pub async fn process_pending_campaigns_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<crate::campaigns::share::ProcessSummary>, ServiceError> {
    let auth = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let expected = format!("Bearer {}", state.config.cron_secret);
    if auth != expected {
        return Err(ServiceError::unauthorized("invalid cron secret"));
    }

    let summary = process_pending_campaigns(&state).await?;
    Ok(Json(summary))
}
