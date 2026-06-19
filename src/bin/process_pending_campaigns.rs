// Copyright (c) The Social Proof Foundation, LLC.
// SPDX-License-Identifier: Apache-2.0

//! Railway Cron entrypoint — must exit when complete.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use myso_identity_verification::auth::SessionValidator;
use myso_identity_verification::config::Config;
use myso_identity_verification::indexer::IndexerClient;
use myso_identity_verification::oauth::x::XOAuthClient;
use myso_identity_verification::process_pending_campaigns;
use myso_identity_verification::relayer::Relayer;
use myso_identity_verification::state::AppState;
use myso_identity_verification::x_api::XApiClient;
use redis::Client as RedisClient;
use reqwest::Client;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = Arc::new(Config::from_env()?);
    let http = Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .context("build http client")?;

    let redis_client = RedisClient::open(config.redis_url.as_str()).context("redis client")?;
    let redis = redis_client
        .get_connection_manager()
        .await
        .context("redis connection manager")?;

    let relayer = Arc::new(Relayer::new(config.clone()).await.context("relayer")?);
    let relayer_address = relayer.address().to_string();

    let state = AppState {
        config: config.clone(),
        http: http.clone(),
        redis,
        sessions: Arc::new(SessionValidator::new(
            config.jwt_signing_key.clone(),
            config.mysocial_auth_issuer.clone(),
            config.mysocial_auth_jwks_uri.clone(),
        )),
        indexer: Arc::new(IndexerClient::new(
            config.myso_indexer_graphql_url.clone(),
            http.clone(),
            relayer_address,
        )),
        x_oauth: Arc::new(XOAuthClient::new(config.clone(), http.clone())),
        x_api: Arc::new(XApiClient::new(http, &config)),
        relayer,
    };

    let summary = process_pending_campaigns(&state)
        .await
        .context("process pending campaigns")?;

    info!(
        completed = summary.completed,
        failed = summary.failed,
        skipped = summary.skipped,
        "cron run finished"
    );

    Ok(())
}
