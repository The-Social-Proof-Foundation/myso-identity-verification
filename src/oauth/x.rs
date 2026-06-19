// Copyright (c) The Social Proof Foundation, LLC.
// SPDX-License-Identifier: Apache-2.0

use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use jsonwebtoken::{encode, DecodingKey, EncodingKey, Header, Validation};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

use crate::config::Config;
use crate::error::ServiceError;

const X_AUTH_URL: &str = "https://twitter.com/i/oauth2/authorize";
const X_TOKEN_URL: &str = "https://api.twitter.com/2/oauth2/token";
const X_SCOPES: &str = "tweet.read users.read follows.read offline.access";

#[derive(Debug, Serialize, Deserialize)]
pub struct OAuthStateClaims {
    pub profile_id: String,
    pub wallet_address: String,
    pub code_verifier: String,
    pub exp: u64,
}

#[derive(Debug, Deserialize)]
pub struct XTokenResponse {
    pub access_token: String,
    pub token_type: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_in: Option<u64>,
}

pub struct XOAuthClient {
    config: std::sync::Arc<Config>,
    http: reqwest::Client,
    state_key: EncodingKey,
    state_validation: Validation,
}

impl XOAuthClient {
    pub fn new(config: std::sync::Arc<Config>, http: reqwest::Client) -> Self {
        let state_key = EncodingKey::from_secret(config.oauth_state_secret.as_bytes());
        let mut state_validation = Validation::new(jsonwebtoken::Algorithm::HS256);
        state_validation.validate_exp = true;
        Self {
            config,
            http,
            state_key,
            state_validation,
        }
    }

    pub fn build_authorize_url(
        &self,
        profile_id: &str,
        wallet_address: &str,
    ) -> Result<(String, String), ServiceError> {
        let code_verifier = generate_code_verifier();
        let code_challenge = code_challenge_s256(&code_verifier);
        let exp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            + 600;

        let state_jwt = encode(
            &Header::default(),
            &OAuthStateClaims {
                profile_id: profile_id.to_string(),
                wallet_address: wallet_address.to_lowercase(),
                code_verifier: code_verifier.clone(),
                exp,
            },
            &self.state_key,
        )
        .map_err(|e| ServiceError::Internal(e.into()))?;

        let mut url = Url::parse(X_AUTH_URL).map_err(|e| ServiceError::Internal(e.into()))?;
        {
            let mut qp = url.query_pairs_mut();
            qp.append_pair("response_type", "code");
            qp.append_pair("client_id", &self.config.x_client_id);
            qp.append_pair("redirect_uri", &self.config.x_callback_url);
            qp.append_pair("scope", X_SCOPES);
            qp.append_pair("state", &state_jwt);
            qp.append_pair("code_challenge", &code_challenge);
            qp.append_pair("code_challenge_method", "S256");
        }

        Ok((url.to_string(), code_verifier))
    }

    pub fn decode_state(&self, state: &str) -> Result<OAuthStateClaims, ServiceError> {
        use jsonwebtoken::decode;
        let data = decode::<OAuthStateClaims>(
            state,
            &DecodingKey::from_secret(self.config.oauth_state_secret.as_bytes()),
            &self.state_validation,
        )
        .map_err(|e| ServiceError::bad_request(format!("invalid oauth state: {e}")))?;
        Ok(data.claims)
    }

    pub async fn exchange_code(
        &self,
        code: &str,
        code_verifier: &str,
    ) -> Result<XTokenResponse, ServiceError> {
        let params = [
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", self.config.x_callback_url.as_str()),
            ("code_verifier", code_verifier),
            ("client_id", self.config.x_client_id.as_str()),
        ];

        let resp = self
            .http
            .post(X_TOKEN_URL)
            .basic_auth(&self.config.x_client_id, Some(&self.config.x_client_secret))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .form(&params)
            .send()
            .await
            .map_err(|e| ServiceError::Upstream(format!("x token exchange failed: {e}")))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ServiceError::Upstream(format!(
                "x token exchange error: {body}"
            )));
        }

        resp.json::<XTokenResponse>()
            .await
            .map_err(|e| ServiceError::Upstream(format!("x token parse failed: {e}")))
    }

    pub async fn refresh_access_token(
        &self,
        refresh_token: &str,
    ) -> Result<XTokenResponse, ServiceError> {
        let params = [
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", self.config.x_client_id.as_str()),
        ];

        let resp = self
            .http
            .post(X_TOKEN_URL)
            .basic_auth(&self.config.x_client_id, Some(&self.config.x_client_secret))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .form(&params)
            .send()
            .await
            .map_err(|e| ServiceError::Upstream(format!("x token refresh failed: {e}")))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ServiceError::Upstream(format!(
                "x token refresh error: {body}"
            )));
        }

        resp.json::<XTokenResponse>()
            .await
            .map_err(|e| ServiceError::Upstream(format!("x token refresh parse failed: {e}")))
    }
}

fn generate_code_verifier() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn code_challenge_s256(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}
