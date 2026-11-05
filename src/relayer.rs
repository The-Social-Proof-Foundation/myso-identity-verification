// Copyright (c) The Social Proof Foundation, LLC.
// SPDX-License-Identifier: Apache-2.0

use std::str::FromStr;
use std::sync::Arc;

use anyhow::{Context, Result};
use myso_sdk::rpc_types::{
    MySoExecutionStatus, MySoTransactionBlockEffectsAPI, MySoTransactionBlockResponse,
    MySoTransactionBlockResponseOptions,
};
use myso_sdk::types::base_types::{ObjectID, MySoAddress};
use myso_sdk::types::crypto::{MySoKeyPair, Signature};
use myso_sdk::types::transaction_driver_types::ExecuteTransactionRequestType;
use myso_sdk::types::transaction::{Transaction, TransactionData};
use myso_sdk::MySoClient;
use shared_crypto::intent::{Intent, IntentMessage};

use crate::chain::{BadgeFields, ChainPtb, SharedObjectRef};
use crate::config::{BadgeAssets, Config};

/// Ecosystem badge names (the on-chain badge id is `ecosystem_badge_<name>`).
pub const VERIFIED_X_BADGE: &str = "verified_x_account";
const BADGE_TYPE_VERIFIED_X: u8 = 1;
const BADGE_TYPE_SHARE_CAMPAIGN: u8 = 2;

/// `platform` module abort codes for soulbound badge minting.
const ABORT_ALREADY_ISSUED: u64 = 23;
const ABORT_X_HANDLE_TAKEN: u64 = 24;
const ABORT_WALLET_HAS_X_HANDLE: u64 = 25;

#[derive(Debug, thiserror::Error)]
pub enum BadgeMintError {
    #[error("wallet already holds this badge")]
    AlreadyIssued,
    #[error("this X account is already verified by another wallet")]
    XHandleTaken,
    #[error("this wallet already verified a different X account")]
    WalletHasXHandle,
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub struct Relayer {
    client: MySoClient,
    keypair: MySoKeyPair,
    config: Arc<Config>,
    package_id: ObjectID,
    admin_cap_id: ObjectID,
    badge_registry: SharedObjectRef,
}

impl Relayer {
    pub async fn new(config: Arc<Config>) -> Result<Self> {
        let client = myso_sdk::MySoClientBuilder::default()
            .build(&config.myso_rpc_url)
            .await
            .context("connect myso rpc")?;

        let keypair = parse_relayer_keypair(&config.relayer_private_key_hex)
            .context("parse RELAYER_PRIVATE_KEY")?;

        let registry_id = ObjectID::from_str(&config.ecosystem_badge_registry_id)
            .context("ECOSYSTEM_BADGE_REGISTRY_ID")?;
        let badge_registry = ChainPtb::fetch_shared_object_ref(&client, registry_id)
            .await
            .context("load EcosystemBadgeRegistry (ECOSYSTEM_BADGE_REGISTRY_ID)")?;

        Ok(Self {
            client,
            keypair,
            package_id: ObjectID::from_str(&config.myso_social_package_id)
                .context("MYSO_SOCIAL_PACKAGE_ID")?,
            admin_cap_id: ObjectID::from_str(&config.ecosystem_badge_admin_cap_id)
                .context("ECOSYSTEM_BADGE_ADMIN_CAP_ID")?,
            badge_registry,
            config,
        })
    }

    pub fn address(&self) -> MySoAddress {
        MySoAddress::from(&self.keypair.public())
    }

    pub fn client(&self) -> &MySoClient {
        &self.client
    }

    /// Mint the soulbound Verified X badge into `wallet` and bind `x_username` to it on-chain.
    /// No profile is required and no user signature is needed.
    pub async fn mint_verified_x(
        &self,
        wallet: &str,
        x_username: &str,
    ) -> Result<MySoTransactionBlockResponse, BadgeMintError> {
        let assets = &self.config.badge_assets;
        let recipient = parse_wallet(wallet)?;
        let cap_ref = self.cap_ref().await?;
        let ptb = ChainPtb::build_mint_verified_x_badge(
            self.package_id,
            cap_ref,
            self.badge_registry,
            recipient,
            x_username.to_string(),
            BadgeFields {
                name: VERIFIED_X_BADGE.to_string(),
                description: assets.verified_x_description.clone(),
                media_url: assets.verified_x_media_url.clone(),
                icon_url: assets.verified_x_icon_url.clone(),
                badge_type: BADGE_TYPE_VERIFIED_X,
            },
        )?;
        self.execute_mint(ptb).await
    }

    /// Mint a share-campaign badge (`early_adopter` / `ambassador`) into `wallet`.
    pub async fn mint_share_badge(
        &self,
        wallet: &str,
        badge_name: &str,
        assets: &BadgeAssets,
    ) -> Result<MySoTransactionBlockResponse, BadgeMintError> {
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
            other => return Err(anyhow::anyhow!("unknown badge name {other}").into()),
        };
        let recipient = parse_wallet(wallet)?;
        let cap_ref = self.cap_ref().await?;
        let ptb = ChainPtb::build_mint_ecosystem_badge(
            self.package_id,
            cap_ref,
            self.badge_registry,
            recipient,
            BadgeFields {
                name: badge_name.to_string(),
                description: description.clone(),
                media_url: media.clone(),
                icon_url: icon.clone(),
                badge_type: BADGE_TYPE_SHARE_CAMPAIGN,
            },
        )?;
        self.execute_mint(ptb).await
    }

