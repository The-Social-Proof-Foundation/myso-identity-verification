// Copyright (c) The Social Proof Foundation, LLC.
// SPDX-License-Identifier: Apache-2.0

use reqwest::Client;
use serde::Serialize;

use crate::error::ServiceError;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FacebookLinkRequest<'a> {
    pub wallet_address: &'a str,
    pub facebook_id: &'a str,
    pub facebook_name: &'a str,
    pub friend_facebook_ids: &'a [String],
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FacebookUnlinkRequest<'a> {
    pub facebook_id: &'a str,
}

pub struct DripdropInternalClient {
    http: Client,
    base_url: String,
    api_key: Option<String>,
}

impl DripdropInternalClient {
    pub fn new(http: Client, base_url: String, api_key: Option<String>) -> Self {
        Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
        }
    }

    pub async fn link_facebook(&self, body: FacebookLinkRequest<'_>) -> Result<(), ServiceError> {
        self.post_json("/internal/user/facebook/link", &body)
            .await
    }

    pub async fn unlink_facebook(&self, facebook_id: &str) -> Result<(), ServiceError> {
        self.post_json(
            "/internal/user/facebook/unlink",
            &FacebookUnlinkRequest { facebook_id },
        )
        .await
    }

    async fn post_json<T: Serialize>(&self, path: &str, body: &T) -> Result<(), ServiceError> {
        let url = format!("{}{path}", self.base_url);
        let mut req = self.http.post(url).json(body);
        if let Some(key) = &self.api_key {
            req = req.header("x-internal-api-key", key);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| ServiceError::Upstream(format!("dripdrop facebook request failed: {e}")))?;
        let status = resp.status();
        if status.as_u16() == 409 {
            let message = resp
                .text()
                .await
                .ok()
                .and_then(|t| {
                    serde_json::from_str::<serde_json::Value>(&t)
                        .ok()
                        .and_then(|v| {
                            v.get("error")
                                .or_else(|| v.get("message"))
                                .and_then(|m| m.as_str())
                                .map(|s| s.to_string())
                        })
                })
                .unwrap_or_else(|| "Facebook account already linked to another wallet".into());
            return Err(ServiceError::conflict(message));
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ServiceError::Upstream(format!(
                "dripdrop facebook {path} error {status}: {body}"
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_body_uses_camel_case() {
        let friends = vec!["11".to_string()];
        let body = FacebookLinkRequest {
            wallet_address: "0xabc",
            facebook_id: "99",
            facebook_name: "Ada",
            friend_facebook_ids: &friends,
        };
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["walletAddress"], "0xabc");
        assert_eq!(json["facebookId"], "99");
        assert_eq!(json["friendFacebookIds"][0], "11");
    }
}
