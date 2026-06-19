# MySo Identity Verification

Ecosystem-wide identity verification service for MySocial, DripDrop, Chatr, and future apps.

## Responsibilities

- X account verification and `verified_x_account` ecosystem badge
- Unified share-badge campaigns (`early_adopter`, `ambassador`) with 24h tweet persistence
- Social graph import (X following → MySocial profile matches)
- On-chain writes via `EcosystemBadgeAdminCap` relayer wallet

## Out of scope (see myso-salt-service)

- User login / OAuth salt / wallet derivation
- Session issuance for authentication

Apps authenticate via `@socialproof/mysocial-auth` + `myso-salt-service`, then call this service with the session JWT.

## Architecture

```
Auth (salt-service) → Profile on-chain
Identity Verification → relayer txs → profile.move events
myso-indexer-alt-social → GraphQL / social-server (read path)
Railway Cron → process pending share campaigns (Redis queue)
```

## Railway deployment

Two services, one Docker image:

| Service | Start command | Notes |
|---------|---------------|-------|
| `identity-verification-api` | `./myso-identity-verification` | Always-on, `/health` |
| `identity-verification-cron` | `./myso-identity-verification-cron` | Cron schedule: `*/15 * * * *` (UTC, dashboard only) |

Add-ons: **Redis** (pending campaign queue only — not source of truth).

## Environment variables

See [`.env.example`](.env.example).

## API

| Method | Path | Description |
|--------|------|-------------|
| GET | `/health` | Liveness |
| POST | `/oauth/x/connect` | Start X OAuth (requires session JWT) |
| GET | `/oauth/x/callback` | Complete X OAuth, issue verified badge + x_username |
| GET | `/verification/x?address=0x...` | Read verification status from indexer |
| GET | `/social-graph/x/matches` | Match X following to MySocial profiles |
| POST | `/campaigns/share/start` | Enqueue share campaign (early_adopter / ambassador) |
| GET | `/campaigns/share/status` | Pending or completed campaign status |
| POST | `/internal/cron/process-pending-campaigns` | Cron endpoint (`Authorization: Bearer $CRON_SECRET`) |

## Local development

```bash
cp .env.example .env
# fill in values
cargo run -p myso-identity-verification
```

Cron binary (must exit):

```bash
cargo run --bin myso-identity-verification-cron
```

## Ecosystem app integration

1. User logs in via MySocial Auth (salt service).
2. App calls `POST /oauth/x/connect` with `Authorization: Bearer <session_access_token>`.
3. Redirect user to returned `authorize_url`.
4. After callback, read `xUsername` and badges from **GraphQL** (not this service).
5. For share badges: `POST /campaigns/share/start`, poll `GET /campaigns/share/status` or wait for indexer badge.

Configure frontend env:

```bash
NEXT_PUBLIC_IDENTITY_VERIFICATION_API_URL=https://identity-verification.testnet.mysocial.network
```

## Badge names (on-chain)

| Badge | `badge_name` |
|-------|--------------|
| Verified X Account | `verified_x_account` |
| Early Adopter | `early_adopter` |
| Ambassador | `ambassador` |

Badge IDs: `ecosystem_badge_{relayer_address}_{badge_name}`