    async fn cap_ref(&self) -> Result<myso_sdk::types::base_types::ObjectRef> {
        ChainPtb::fetch_owned_cap(&self.client, self.address(), self.admin_cap_id).await
    }

    async fn execute_mint(
        &self,
        ptb: myso_sdk::types::transaction::ProgrammableTransaction,
    ) -> Result<MySoTransactionBlockResponse, BadgeMintError> {
        let signer = self.address();
        let gas_price = self
            .client
            .read_api()
            .get_reference_gas_price()
            .await
            .context("reference gas price")?;
        let gas = self
            .client
            .transaction_builder()
            .select_gas(signer, None, 50_000_000, vec![], gas_price)
            .await
            .context("select gas")?;
        let resp = self.sign_and_execute(signer, gas, ptb).await?;
        if let Some(effects) = resp.effects.as_ref() {
            if let MySoExecutionStatus::Failure { error } = effects.status() {
                return Err(classify_mint_failure(error));
            }
        }
        Ok(resp)
    }

    pub async fn chain_identifier(&self) -> Result<String> {
        self.client
            .read_api()
            .get_chain_identifier()
            .await
            .context("get chain identifier")
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

fn parse_wallet(wallet: &str) -> Result<MySoAddress> {
    let normalized = crate::chain::normalize_object_id(wallet).context("invalid wallet address")?;
    MySoAddress::from_str(&normalized).context("invalid wallet address")
}

/// Map a failed mint's execution error to a badge error. Move aborts render as
/// `MoveAbort(MoveLocation { module: ... name: Identifier("platform") ... }, <code>)`.
fn classify_mint_failure(error: &str) -> BadgeMintError {
    if error.contains("MoveAbort") && error.contains("\"platform\"") {
        // The abort code follows the closing `}` of the MoveLocation: `}, <code>)`.
        let code = error.rsplit_once("}, ").and_then(|(_, tail)| {
            let digits: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
            digits.parse::<u64>().ok()
        });
        match code {
            Some(ABORT_ALREADY_ISSUED) => return BadgeMintError::AlreadyIssued,
            Some(ABORT_X_HANDLE_TAKEN) => return BadgeMintError::XHandleTaken,
            Some(ABORT_WALLET_HAS_X_HANDLE) => return BadgeMintError::WalletHasXHandle,
            _ => {}
        }
    }
    BadgeMintError::Other(anyhow::anyhow!("badge mint failed on-chain: {error}"))
}

/// Accepts the same formats as `myso keytool` / keystore exports:
/// - Bech32 `mysoprivkey1...`
/// - Base64 `flag || privkey`
/// - Hex `flag || privkey`
fn parse_relayer_keypair(raw: &str) -> Result<MySoKeyPair> {
    let contents = raw.trim();
    if contents.is_empty() {
        anyhow::bail!("RELAYER_PRIVATE_KEY is empty");
    }

    if let Ok(key) = MySoKeyPair::decode(contents) {
        return Ok(key);
    }

    if let Ok(bytes) = hex::decode(contents) {
        return MySoKeyPair::from_bytes(&bytes)
            .map_err(|e| anyhow::anyhow!("invalid hex RELAYER_PRIVATE_KEY: {e}"));
    }

    anyhow::bail!(
        "RELAYER_PRIVATE_KEY must be a mysoprivkey1... bech32 string or hex-encoded flag||privkey"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mysoprivkey_format() {
        use myso_sdk::types::base_types::MySoAddress;
        use std::str::FromStr;

        let key = parse_relayer_keypair(
            "mysoprivkey1qpekep5ltp9klhcajc25reg4xpxdh6hcu0th3xh8nlg9utva5dzhss4859v",
        )
        .expect("bech32 key should parse");

        let address = MySoAddress::from(&key.public());
        assert_eq!(
            address,
            MySoAddress::from_str(
                "0x2458950181e415250823d6ce1d55f2b3427826a111939e0d6d38e9a1397411d8"
            )
            .unwrap()
        );
    }

    #[test]
    fn classify_platform_abort_codes() {
        let loc = |code: u64| {
            format!(
                "MoveAbort(MoveLocation {{ module: ModuleId {{ address: 50c1, name: Identifier(\"platform\") }}, function: 12, instruction: 40, function_name: Some(\"mint_ecosystem_badge_internal\") }}, {code}) in command 0"
            )
        };
        assert!(matches!(classify_mint_failure(&loc(23)), BadgeMintError::AlreadyIssued));
        assert!(matches!(classify_mint_failure(&loc(24)), BadgeMintError::XHandleTaken));
        assert!(matches!(classify_mint_failure(&loc(25)), BadgeMintError::WalletHasXHandle));
        assert!(matches!(classify_mint_failure(&loc(9)), BadgeMintError::Other(_)));
        assert!(matches!(classify_mint_failure("InsufficientGas"), BadgeMintError::Other(_)));
    }
}
