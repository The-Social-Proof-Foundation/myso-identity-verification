// Copyright (c) The Social Proof Foundation, LLC.
// SPDX-License-Identifier: Apache-2.0

use std::str::FromStr;

use anyhow::{Context, Result};
use move_core_types::identifier::Identifier;
use myso_sdk::types::object::Owner;
use myso_sdk::types::base_types::{ObjectID, ObjectRef};
use myso_sdk::types::programmable_transaction_builder::ProgrammableTransactionBuilder;
use myso_sdk::types::transaction::{Command, ObjectArg, ProgrammableTransaction};
use myso_sdk::types::transaction::SharedObjectMutability;
use myso_sdk::MySoClient;

pub struct ChainPtb;

impl ChainPtb {
    pub async fn fetch_owned_cap(
        client: &MySoClient,
        owner: myso_sdk::types::base_types::MySoAddress,
        cap_id: ObjectID,
    ) -> Result<ObjectRef> {
        let resp = client
            .read_api()
            .get_object_with_options(
                cap_id,
                myso_sdk::rpc_types::MySoObjectDataOptions::default(),
            )
            .await
            .context("fetch admin cap object")?;

        let data = resp.data.context("admin cap missing")?;
        let owner_matches = matches!(
            data.owner,
            Some(Owner::AddressOwner(addr)) if addr == owner
        );
        anyhow::ensure!(owner_matches, "relayer does not own ecosystem badge admin cap");
        Ok(data.object_ref())
    }

    pub fn build_set_x_username_and_badge(
        package_id: ObjectID,
        cap_ref: ObjectRef,
        profile_id: ObjectID,
        profile_shared_version: u64,
        x_username: String,
        badge_name: String,
        badge_description: String,
        badge_media_url: String,
        badge_icon_url: String,
        badge_type: u8,
    ) -> Result<ProgrammableTransaction> {
        let mut ptb = ProgrammableTransactionBuilder::new();
        let cap = ptb.obj(ObjectArg::ImmOrOwnedObject(cap_ref))?;
        let profile = ptb.obj(ObjectArg::SharedObject {
            id: profile_id,
            initial_shared_version: profile_shared_version.into(),
            mutability: SharedObjectMutability::Mutable,
        })?;

        let username_arg = ptb.pure(Some(x_username))?;
        ptb.command(Command::move_call(
            package_id,
            Identifier::new("profile")?,
            Identifier::new("admin_set_profile_x_username")?,
            vec![],
            vec![cap, profile, username_arg],
        ));

        let badge_name = ptb.pure(badge_name)?;
        let badge_description = ptb.pure(badge_description)?;
        let badge_media_url = ptb.pure(badge_media_url)?;
        let badge_icon_url = ptb.pure(badge_icon_url)?;
        let badge_type = ptb.pure(badge_type)?;

        ptb.command(Command::move_call(
            package_id,
            Identifier::new("profile")?,
            Identifier::new("assign_ecosystem_badge")?,
            vec![],
            vec![
                cap,
                profile,
                badge_name,
                badge_description,
                badge_media_url,
                badge_icon_url,
                badge_type,
            ],
        ));

        Ok(ptb.finish())
    }

    pub fn build_assign_badge(
        package_id: ObjectID,
        cap_ref: ObjectRef,
        profile_id: ObjectID,
        profile_shared_version: u64,
        badge_name: String,
        badge_description: String,
        badge_media_url: String,
        badge_icon_url: String,
        badge_type: u8,
    ) -> Result<ProgrammableTransaction> {
        let mut ptb = ProgrammableTransactionBuilder::new();
        let cap = ptb.obj(ObjectArg::ImmOrOwnedObject(cap_ref))?;
        let profile = ptb.obj(ObjectArg::SharedObject {
            id: profile_id,
            initial_shared_version: profile_shared_version.into(),
            mutability: SharedObjectMutability::Mutable,
        })?;

        let badge_name = ptb.pure(badge_name)?;
        let badge_description = ptb.pure(badge_description)?;
        let badge_media_url = ptb.pure(badge_media_url)?;
        let badge_icon_url = ptb.pure(badge_icon_url)?;
        let badge_type = ptb.pure(badge_type)?;

        ptb.command(Command::move_call(
            package_id,
            Identifier::new("profile")?,
            Identifier::new("assign_ecosystem_badge")?,
            vec![],
            vec![
                cap,
                profile,
                badge_name,
                badge_description,
                badge_media_url,
                badge_icon_url,
                badge_type,
            ],
        ));

        Ok(ptb.finish())
    }

    pub fn parse_object_id(id: &str) -> Result<ObjectID> {
        let normalized = normalize_object_id(id).context("invalid object id")?;
        ObjectID::from_str(&normalized).context("invalid object id")
    }
}

/// Lowercase `0x` + 64 hex chars (32-byte object id), matching iOS `normalizedObjectId`.
pub fn normalize_object_id(raw: &str) -> Result<String> {
    let trimmed = raw.trim().to_ascii_lowercase();
    let hex = trimmed
        .strip_prefix("0x")
        .ok_or_else(|| anyhow::anyhow!("object id must start with 0x"))?;
    if hex.is_empty() || hex.len() > 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        anyhow::bail!("object id must be 1..=64 hex characters after 0x");
    }
    if hex.len() == 64 {
        return Ok(format!("0x{hex}"));
    }
    Ok(format!("0x{}{hex}", "0".repeat(64 - hex.len())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_pads_short_ids() {
        assert_eq!(
            normalize_object_id("0x50c1").unwrap(),
            "0x00000000000000000000000000000000000000000000000000000000000050c1"
        );
    }

    #[test]
    fn normalize_lowercases_and_accepts_full_ids() {
        let full = "0xABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789";
        assert_eq!(
            normalize_object_id(full).unwrap(),
            "0xabcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
        );
    }

    #[test]
    fn normalize_rejects_non_hex() {
        assert!(normalize_object_id("0xzz").is_err());
        assert!(normalize_object_id("50c1").is_err());
    }
}
