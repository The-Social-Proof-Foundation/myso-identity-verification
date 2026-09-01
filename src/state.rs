// Copyright (c) The Social Proof Foundation, LLC.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use redis::aio::ConnectionManager;
use reqwest::Client;

use crate::auth::SessionValidator;
use crate::config::Config;
use crate::dripdrop::DripdropInternalClient;
use crate::facebook_api::FacebookApiClient;
use crate::indexer::IndexerClient;
use crate::oauth::facebook::FacebookOAuthClient;
use crate::oauth::x::XOAuthClient;
use crate::relayer::Relayer;
use crate::x_api::XApiClient;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub http: Client,
    pub redis: ConnectionManager,
    pub sessions: Arc<SessionValidator>,
    pub indexer: Arc<IndexerClient>,
    pub x_oauth: Arc<XOAuthClient>,
    pub x_api: Arc<XApiClient>,
    pub facebook_oauth: Option<Arc<FacebookOAuthClient>>,
    pub facebook_api: Option<Arc<FacebookApiClient>>,
    pub dripdrop: Option<Arc<DripdropInternalClient>>,
    pub relayer: Arc<Relayer>,
}
