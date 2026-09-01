// Copyright (c) The Social Proof Foundation, LLC.
// SPDX-License-Identifier: Apache-2.0

use std::env;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};

pub const MYSO_SOCIAL_PACKAGE_ID: &str = "0x50c1";

#[derive(Debug, Clone)]
pub struct BadgeAssets {
    pub verified_x_description: String,
    pub verified_x_media_url: String,
    pub verified_x_icon_url: String,
    pub early_adopter_description: String,
    pub early_adopter_media_url: String,
    pub early_adopter_icon_url: String,
    pub ambassador_description: String,
    pub ambassador_media_url: String,
    pub ambassador_icon_url: String,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub port: u16,
    pub allowed_origins: Vec<String>,
    pub redis_url: String,
    pub oauth_state_secret: String,
    pub jwt_signing_key: Option<String>,
    pub mysocial_auth_issuer: Option<String>,
    pub mysocial_auth_jwks_uri: Option<String>,
    pub x_client_id: String,
    pub x_client_secret: String,
    pub x_callback_url: String,
    pub facebook_app_id: Option<String>,
    pub facebook_app_secret: Option<String>,
    pub facebook_callback_url: Option<String>,
    pub dripdrop_internal_url: Option<String>,
    pub dripdrop_internal_api_key: Option<String>,
    pub share_campaign_check_delay_hours: u32,
    pub myso_rpc_url: String,
    pub myso_social_package_id: String,
    pub ecosystem_badge_admin_cap_id: String,
    pub relayer_private_key_hex: String,
    pub myso_indexer_graphql_url: String,
    pub early_access_ends_at: Option<DateTime<Utc>>,
    pub badge_assets: BadgeAssets,
    pub profile_url_template: String,
    pub allow_poc_claim_attestation: bool,
    pub poc_service_secret: Option<String>,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        dotenvy::dotenv().ok();

        let early_access_ends_at = env::var("EARLY_ACCESS_ENDS_AT")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(|s| {
                DateTime::parse_from_rfc3339(s.trim())
                    .map(|dt| dt.with_timezone(&Utc))
                    .or_else(|_| {
                        s.trim()
                            .parse::<i64>()
                            .map(|ms| DateTime::from_timestamp_millis(ms).unwrap_or_else(Utc::now))
                            .map_err(|_| anyhow::anyhow!("invalid EARLY_ACCESS_ENDS_AT"))
                    })
            })
            .transpose()
            .context("EARLY_ACCESS_ENDS_AT parse error")?;

