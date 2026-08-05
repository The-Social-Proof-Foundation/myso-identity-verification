# PoC Claim Evidence Hash v1

Shared contract between `proof-of-creativity` and `myso-identity-verification`.

See also: `proof-of-creativity/docs/poc-claim-evidence-v1.md`.

## Payload

Canonical JSON (sorted keys, no whitespace):

```json
{
  "attested_x_handle": "creatorname",
  "beneficiary_id": "0x...",
  "identity_hash": "0x63726561746f726e616d65",
  "identity_source": 1,
  "v": 1,
  "verifier": "myso-identity-verification",
  "verified_at": 1720000000,
  "wallet": "0x..."
}
```

## Hash

`evidence_hash = blake2b-256(canonical_json_utf8)` → 32-byte digest, hex-encoded with `0x` prefix.

## Identity hash

```
identity_hash = "0x" + hex(utf8(lowercase(trim(handle))))
```

Golden example: `CreatorName` → `0x63726561746f726e616d65`.
