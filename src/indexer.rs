// Copyright (c) The Social Proof Foundation, LLC.
// SPDX-License-Identifier: Apache-2.0

use reqwest::Client;
use serde::de::DeserializeOwned;
use serde::Deserialize;

use crate::error::ServiceError;

#[derive(Debug, Clone, Deserialize)]
pub struct IndexerProfile {
    pub owner_address: Option<String>,
    pub profile_id: Option<String>,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub x_username: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IndexerBadge {
    pub badge_id: String,
    pub badge_name: String,
}

pub struct IndexerClient {
    graphql_url: String,
    http: Client,
    relayer_address: String,
}

impl IndexerClient {
    pub fn new(graphql_url: String, http: Client, relayer_address: String) -> Self {
        Self {
            graphql_url,
            http,
            relayer_address: relayer_address.to_lowercase(),
        }
    }

    pub async fn get_profile_by_address(
        &self,
        address: &str,
    ) -> Result<Option<IndexerProfile>, ServiceError> {
        let query = r#"
            query Profile($address: MySoAddress!) {
                profile(address: $address) {
                    address
                    profileId
                    username
                    displayName
                    xUsername
                }
            }
        "#;

        #[derive(Deserialize)]
        struct Data {
            profile: Option<ProfileGraphNode>,
        }
        #[derive(Deserialize)]
        struct Resp {
            data: Option<Data>,
        }

        let resp: Resp = self
            .graphql_query(
                query,
                serde_json::json!({ "address": address }),
            )
            .await?;

        Ok(resp
            .data
            .and_then(|d| d.profile)
            .map(profile_node_to_indexer))
    }

    pub async fn get_profile_badges(
        &self,
        address: &str,
    ) -> Result<Vec<IndexerBadge>, ServiceError> {
        let query = r#"
            query ProfileBadges($address: MySoAddress!) {
                profile(address: $address) {
                    badges {
                        badgeId
                        badgeName
                    }
                }
            }
        "#;

        #[derive(Deserialize)]
        struct BadgeNode {
            #[serde(rename = "badgeId")]
            badge_id: String,
            #[serde(rename = "badgeName")]
            badge_name: String,
        }
        #[derive(Deserialize)]
        struct ProfileNode {
            badges: Option<Vec<BadgeNode>>,
        }
        #[derive(Deserialize)]
        struct Data {
            profile: Option<ProfileNode>,
        }
        #[derive(Deserialize)]
        struct Resp {
            data: Option<Data>,
        }

        let resp: Resp = self
            .graphql_query(
                query,
                serde_json::json!({ "address": address }),
            )
            .await?;

        Ok(resp
            .data
            .and_then(|d| d.profile)
            .and_then(|p| p.badges)
            .unwrap_or_default()
            .into_iter()
            .map(|b| IndexerBadge {
                badge_id: b.badge_id,
                badge_name: b.badge_name,
            })
            .collect())
    }

    pub async fn profiles_by_x_usernames(
        &self,
        usernames: &[String],
    ) -> Result<Vec<IndexerProfile>, ServiceError> {
        if usernames.is_empty() {
            return Ok(vec![]);
        }

        let normalized: Vec<String> = usernames.iter().map(|u| u.to_lowercase()).collect();

        match self
            .profiles_by_x_usernames_direct(&normalized)
            .await
        {
            Ok(profiles) => Ok(profiles),
            Err(ServiceError::Upstream(msg))
                if msg.contains("profilesByXUsernames") && msg.contains("Unknown field") =>
            {
                self.profiles_by_x_usernames_scan(&normalized).await
            }
            Err(e) => Err(e),
        }
    }

    async fn profiles_by_x_usernames_direct(
        &self,
        usernames: &[String],
    ) -> Result<Vec<IndexerProfile>, ServiceError> {
        let query = r#"
            query ProfilesByXUsernames($usernames: [String!]!) {
                profilesByXUsernames(usernames: $usernames) {
                    address
                    profileId
                    username
                    displayName
                    xUsername
                }
            }
        "#;

        #[derive(Deserialize)]
        struct Data {
            #[serde(rename = "profilesByXUsernames")]
            profiles_by_x_usernames: Option<Vec<ProfileGraphNode>>,
        }
        #[derive(Deserialize)]
        struct Resp {
            data: Option<Data>,
        }

        let resp: Resp = self
            .graphql_query(
                query,
                serde_json::json!({ "usernames": usernames }),
            )
            .await?;

        Ok(resp
            .data
            .and_then(|d| d.profiles_by_x_usernames)
            .unwrap_or_default()
            .into_iter()
            .map(profile_node_to_indexer)
            .collect())
    }

