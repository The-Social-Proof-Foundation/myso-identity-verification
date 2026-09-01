// Copyright (c) The Social Proof Foundation, LLC.
// SPDX-License-Identifier: Apache-2.0

use base64::Engine;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::error::ServiceError;

#[derive(Debug, Deserialize)]
pub struct FacebookSignedRequest {
    pub user_id: Option<String>,
}

pub fn parse_signed_request(
    signed_request: &str,
    app_secret: &str,
) -> Result<FacebookSignedRequest, ServiceError> {
    let (encoded_sig, encoded_payload) = signed_request
        .split_once('.')
        .ok_or_else(|| ServiceError::bad_request("invalid facebook signed_request"))?;

    let sig = b64url_decode(encoded_sig)
        .map_err(|_| ServiceError::bad_request("invalid facebook signed_request signature"))?;
    let expected = hmac_sha256(app_secret.as_bytes(), encoded_payload.as_bytes());
    if sig != expected {
        return Err(ServiceError::unauthorized(
            "facebook signed_request signature mismatch",
        ));
    }

    let payload = b64url_decode(encoded_payload)
        .map_err(|_| ServiceError::bad_request("invalid facebook signed_request payload"))?;
    serde_json::from_slice(&payload)
        .map_err(|e| ServiceError::bad_request(format!("facebook signed_request json: {e}")))
}

fn b64url_decode(raw: &str) -> Result<Vec<u8>, ()> {
    let mut padded = raw.replace('-', "+").replace('_', "/");
    while padded.len() % 4 != 0 {
        padded.push('=');
    }
    base64::engine::general_purpose::STANDARD
        .decode(padded)
        .map_err(|_| ())
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> Vec<u8> {
    const BLOCK: usize = 64;
    let mut key_block = [0u8; BLOCK];
    if key.len() > BLOCK {
        let hashed = Sha256::digest(key);
        key_block[..hashed.len()].copy_from_slice(&hashed);
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }

    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] ^= key_block[i];
        opad[i] ^= key_block[i];
    }

    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(message);
    let inner_hash = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner_hash);
    outer.finalize().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_sha256_rfc_vector() {
        let digest = hmac_sha256(b"key", b"The quick brown fox jumps over the lazy dog");
        assert_eq!(
            hex::encode(digest),
            "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
        );
    }

    #[test]
    fn parse_signed_request_roundtrip() {
        let secret = "app-secret";
        let payload = r#"{"user_id":"12345","algorithm":"HMAC-SHA256"}"#;
        let encoded_payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload);
        let sig = hmac_sha256(secret.as_bytes(), encoded_payload.as_bytes());
        let encoded_sig = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sig);
        let signed = format!("{encoded_sig}.{encoded_payload}");
        let parsed = parse_signed_request(&signed, secret).unwrap();
        assert_eq!(parsed.user_id.as_deref(), Some("12345"));
    }

    #[test]
    fn parse_signed_request_rejects_bad_sig() {
        let secret = "app-secret";
        let payload = r#"{"user_id":"12345"}"#;
        let encoded_payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload);
        let signed = format!("AAAA.{encoded_payload}");
        let err = parse_signed_request(&signed, secret).unwrap_err();
        assert!(matches!(err, ServiceError::Unauthorized(_)));
    }
}
