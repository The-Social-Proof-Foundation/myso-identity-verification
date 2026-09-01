// Copyright (c) The Social Proof Foundation, LLC.
// SPDX-License-Identifier: Apache-2.0

mod campaigns;
mod facebook;
mod health;
mod oauth;
mod poc_claim;
mod social_graph;
mod verification;

use axum::response::{IntoResponse, Response};
use axum::Json;
use axum::http::StatusCode;

pub use campaigns::*;
pub use facebook::*;
pub use health::*;
pub use oauth::*;
pub use poc_claim::*;
pub use social_graph::*;
pub use verification::*;

use crate::error::ServiceError;

impl IntoResponse for ServiceError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            ServiceError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg.clone()),
            ServiceError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            ServiceError::Conflict(msg) => (StatusCode::CONFLICT, msg.clone()),
            ServiceError::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
            ServiceError::Upstream(msg) => (StatusCode::BAD_GATEWAY, msg.clone()),
            ServiceError::Internal(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()),
        };
        (status, Json(serde_json::json!({ "error": message }))).into_response()
    }
}
