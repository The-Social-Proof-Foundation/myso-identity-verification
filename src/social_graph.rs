// Copyright (c) The Social Proof Foundation, LLC.
// SPDX-License-Identifier: Apache-2.0

use crate::error::ServiceError;
use crate::indexer::IndexerProfile;
use crate::state::AppState;
use crate::x_tokens::XTokenStore;

#[derive(Debug, serde::Serialize)]
pub struct SocialGraphMatch {
    pub owner_address: String,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub x_username: Option<String>,
}

pub async fn find_x_matches(
    state: &AppState,
    wallet_address: &str,
) -> Result<Vec<SocialGraphMatch>, ServiceError> {
    let _profile = state
        .indexer
        .get_profile_by_address(wallet_address)
        .await?
        .ok_or_else(|| ServiceError::not_found("profile not found"))?;

    let access_token = XTokenStore::get_valid_access_token(state, wallet_address).await?;
    let x_user_id = XTokenStore::get_x_user_id(state, wallet_address).await?;

    let following = state
        .x_api
        .get_following_usernames(&x_user_id, &access_token, 500)
        .await?;

    if following.is_empty() {
        return Ok(vec![]);
    }

    let matched = state.indexer.profiles_by_x_usernames(&following).await?;
    Ok(matched
        .into_iter()
        .filter_map(map_match)
        .collect())
}

fn map_match(p: IndexerProfile) -> Option<SocialGraphMatch> {
    Some(SocialGraphMatch {
        owner_address: p.owner_address?,
        username: p.username,
        display_name: p.display_name,
        x_username: p.x_username,
    })
}