    async fn profiles_by_x_usernames_scan(
        &self,
        usernames: &[String],
    ) -> Result<Vec<IndexerProfile>, ServiceError> {
        use std::collections::HashSet;

        let wanted: HashSet<String> = usernames.iter().cloned().collect();
        let mut matched = Vec::new();
        let mut matched_usernames = HashSet::new();
        let mut offset = 0;
        const PAGE: i32 = 100;

        loop {
            let page = self.fetch_profiles_page(PAGE, offset).await?;
            let page_len = page.len();
            if page.is_empty() {
                break;
            }

            for profile in page {
                if profile
                    .x_username
                    .as_ref()
                    .map(|x| wanted.contains(&x.to_lowercase()))
                    .unwrap_or(false)
                {
                    let key = profile.x_username.as_ref().unwrap().to_lowercase();
                    if matched_usernames.insert(key) {
                        matched.push(profile);
                    }
                }
            }

            if matched_usernames.len() >= wanted.len() {
                break;
            }

            if page_len < PAGE as usize {
                break;
            }
            offset += PAGE;
        }

        Ok(matched)
    }

    async fn fetch_profiles_page(
        &self,
        limit: i32,
        offset: i32,
    ) -> Result<Vec<IndexerProfile>, ServiceError> {
        let query = r#"
            query ProfilesPage($limit: Int!, $offset: Int!) {
                profiles(limit: $limit, offset: $offset) {
                    address
                    profileId
                    username
                    displayName
                    xUsername
                }
            }
        "#;

        #[derive(Deserialize)]
        struct Data {
            profiles: Option<Vec<ProfileGraphNode>>,
        }
        #[derive(Deserialize)]
        struct Resp {
            data: Option<Data>,
        }

        let resp: Resp = self
            .graphql_query(
                query,
                serde_json::json!({ "limit": limit, "offset": offset }),
            )
            .await?;

        Ok(resp
            .data
            .and_then(|d| d.profiles)
            .unwrap_or_default()
            .into_iter()
            .map(profile_node_to_indexer)
            .collect())
    }

    pub fn ecosystem_badge_id(&self, badge_name: &str) -> String {
        format!("ecosystem_badge_{}_{}", self.relayer_address, badge_name)
    }

    pub async fn has_ecosystem_badge(
        &self,
        wallet_address: &str,
        badge_name: &str,
    ) -> Result<bool, ServiceError> {
        let badge_id = self.ecosystem_badge_id(badge_name);
        let badges = self.get_profile_badges(wallet_address).await?;
        Ok(badges.iter().any(|b| b.badge_id == badge_id))
    }

    pub async fn x_username_taken(
        &self,
        x_username: &str,
        except_wallet: Option<&str>,
    ) -> Result<bool, ServiceError> {
        let profiles = self
            .profiles_by_x_usernames(&[x_username.to_lowercase()])
            .await?;
        Ok(profiles.iter().any(|p| {
            p.x_username
                .as_ref()
                .map(|x| x.eq_ignore_ascii_case(x_username))
                .unwrap_or(false)
                && p.owner_address
                    .as_ref()
                    .map(|o| {
                        except_wallet
                            .map(|w| !o.eq_ignore_ascii_case(w))
                            .unwrap_or(true)
                    })
                    .unwrap_or(false)
        }))
    }

    async fn graphql_query<T: DeserializeOwned>(
        &self,
        query: &str,
        variables: serde_json::Value,
    ) -> Result<T, ServiceError> {
        let body = serde_json::json!({ "query": query, "variables": variables });
        let resp = self
            .http
            .post(&self.graphql_url)
            .json(&body)
            .send()
            .await
            .map_err(|e| ServiceError::Upstream(format!("graphql request failed: {e}")))?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| ServiceError::Upstream(format!("graphql body read: {e}")))?;

        if !status.is_success() {
            return Err(ServiceError::Upstream(format!(
                "graphql status {status}: {text}"
            )));
        }

        let v: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| ServiceError::Upstream(format!("graphql json parse: {e}")))?;

        if let Some(errors) = v.get("errors") {
            return Err(ServiceError::Upstream(format!("graphql errors: {errors}")));
        }

        serde_json::from_value(v).map_err(|e| ServiceError::Upstream(format!("graphql decode: {e}")))
    }
}

pub fn assert_profile_owner(
    profile: &IndexerProfile,
    wallet_address: &str,
) -> Result<(), ServiceError> {
    match profile.owner_address.as_deref() {
        Some(owner) if owner.eq_ignore_ascii_case(wallet_address) => Ok(()),
        _ => Err(ServiceError::unauthorized(
            "session wallet does not own profile",
        )),
    }
}

fn profile_node_to_indexer(p: ProfileGraphNode) -> IndexerProfile {
    IndexerProfile {
        owner_address: p.address,
        profile_id: p.profile_id,
        username: p.username,
        display_name: p.display_name,
        x_username: p.x_username,
    }
}

#[derive(Debug, Deserialize)]
struct ProfileGraphNode {
    address: Option<String>,
    #[serde(rename = "profileId")]
    profile_id: Option<String>,
    username: Option<String>,
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    #[serde(rename = "xUsername")]
    x_username: Option<String>,
}
