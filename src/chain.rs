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
        ObjectID::from_str(id).context("invalid object id")
    }
}
