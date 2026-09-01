// Copyright (c) The Social Proof Foundation, LLC.
// SPDX-License-Identifier: Apache-2.0

use std::time::{SystemTime, UNIX_EPOCH};

use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::error::ServiceError;

const FB_DIALOG_URL: &str = "https://www.facebook.com/v21.0/dialog/oauth";
const FB_TOKEN_URL: &str = "https://graph.facebook.com/v21.0/oauth/access_token";
const FB_SCOPES: &str = "public_profile,user_friends";

#[derive(Debug, Serialize, Deserialize)]
pub struct FacebookOAuthStateClaims {
    pub profile_id: String,
    pub wallet_address: String,
    pub exp: u64,
}

#[derive(Debug, Deserialize)]
pub struct FacebookTokenResponse {
    pub access_token: String,
    #[serde(default)]
    pub token_type: Option<String>,
    #[serde(default)]
    pub expires_in: Option<u64>,
}

pub struct FacebookOAuthClient {
    app_id: String,
    app_secret: String,
    callback_url: String,
    state_secret: String,
    http: reqwest::Client,
    state_key: EncodingKey,
    state_validation: Validation,
}

impl FacebookOAuthClient {
    pub fn new(
        app_id: String,
        app_secret: String,
        callback_url: String,
        state_secret: String,
        http: reqwest::Client,
    ) -> Self {
        let state_key = EncodingKey::from_secret(state_secret.as_bytes());
        let mut state_validation = Validation::new(jsonwebtoken::Algorithm::HS256);
        state_validation.validate_exp = true;
        Self {
            app_id,
            app_secret,
            callback_url,
            state_secret,
            http,
            state_key,
            state_validation,
        }
    }

    pub fn build_authorize_url(
        &self,
        profile_id: &str,
        wallet_address: &str,
    ) -> Result<String, ServiceError> {
        let exp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            + 600;

        let state_jwt = encode(
            &Header::default(),
            &FacebookOAuthStateClaims {
                profile_id: profile_id.to_string(),
                wallet_address: wallet_address.to_lowercase(),
                exp,
            },
            &self.state_key,
        )
        .map_err(|e| ServiceError::Internal(e.into()))?;

        Ok(build_facebook_authorize_url(
            &self.app_id,
            &self.callback_url,
            &state_jwt,
        )?)
    }

    pub fn decode_state(&self, state: &str) -> Result<FacebookOAuthStateClaims, ServiceError> {
        let data = decode::<FacebookOAuthStateClaims>(
            state,
            &DecodingKey::from_secret(self.state_secret.as_bytes()),
            &self.state_validation,
        )
        .map_err(|e| ServiceError::bad_request(format!("invalid oauth state: {e}")))?;
        Ok(data.claims)
    }

    pub async fn exchange_code(&self, code: &str) -> Result<FacebookTokenResponse, ServiceError> {
        let mut url =
            Url::parse(FB_TOKEN_URL).map_err(|e| ServiceError::Internal(e.into()))?;
        {
            let mut qp = url.query_pairs_mut();
            qp.append_pair("client_id", &self.app_id);
            qp.append_pair("redirect_uri", &self.callback_url);
            qp.append_pair("client_secret", &self.app_secret);
            qp.append_pair("code", code);
        }

        let resp = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| ServiceError::Upstream(format!("facebook token exchange failed: {e}")))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ServiceError::Upstream(format!(
                "facebook token exchange error: {body}"
            )));
        }

        resp.json::<FacebookTokenResponse>()
            .await
            .map_err(|e| ServiceError::Upstream(format!("facebook token parse failed: {e}")))
    }

    pub async fn exchange_long_lived(
        &self,
        short_lived: &str,
    ) -> Result<FacebookTokenResponse, ServiceError> {
        let mut url =
            Url::parse(FB_TOKEN_URL).map_err(|e| ServiceError::Internal(e.into()))?;
        {
            let mut qp = url.query_pairs_mut();
            qp.append_pair("grant_type", "fb_exchange_token");
            qp.append_pair("client_id", &self.app_id);
            qp.append_pair("client_secret", &self.app_secret);
            qp.append_pair("fb_exchange_token", short_lived);
        }

        let resp = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| ServiceError::Upstream(format!("facebook long-lived exchange failed: {e}")))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ServiceError::Upstream(format!(
                "facebook long-lived exchange error: {body}"
            )));
        }

        resp.json::<FacebookTokenResponse>()
            .await
            .map_err(|e| ServiceError::Upstream(format!("facebook long-lived parse failed: {e}")))
    }
}

pub fn build_facebook_authorize_url(
    app_id: &str,
    callback_url: &str,
    state: &str,
) -> Result<String, ServiceError> {
    let mut url = Url::parse(FB_DIALOG_URL).map_err(|e| ServiceError::Internal(e.into()))?;
    {
        let mut qp = url.query_pairs_mut();
        qp.append_pair("client_id", app_id);
        qp.append_pair("redirect_uri", callback_url);
        qp.append_pair("state", state);
        qp.append_pair("scope", FB_SCOPES);
        qp.append_pair("response_type", "code");
    }
    Ok(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorize_url_includes_scopes_and_state() {
        let url = build_facebook_authorize_url(
            "123456",
            "https://example.com/oauth/facebook/callback",
            "state-jwt",
        )
        .unwrap();
        assert!(url.starts_with("https://www.facebook.com/v21.0/dialog/oauth?"));
        assert!(url.contains("client_id=123456"));
        assert!(url.contains("scope=public_profile%2Cuser_friends"));
        assert!(url.contains("state=state-jwt"));
        assert!(url.contains("response_type=code"));
    }
}