        Ok(Config {
            port: env::var("PORT")
                .unwrap_or_else(|_| "3000".into())
                .parse()
                .context("invalid PORT")?,
            allowed_origins: parse_csv(env::var("ALLOWED_ORIGINS").unwrap_or_else(|_| {
                "https://mysocial.network,http://localhost:3000".into()
            })),
            redis_url: env::var("REDIS_URL").context("REDIS_URL not set")?,
            oauth_state_secret: env::var("OAUTH_STATE_SECRET")
                .or_else(|_| env::var("JWT_SIGNING_KEY"))
                .context("OAUTH_STATE_SECRET or JWT_SIGNING_KEY required")?,
            jwt_signing_key: env::var("JWT_SIGNING_KEY").ok(),
            mysocial_auth_issuer: env::var("MYSOCIAL_AUTH_ISSUER").ok(),
            mysocial_auth_jwks_uri: env::var("MYSOCIAL_AUTH_JWKS_URI").ok(),
            x_client_id: env::var("X_CLIENT_ID").context("X_CLIENT_ID not set")?,
            x_client_secret: env::var("X_CLIENT_SECRET").context("X_CLIENT_SECRET not set")?,
            x_callback_url: env::var("X_CALLBACK_URL").context("X_CALLBACK_URL not set")?,
            facebook_app_id: optional_env("FACEBOOK_APP_ID"),
            facebook_app_secret: optional_env("FACEBOOK_APP_SECRET"),
            facebook_callback_url: optional_env("FACEBOOK_CALLBACK_URL"),
            dripdrop_internal_url: optional_env("DRIPDROP_INTERNAL_URL"),
            dripdrop_internal_api_key: optional_env("DRIPDROP_INTERNAL_API_KEY")
                .or_else(|| optional_env("INTERNAL_API_KEY")),
            share_campaign_check_delay_hours: env::var("SHARE_CAMPAIGN_CHECK_DELAY_HOURS")
                .unwrap_or_else(|_| "24".into())
                .parse()
                .context("invalid SHARE_CAMPAIGN_CHECK_DELAY_HOURS")?,
            myso_rpc_url: env::var("MYSO_RPC_URL").context("MYSO_RPC_URL not set")?,
            myso_social_package_id: MYSO_SOCIAL_PACKAGE_ID.to_string(),
            ecosystem_badge_admin_cap_id: env::var("ECOSYSTEM_BADGE_ADMIN_CAP_ID")
                .context("ECOSYSTEM_BADGE_ADMIN_CAP_ID not set")?,
            relayer_private_key_hex: env::var("RELAYER_PRIVATE_KEY")
                .context("RELAYER_PRIVATE_KEY not set")?,
            myso_indexer_graphql_url: env::var("MYSO_INDEXER_GRAPHQL_URL")
                .context("MYSO_INDEXER_GRAPHQL_URL not set")?,
            early_access_ends_at,
            profile_url_template: env::var("PROFILE_URL_TEMPLATE")
                .unwrap_or_else(|_| "https://mysocial.network/@{username}".into()),
            allow_poc_claim_attestation: env::var("ALLOW_POC_CLAIM_ATTESTATION")
                .unwrap_or_else(|_| "true".into())
                .parse()
                .unwrap_or(true),
            poc_service_secret: env::var("POC_SERVICE_SECRET").ok().filter(|s| !s.trim().is_empty()),
            badge_assets: BadgeAssets {
                verified_x_description: env_or(
                    "BADGE_VERIFIED_X_DESCRIPTION",
                    "Verified X account linked to this profile",
                ),
                verified_x_media_url: env_or(
                    "BADGE_VERIFIED_X_MEDIA_URL",
                    "https://assets.mysocial.network/badges/verified-x.png",
                ),
                verified_x_icon_url: env_or(
                    "BADGE_VERIFIED_X_ICON_URL",
                    "https://assets.mysocial.network/badges/verified-x-icon.png",
                ),
                early_adopter_description: env_or(
                    "BADGE_EARLY_ADOPTER_DESCRIPTION",
                    "Joined and promoted MySocial during Early Access",
                ),
                early_adopter_media_url: env_or(
                    "BADGE_EARLY_ADOPTER_MEDIA_URL",
                    "https://assets.mysocial.network/badges/early-adopter.png",
                ),
                early_adopter_icon_url: env_or(
                    "BADGE_EARLY_ADOPTER_ICON_URL",
                    "https://assets.mysocial.network/badges/early-adopter-icon.png",
                ),
                ambassador_description: env_or(
                    "BADGE_AMBASSADOR_DESCRIPTION",
                    "Active MySocial ecosystem ambassador",
                ),
                ambassador_media_url: env_or(
                    "BADGE_AMBASSADOR_MEDIA_URL",
                    "https://assets.mysocial.network/badges/ambassador.png",
                ),
                ambassador_icon_url: env_or(
                    "BADGE_AMBASSADOR_ICON_URL",
                    "https://assets.mysocial.network/badges/ambassador-icon.png",
                ),
            },
        })
    }

    pub fn facebook_enabled(&self) -> bool {
        self.facebook_app_id.as_deref().is_some_and(|s| !s.is_empty())
            && self.facebook_app_secret.as_deref().is_some_and(|s| !s.is_empty())
            && self
                .facebook_callback_url
                .as_deref()
                .is_some_and(|s| !s.is_empty())
    }

    pub fn is_early_access_active(&self) -> bool {
        match self.early_access_ends_at {
            Some(end) => Utc::now() < end,
            None => true,
        }
    }
}

fn env_or(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_string())
}

fn optional_env(key: &str) -> Option<String> {
    env::var(key).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn parse_csv(raw: String) -> Vec<String> {
    raw.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}
