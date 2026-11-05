// Copyright (c) The Social Proof Foundation, LLC.
// SPDX-License-Identifier: Apache-2.0

use std::str::FromStr;

use anyhow::{Context, Result};
use move_core_types::identifier::Identifier;
use myso_sdk::types::object::Owner;
use myso_sdk::types::base_types::{MySoAddress, ObjectID, ObjectRef};
use myso_sdk::types::programmable_transaction_builder::ProgrammableTransactionBuilder as Ptb;
use myso_sdk::types::programmable_transaction_builder::ProgrammableTransactionBuilder;
use myso_sdk::types::transaction::{Command, ObjectArg, ProgrammableTransaction};
use myso_sdk::types::transaction::SharedObjectMutability;
use myso_sdk::MySoClient;

pub struct ChainPtb;

/// Shared object id + initial shared version.
#[derive(Debug, Clone, Copy)]
pub struct SharedObjectRef {
    pub id: ObjectID,
    pub initial_shared_version: u64,
}

impl SharedObjectRef {
    fn mutable_arg(&self) -> ObjectArg {
        ObjectArg::SharedObject {
            id: self.id,
            initial_shared_version: self.initial_shared_version.into(),
            mutability: SharedObjectMutability::Mutable,
        }
    }
}

/// `0x6` clock, read-only.
fn clock_arg() -> ObjectArg {
    ObjectArg::SharedObject {
        id: ObjectID::from_single_byte(6),
        initial_shared_version: 1u64.into(),
        mutability: SharedObjectMutability::Immutable,
    }
}

/// Display fields of an ecosystem badge, in `platform::mint_*` argument order.
#[derive(Debug, Clone)]
pub struct BadgeFields {
    pub name: String,
    pub description: String,
    pub media_url: String,
    pub icon_url: String,
    pub badge_type: u8,
}

impl BadgeFields {
    fn push_args(
        self,
        ptb: &mut Ptb,
    ) -> Result<Vec<myso_sdk::types::transaction::Argument>> {
        Ok(vec![
            ptb.pure(self.name)?,
            ptb.pure(self.description)?,
            ptb.pure(self.media_url)?,
            ptb.pure(self.icon_url)?,
            ptb.pure(self.badge_type)?,
        ])
    }
}

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

    /// `platform::mint_verified_x_badge` — soulbound Verified X badge minted into `recipient`.
    /// No profile object is involved, so the relayer never needs a user-owned object.
    #[allow(clippy::too_many_arguments)]
    pub fn build_mint_verified_x_badge(
        package_id: ObjectID,
        cap_ref: ObjectRef,
        registry: SharedObjectRef,
        recipient: MySoAddress,
        x_handle: String,
        badge: BadgeFields,
    ) -> Result<ProgrammableTransaction> {
        let mut ptb = ProgrammableTransactionBuilder::new();
        let cap = ptb.obj(ObjectArg::ImmOrOwnedObject(cap_ref))?;
        let registry = ptb.obj(registry.mutable_arg())?;
        let recipient = ptb.pure(recipient)?;
        let handle = ptb.pure(x_handle)?;
        let mut args = vec![cap, registry, recipient, handle];
        args.extend(badge.push_args(&mut ptb)?);
        args.push(ptb.obj(clock_arg())?);
        ptb.command(Command::move_call(
            package_id,
            Identifier::new("platform")?,
            Identifier::new("mint_verified_x_badge")?,
            vec![],
            args,
        ));
        Ok(ptb.finish())
    }

    /// `platform::mint_ecosystem_badge` — soulbound badge (Early Adopter, Ambassador, ...).
    pub fn build_mint_ecosystem_badge(
        package_id: ObjectID,
        cap_ref: ObjectRef,
        registry: SharedObjectRef,
        recipient: MySoAddress,
        badge: BadgeFields,
    ) -> Result<ProgrammableTransaction> {
        let mut ptb = ProgrammableTransactionBuilder::new();
        let cap = ptb.obj(ObjectArg::ImmOrOwnedObject(cap_ref))?;
        let registry = ptb.obj(registry.mutable_arg())?;
        let recipient = ptb.pure(recipient)?;
        let mut args = vec![cap, registry, recipient];
        args.extend(badge.push_args(&mut ptb)?);
        args.push(ptb.obj(clock_arg())?);
        ptb.command(Command::move_call(
            package_id,
            Identifier::new("platform")?,
            Identifier::new("mint_ecosystem_badge")?,
            vec![],
            args,
        ));
        Ok(ptb.finish())
    }

    /// Initial shared version of a shared object (e.g. the `EcosystemBadgeRegistry`).
    pub async fn fetch_shared_object_ref(
        client: &MySoClient,
        id: ObjectID,
    ) -> Result<SharedObjectRef> {
        let resp = client
            .read_api()
            .get_object_with_options(
                id,
                myso_sdk::rpc_types::MySoObjectDataOptions {
                    show_owner: true,
                    ..Default::default()
                },
            )
            .await
            .with_context(|| format!("fetch shared object {id}"))?;
        let data = resp.data.with_context(|| format!("shared object {id} not found"))?;
        match data.owner {
            Some(Owner::Shared {
                initial_shared_version,
            }) => Ok(SharedObjectRef {
                id,
                initial_shared_version: initial_shared_version.into(),
            }),
            other => anyhow::bail!("object {id} is not shared (owner={other:?})"),
        }
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
