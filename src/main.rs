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
use tokio_util::sync::CancellationToken;
use tower_http::cors::CorsLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use myso_identity_verification::auth::SessionValidator;
use myso_identity_verification::config::Config;
use myso_identity_verification::handlers;
use myso_identity_verification::indexer::IndexerClient;
use myso_identity_verification::dripdrop::DripdropInternalClient;
use myso_identity_verification::facebook_api::FacebookApiClient;
use myso_identity_verification::oauth::facebook::FacebookOAuthClient;
use myso_identity_verification::oauth::x::XOAuthClient;
use myso_identity_verification::relayer::Relayer;
use myso_identity_verification::run_scheduler;
use myso_identity_verification::state::AppState;
use myso_identity_verification::x_api::XApiClient;

const SCHEDULER_SHUTDOWN_TIMEOUT_SECS: u64 = 5;

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

    match relayer.chain_identifier().await {
        Ok(chain_id) => info!(
            myso_rpc_url = %config.myso_rpc_url,
            indexer_graphql = %config.myso_indexer_graphql_url,
            chain_id = %chain_id,
            "Connected to MySo fullnode"
        ),
        Err(err) => tracing::warn!(
            myso_rpc_url = %config.myso_rpc_url,
            error = %err,
            "Failed to read chain identifier from MYSO_RPC_URL"
        ),
    }

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
        x_api: Arc::new(XApiClient::new(http.clone(), &config)),
        facebook_oauth: if config.facebook_enabled() {
            Some(Arc::new(FacebookOAuthClient::new(
                config.facebook_app_id.clone().unwrap(),
                config.facebook_app_secret.clone().unwrap(),
                config.facebook_callback_url.clone().unwrap(),
                config.oauth_state_secret.clone(),
                http.clone(),
            )))
        } else {
            None
        },
        facebook_api: if config.facebook_enabled() {
            Some(Arc::new(FacebookApiClient::new(http.clone())))
        } else {
            None
        },
        dripdrop: config.dripdrop_internal_url.clone().map(|url| {
            Arc::new(DripdropInternalClient::new(
                http,
                url,
                config.dripdrop_internal_api_key.clone(),
            ))
        }),
        relayer,
    };

    let shutdown = CancellationToken::new();
    let scheduler_shutdown = shutdown.clone();
    let scheduler_handle = tokio::spawn(run_scheduler(state.clone(), scheduler_shutdown));

    let app = build_router(state, &config.allowed_origins);

    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    info!("Listening on {addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    let shutdown_for_serve = shutdown.clone();
    axum::serve(listener, app)
        .with_graceful_shutdown(wait_for_shutdown(shutdown_for_serve))
        .await?;

    match tokio::time::timeout(
        Duration::from_secs(SCHEDULER_SHUTDOWN_TIMEOUT_SECS),
        scheduler_handle,
    )
    .await
    {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            if !e.is_cancelled() {
                return Err(e.into());
            }
        }
        Err(_) => {
            info!(
                "Scheduler shutdown timed out after {SCHEDULER_SHUTDOWN_TIMEOUT_SECS}s, aborting"
            );
        }
    }

    info!("Shutdown complete");
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
        .route("/oauth/facebook/connect", post(handlers::facebook_connect))
        .route("/oauth/facebook/callback", get(handlers::facebook_callback))
        .route("/verification/facebook", get(handlers::get_facebook_verification))
        .route("/facebook/data-deletion", post(handlers::facebook_data_deletion).get(handlers::facebook_data_deletion_status))
        .route("/oauth/x/connect-for-poc-claim", post(handlers::poc_claim_connect))
        .route("/verification/poc/status", get(handlers::poc_claim_status))
        .route("/verification/poc/attest-for-claim", post(handlers::poc_claim_attest))
        .route("/verification/x", get(handlers::get_x_verification))
        .route("/social-graph/x/matches", get(handlers::x_matches))
        .route("/campaigns/share/start", post(handlers::share_start))
        .route("/campaigns/share/status", get(handlers::share_status))
        .with_state(state)
        .layer(RequestBodyLimitLayer::new(1024 * 1024))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
}

async fn wait_for_shutdown(shutdown: CancellationToken) {
    let signal_name = wait_for_shutdown_signal().await;
    info!(signal = signal_name, "Shutdown signal received");
    shutdown.cancel();
}

async fn wait_for_shutdown_signal() -> &'static str {
    #[cfg(unix)]
    {
        use signal::unix::{signal, SignalKind};

        let mut sigterm = signal(SignalKind::terminate()).expect("register SIGTERM handler");

        tokio::select! {
            _ = signal::ctrl_c() => "SIGINT",
            _ = sigterm.recv() => "SIGTERM",
        }
    }

    #[cfg(not(unix))]
    {
        let _ = signal::ctrl_c().await;
        "SIGINT"
    }
}
