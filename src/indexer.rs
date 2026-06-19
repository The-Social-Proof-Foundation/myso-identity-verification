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
                    ownerAddress
                    profileId
                    username
                    displayName
                    xUsername
                }
            }
        "#;

        #[derive(Deserialize)]
        struct ProfileNode {
            #[serde(rename = "ownerAddress")]
            owner_address: Option<String>,
            #[serde(rename = "profileId")]
            profile_id: Option<String>,
            username: Option<String>,
            #[serde(rename = "displayName")]
            display_name: Option<String>,
            #[serde(rename = "xUsername")]
            x_username: Option<String>,
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

        Ok(resp.data.and_then(|d| d.profile).map(|p| IndexerProfile {
            owner_address: p.owner_address,
            profile_id: p.profile_id,
            username: p.username,
            display_name: p.display_name,
            x_username: p.x_username,
        }))
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

        let query = r#"
            query ProfilesByXUsernames($usernames: [String!]!) {
                profilesByXUsernames(usernames: $usernames) {
                    ownerAddress
                    profileId
                    username
                    displayName
                    xUsername
                }
            }
        "#;

        #[derive(Deserialize)]
        struct ProfileNode {
            #[serde(rename = "ownerAddress")]
            owner_address: Option<String>,
            #[serde(rename = "profileId")]
            profile_id: Option<String>,
            username: Option<String>,
            #[serde(rename = "displayName")]
            display_name: Option<String>,
            #[serde(rename = "xUsername")]
            x_username: Option<String>,
        }
        #[derive(Deserialize)]
        struct Data {
            #[serde(rename = "profilesByXUsernames")]
            profiles_by_x_usernames: Option<Vec<ProfileNode>>,
        }
        #[derive(Deserialize)]
        struct Resp {
            data: Option<Data>,
        }

        let normalized: Vec<String> = usernames.iter().map(|u| u.to_lowercase()).collect();
        let resp: Resp = self
            .graphql_query(
                query,
                serde_json::json!({ "usernames": normalized }),
            )
            .await?;

        Ok(resp
            .data
            .and_then(|d| d.profiles_by_x_usernames)
            .unwrap_or_default()
            .into_iter()
            .map(|p| IndexerProfile {
                owner_address: p.owner_address,
                profile_id: p.profile_id,
                username: p.username,
                display_name: p.display_name,
                x_username: p.x_username,
            })
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
