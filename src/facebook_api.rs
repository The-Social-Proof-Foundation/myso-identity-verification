// Copyright (c) The Social Proof Foundation, LLC.
// SPDX-License-Identifier: Apache-2.0

use serde::Deserialize;
use serde_json::Value;

use crate::error::ServiceError;

const GRAPH_ME: &str = "https://graph.facebook.com/v21.0/me";
const GRAPH_FRIENDS: &str = "https://graph.facebook.com/v21.0/me/friends";
const MAX_FRIEND_PAGES: usize = 50;
const MAX_FRIENDS: usize = 5000;

#[derive(Debug, Clone)]
pub struct FacebookMe {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Deserialize)]
struct GraphMe {
    id: String,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GraphFriendsPage {
    #[serde(default)]
    data: Vec<GraphFriend>,
    paging: Option<GraphPaging>,
}

#[derive(Debug, Deserialize)]
struct GraphFriend {
    id: String,
}

#[derive(Debug, Deserialize)]
struct GraphPaging {
    next: Option<String>,
}

pub struct FacebookApiClient {
    http: reqwest::Client,
}

impl FacebookApiClient {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }

    pub async fn get_me(&self, access_token: &str) -> Result<FacebookMe, ServiceError> {
        let resp = self
            .http
            .get(GRAPH_ME)
            .query(&[("fields", "id,name"), ("access_token", access_token)])
            .send()
            .await
            .map_err(|e| ServiceError::Upstream(format!("facebook /me failed: {e}")))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ServiceError::Upstream(format!("facebook /me error: {body}")));
        }

        let me = resp
            .json::<GraphMe>()
            .await
            .map_err(|e| ServiceError::Upstream(format!("facebook /me parse failed: {e}")))?;

        if me.id.trim().is_empty() {
            return Err(ServiceError::Upstream("facebook /me missing id".into()));
        }

        Ok(FacebookMe {
            id: me.id,
            name: me.name.unwrap_or_default(),
        })
    }

    pub async fn list_friend_ids(&self, access_token: &str) -> Result<Vec<String>, ServiceError> {
        let mut url = format!("{GRAPH_FRIENDS}?fields=id&limit=100");
        let mut ids = Vec::new();

        for _ in 0..MAX_FRIEND_PAGES {
            let resp = self
                .http
                .get(&url)
                .query(&[("access_token", access_token)])
                .send()
                .await
                .map_err(|e| ServiceError::Upstream(format!("facebook /me/friends failed: {e}")))?;

            if !resp.status().is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(ServiceError::Upstream(format!(
                    "facebook /me/friends error: {body}"
                )));
            }

            let page: Value = resp
                .json()
                .await
                .map_err(|e| ServiceError::Upstream(format!("facebook friends parse failed: {e}")))?;
            let (page_ids, next) = parse_friends_page(&page)?;
            for id in page_ids {
                if ids.len() >= MAX_FRIENDS {
                    return Ok(ids);
                }
                if !ids.iter().any(|existing| existing == &id) {
                    ids.push(id);
                }
            }
            match next {
                Some(next_url) if !next_url.is_empty() => url = next_url,
                _ => break,
            }
        }

        Ok(ids)
    }
}

pub fn parse_friends_page(value: &Value) -> Result<(Vec<String>, Option<String>), ServiceError> {
    let page: GraphFriendsPage = serde_json::from_value(value.clone())
        .map_err(|e| ServiceError::Upstream(format!("facebook friends page parse: {e}")))?;
    let ids = page
        .data
        .into_iter()
        .map(|f| f.id)
        .filter(|id| !id.trim().is_empty())
        .collect();
    Ok((ids, page.paging.and_then(|p| p.next)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_friends_page_extracts_ids_and_next() {
        let page = json!({
            "data": [{ "id": "111" }, { "id": "222" }, { "id": "" }],
            "paging": { "next": "https://graph.facebook.com/v21.0/me/friends?after=abc" }
        });
        let (ids, next) = parse_friends_page(&page).unwrap();
        assert_eq!(ids, vec!["111".to_string(), "222".to_string()]);
        assert_eq!(
            next.as_deref(),
            Some("https://graph.facebook.com/v21.0/me/friends?after=abc")
        );
    }

    #[test]
    fn parse_friends_page_empty_has_no_next() {
        let page = json!({ "data": [] });
        let (ids, next) = parse_friends_page(&page).unwrap();
        assert!(ids.is_empty());
        assert!(next.is_none());
    }
}
