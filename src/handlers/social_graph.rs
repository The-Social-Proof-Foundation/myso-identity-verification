// Copyright (c) The Social Proof Foundation, LLC.
// SPDX-License-Identifier: Apache-2.0

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::Deserialize;

use crate::error::ServiceError;
use crate::social_graph::{find_x_matches, SocialGraphMatch};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct MatchesQuery {
    pub address: String,
}

pub async fn x_matches(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<MatchesQuery>,
) -> Result<Json<Vec<SocialGraphMatch>>, ServiceError> {
    let session = state.sessions.validate_headers(&headers).await?;
    if !session.wallet_address.eq_ignore_ascii_case(&query.address) {
        return Err(ServiceError::unauthorized("address does not match session"));
    }

    let matches = find_x_matches(&state, &query.address).await?;

    Ok(Json(matches))
}
