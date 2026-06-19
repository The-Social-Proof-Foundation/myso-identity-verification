// Copyright (c) The Social Proof Foundation, LLC.
// SPDX-License-Identifier: Apache-2.0

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use axum::http::{header, Method};
use axum::routing::{get, post};
use axum::Router;
use redis::Client as RedisClient;
use reqwest::Client;
use tokio::signal;
use tower_http::cors::CorsLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use myso_identity_verification::auth::SessionValidator;
use myso_identity_verification::config::Config;
use myso_identity_verification::handlers;
use myso_identity_verification::indexer::IndexerClient;
use myso_identity_verification::oauth::x::XOAuthClient;
use myso_identity_verification::relayer::Relayer;
use myso_identity_verification::state::AppState;
use myso_identity_verification::x_api::XApiClient;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Starting MySo Identity Verification API");

    let config = Arc::new(Config::from_env()?);
    let http = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("build http client")?;

    let redis_client = RedisClient::open(config.redis_url.as_str()).context("redis client")?;
    let redis = redis_client
        .get_connection_manager()
        .await
        .context("redis connection manager")?;

    let relayer = Arc::new(
        Relayer::new(config.clone())
            .await
            .context("initialize relayer")?,
    );
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

    let app = build_router(state, &config.allowed_origins);

    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    info!("Listening on {addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

fn build_router(state: AppState, allowed_origins: &[String]) -> Router {
    let origins: Vec<_> = allowed_origins
        .iter()
        .filter_map(|o| o.trim().parse().ok())
        .collect();

    let cors = if origins.is_empty() {
        CorsLayer::new()
            .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
            .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION])
    } else {
        CorsLayer::new()
            .allow_origin(origins)
            .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
            .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION])
    };

    Router::new()
        .route("/health", get(handlers::health_check))
        .route("/oauth/x/connect", post(handlers::x_connect))
        .route("/oauth/x/callback", get(handlers::x_callback))
        .route("/verification/x", get(handlers::get_x_verification))
        .route("/social-graph/x/matches", get(handlers::x_matches))
        .route("/campaigns/share/start", post(handlers::share_start))
        .route("/campaigns/share/status", get(handlers::share_status))
        .route(
            "/internal/cron/process-pending-campaigns",
            post(handlers::process_pending_campaigns_handler),
        )
        .with_state(state)
        .layer(RequestBodyLimitLayer::new(1024 * 1024))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
}

async fn shutdown_signal() {
    let _ = signal::ctrl_c().await;
    info!("Shutdown signal received");
}
