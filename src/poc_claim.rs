// Copyright (c) The Social Proof Foundation, LLC.
// SPDX-License-Identifier: Apache-2.0

use std::collections::BTreeMap;

use blake2::{Blake2b512, Digest};
use chrono::Utc;
use serde_json::{json, Value};

use crate::error::ServiceError;

const EVIDENCE_VERSION: i64 = 1;
const IDENTITY_SOURCE_X: i64 = 1;

pub fn canonical_registry_username(username: &str) -> String {
    username.trim().trim_start_matches('@').to_lowercase()
}

pub fn parse_identity_hash(value: &str) -> Result<Vec<u8>, ServiceError> {
    let text = value.trim();
    if let Some(hex_part) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        if hex_part.is_empty() {
            return Ok(Vec::new());
        }
        let padded = if hex_part.len() % 2 == 1 {
            format!("0{hex_part}")
        } else {
            hex_part.to_string()
        };
        return hex::decode(padded)
            .map_err(|e| ServiceError::bad_request(format!("invalid identity_hash hex: {e}")));
    }
    Ok(text.as_bytes().to_vec())
}

pub fn identity_hash_hex_from_handle(handle: &str) -> String {
    let canonical = canonical_registry_username(handle);
    format!("0x{}", hex::encode(canonical.as_bytes()))
}

pub fn validate_handle_matches_identity_hash(
    handle: &str,
    identity_hash: &str,
) -> Result<String, ServiceError> {
    let canonical = canonical_registry_username(handle);
    let expected = parse_identity_hash(identity_hash)?;
    let actual = canonical.as_bytes().to_vec();
    if expected != actual {
        return Err(ServiceError::bad_request(format!(
            "X handle @{canonical} does not match identity_hash"
        )));
    }
    Ok(canonical)
}

pub fn compute_evidence_hash_v1(payload: &BTreeMap<String, Value>) -> Result<String, ServiceError> {
    let canonical = serde_json::to_string(payload)
        .map_err(|e| ServiceError::Internal(e.into()))?;
    let mut hasher = Blake2b512::new();
    hasher.update(canonical.as_bytes());
    let digest = hasher.finalize();
    let short = &digest[..32];
    Ok(format!("0x{}", hex::encode(short)))
}

pub fn build_attestation_payload(
    beneficiary_id: &str,
    identity_hash: &str,
    attested_x_handle: &str,
    wallet: &str,
    verifier: &str,
    verified_at: i64,
) -> BTreeMap<String, Value> {
    let mut payload = BTreeMap::new();
    payload.insert("attested_x_handle".into(), json!(attested_x_handle));
    payload.insert("beneficiary_id".into(), json!(beneficiary_id));
    payload.insert("identity_hash".into(), json!(identity_hash));
    payload.insert("identity_source".into(), json!(IDENTITY_SOURCE_X));
    payload.insert("v".into(), json!(EVIDENCE_VERSION));
    payload.insert("verifier".into(), json!(verifier));
    payload.insert("verified_at".into(), json!(verified_at));
    payload.insert("wallet".into(), json!(wallet));
    payload
}

pub fn build_attestation(
    beneficiary_id: &str,
    identity_hash: &str,
    attested_x_handle: &str,
    wallet: &str,
) -> Result<(String, i64), ServiceError> {
    let verified_at = Utc::now().timestamp();
    let payload = build_attestation_payload(
        beneficiary_id,
        identity_hash,
        attested_x_handle,
        wallet,
        "myso-identity-verification",
        verified_at,
    );
    let evidence_hash = compute_evidence_hash_v1(&payload)?;
    Ok((evidence_hash, verified_at))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evidence_hash_stable() {
        let mut payload = BTreeMap::new();
        payload.insert("v".into(), json!(1));
        payload.insert("beneficiary_id".into(), json!("0x1"));
        payload.insert("wallet".into(), json!("0x2"));
        let h1 = compute_evidence_hash_v1(&payload).unwrap();
        let h2 = compute_evidence_hash_v1(&payload).unwrap();
        assert_eq!(h1, h2);
    }

    #[test]
    fn handle_identity_hash_roundtrip() {
        let hash = identity_hash_hex_from_handle("CreatorName");
        assert_eq!(hash, "0x63726561746f726e616d65");
        assert!(validate_handle_matches_identity_hash("CreatorName", &hash).is_ok());
    }
}
