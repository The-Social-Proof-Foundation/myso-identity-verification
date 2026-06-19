// Copyright (c) The Social Proof Foundation, LLC.
// SPDX-License-Identifier: Apache-2.0

use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;

use crate::config::Config;
use crate::error::ServiceError;

#[derive(Debug, Clone, Deserialize)]
pub struct XUser {
    pub id: String,
    pub username: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct XTweet {
    pub id: String,
    pub text: String,
    pub author_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub struct XApiClient {
    http: Client,
}

impl XApiClient {
    pub fn new(http: Client, _config: &Config) -> Self {
        Self { http }
    }

    pub async fn get_authenticated_user(&self, access_token: &str) -> Result<XUser, ServiceError> {
        #[derive(Deserialize)]
        struct Resp {
            data: XUser,
        }

        let resp = self
            .http
            .get("https://api.twitter.com/2/users/me")
            .bearer_auth(access_token)
            .query(&[("user.fields", "username,name")])
            .send()
            .await
            .map_err(|e| ServiceError::Upstream(format!("x users/me failed: {e}")))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ServiceError::Upstream(format!("x users/me error: {body}")));
        }

        resp.json::<Resp>()
            .await
            .map(|r| r.data)
            .map_err(|e| ServiceError::Upstream(format!("x users/me parse: {e}")))
    }

    pub async fn get_tweet(
        &self,
        tweet_ref: &str,
        access_token: &str,
    ) -> Result<XTweet, ServiceError> {
        let tweet_id = parse_tweet_id(tweet_ref)?;

        #[derive(Deserialize)]
        struct TweetData {
            id: String,
            text: String,
            author_id: Option<String>,
            created_at: String,
        }
        #[derive(Deserialize)]
        struct Resp {
            data: TweetData,
        }

        let url = format!("https://api.twitter.com/2/tweets/{tweet_id}");
        let resp = self
            .http
            .get(&url)
            .bearer_auth(access_token)
            .query(&[(
                "tweet.fields",
                "created_at,author_id,text",
            )])
            .send()
            .await
            .map_err(|e| ServiceError::Upstream(format!("x tweet fetch failed: {e}")))?;

        if resp.status().as_u16() == 404 {
            return Err(ServiceError::not_found("tweet not found"));
        }
        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ServiceError::Upstream(format!("x tweet error: {body}")));
        }

        let parsed = resp
            .json::<Resp>()
            .await
            .map_err(|e| ServiceError::Upstream(format!("x tweet parse: {e}")))?;

        let created_at = DateTime::parse_from_rfc3339(&parsed.data.created_at)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|e| ServiceError::Upstream(format!("tweet created_at parse: {e}")))?;

        Ok(XTweet {
            id: parsed.data.id,
            text: parsed.data.text,
            author_id: parsed.data.author_id,
            created_at,
        })
    }

    pub async fn get_following_usernames(
        &self,
        user_id: &str,
        access_token: &str,
        max_results: u32,
    ) -> Result<Vec<String>, ServiceError> {
        #[derive(Deserialize)]
        struct UserRow {
            username: String,
        }
        #[derive(Deserialize)]
        struct Resp {
            data: Option<Vec<UserRow>>,
        }

        let resp = self
            .http
            .get(format!("https://api.twitter.com/2/users/{user_id}/following"))
            .bearer_auth(access_token)
            .query(&[
                ("max_results", max_results.min(1000).to_string()),
                ("user.fields", "username".into()),
            ])
            .send()
            .await
            .map_err(|e| ServiceError::Upstream(format!("x following failed: {e}")))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ServiceError::Upstream(format!("x following error: {body}")));
        }

        let parsed = resp
            .json::<Resp>()
            .await
            .map_err(|e| ServiceError::Upstream(format!("x following parse: {e}")))?;

        Ok(parsed
            .data
            .unwrap_or_default()
            .into_iter()
            .map(|u| u.username.to_lowercase())
            .collect())
    }
}

pub fn parse_tweet_id(tweet_ref: &str) -> Result<String, ServiceError> {
    let trimmed = tweet_ref.trim();
    if trimmed.chars().all(|c| c.is_ascii_digit()) {
        return Ok(trimmed.to_string());
    }
    if let Some(id) = trimmed.rsplit('/').next() {
        if id.chars().all(|c| c.is_ascii_digit()) {
            return Ok(id.to_string());
        }
    }
    Err(ServiceError::bad_request("invalid tweet url or id"))
}

pub fn tweet_contains_profile_link(text: &str, profile_url: &str, username: &str) -> bool {
    let lower = text.to_lowercase();
    let profile_url_lower = profile_url.to_lowercase();
    let username_lower = username.to_lowercase();
    lower.contains(&profile_url_lower)
        || lower.contains(&format!("@{username_lower}"))
        || lower.contains(&format!("mysocial.network/@{username_lower}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tweet_id_from_url() {
        assert_eq!(
            parse_tweet_id("https://x.com/user/status/12345").unwrap(),
            "12345"
        );
    }
}
