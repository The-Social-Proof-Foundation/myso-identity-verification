// Copyright (c) The Social Proof Foundation, LLC.
// SPDX-License-Identifier: Apache-2.0

use std::str::FromStr;
use std::sync::Arc;

use anyhow::{Context, Result};
use myso_sdk::rpc_types::{
    MySoObjectDataOptions, MySoTransactionBlockResponse, MySoTransactionBlockResponseOptions,
};
use myso_sdk::types::base_types::{ObjectID, MySoAddress};
use myso_sdk::types::crypto::{MySoKeyPair, Signature};
use myso_sdk::types::transaction_driver_types::ExecuteTransactionRequestType;
use myso_sdk::types::transaction::{Transaction, TransactionData};
use myso_sdk::MySoClient;
use shared_crypto::intent::{Intent, IntentMessage};

use crate::chain::ChainPtb;
use crate::config::{BadgeAssets, Config};

pub struct Relayer {
    client: MySoClient,
    keypair: MySoKeyPair,
    config: Arc<Config>,
    package_id: ObjectID,
    admin_cap_id: ObjectID,
}

impl Relayer {
    pub async fn new(config: Arc<Config>) -> Result<Self> {
        let client = myso_sdk::MySoClientBuilder::default()
            .build(&config.myso_rpc_url)
            .await
            .context("connect myso rpc")?;

        let key_bytes = hex::decode(config.relayer_private_key_hex.trim())
            .context("decode RELAYER_PRIVATE_KEY hex")?;
        let keypair = MySoKeyPair::from_bytes(&key_bytes).context("parse relayer keypair")?;

        Ok(Self {
            client,
            keypair,
            package_id: ObjectID::from_str(&config.myso_social_package_id)
                .context("MYSO_SOCIAL_PACKAGE_ID")?,
            admin_cap_id: ObjectID::from_str(&config.ecosystem_badge_admin_cap_id)
                .context("ECOSYSTEM_BADGE_ADMIN_CAP_ID")?,
            config,
        })
    }

    pub fn address(&self) -> MySoAddress {
        MySoAddress::from(&self.keypair.public())
    }

    pub fn client(&self) -> &MySoClient {
        &self.client
    }

    pub async fn verify_x_account(
        &self,
        profile_id: &str,
        profile_shared_version: u64,
        x_username: &str,
    ) -> Result<MySoTransactionBlockResponse> {
        let assets = &self.config.badge_assets;
        self.execute_profile_badge_tx(
            profile_id,
            profile_shared_version,
            Some(x_username.to_string()),
            "verified_x_account",
            &assets.verified_x_description,
            &assets.verified_x_media_url,
            &assets.verified_x_icon_url,
            1,
        )
        .await
    }

    pub async fn assign_share_badge(
        &self,
        profile_id: &str,
        profile_shared_version: u64,
        badge_name: &str,
        assets: &BadgeAssets,
    ) -> Result<MySoTransactionBlockResponse> {
        let (description, media, icon) = match badge_name {
            "early_adopter" => (
                &assets.early_adopter_description,
                &assets.early_adopter_media_url,
                &assets.early_adopter_icon_url,
            ),
            "ambassador" => (
                &assets.ambassador_description,
                &assets.ambassador_media_url,
                &assets.ambassador_icon_url,
            ),
            other => anyhow::bail!("unknown badge name {other}"),
        };

        self.execute_profile_badge_tx(
            profile_id,
            profile_shared_version,
            None,
            badge_name,
            description,
            media,
            icon,
            2,
        )
        .await
    }

    async fn execute_profile_badge_tx(
        &self,
        profile_id: &str,
        profile_shared_version: u64,
        x_username: Option<String>,
        badge_name: &str,
        badge_description: &str,
        badge_media_url: &str,
        badge_icon_url: &str,
        badge_type: u8,
    ) -> Result<MySoTransactionBlockResponse> {
        let signer = self.address();
        let cap_ref =
            ChainPtb::fetch_owned_cap(&self.client, signer, self.admin_cap_id).await?;
        let profile_oid = ChainPtb::parse_object_id(profile_id)?;

        let gas = self
            .client
            .coin_read_api()
            .select_gas(signer, None, None, None, None)
            .await
            .context("select gas")?;

        let ptb = if let Some(username) = x_username {
            ChainPtb::build_set_x_username_and_badge(
                self.package_id,
                cap_ref,
                profile_oid,
                profile_shared_version,
                username,
                badge_name.to_string(),
                badge_description.to_string(),
                badge_media_url.to_string(),
                badge_icon_url.to_string(),
                badge_type,
            )?
        } else {
            ChainPtb::build_assign_badge(
                self.package_id,
                cap_ref,
                profile_oid,
                profile_shared_version,
                badge_name.to_string(),
                badge_description.to_string(),
                badge_media_url.to_string(),
                badge_icon_url.to_string(),
                badge_type,
            )?
        };

        self.sign_and_execute(signer, gas.object, ptb).await
    }

    pub async fn fetch_profile_shared_version(&self, profile_id: &str) -> Result<u64> {
        let oid = ChainPtb::parse_object_id(profile_id)?;
        let resp = self
            .client
            .read_api()
            .get_object_with_options(oid, MySoObjectDataOptions::default())
            .await
            .context("fetch profile object")?;
        let data = resp.data.context("profile object missing")?;
        match data.owner {
            myso_sdk::rpc_types::Owner::Shared { initial_shared_version } => {
                Ok(initial_shared_version)
            }
            other => anyhow::bail!("profile is not a shared object: {other:?}"),
        }
    }

    async fn sign_and_execute(
        &self,
        signer: MySoAddress,
        gas: myso_sdk::types::base_types::ObjectRef,
        ptb: myso_sdk::types::transaction::ProgrammableTransaction,
    ) -> Result<MySoTransactionBlockResponse> {
        let reference = self
            .client
            .read_api()
            .get_reference_gas_price()
            .await
            .context("reference gas price")?;

        let msg = IntentMessage {
            intent: Intent::myso_transaction(),
            value: TransactionData::new_programmable(
                signer,
                vec![gas],
                ptb,
                50_000_000,
                reference,
            ),
        };
        let sig = Signature::new_secure(&msg, &self.keypair);

        self.client
            .quorum_driver_api()
            .execute_transaction_block(
                Transaction::from_data(msg.value, vec![sig]),
                MySoTransactionBlockResponseOptions::new().with_effects(),
                Some(ExecuteTransactionRequestType::WaitForLocalExecution),
            )
            .await
            .context("execute transaction")
    }
}
