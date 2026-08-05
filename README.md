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

### Session JWT validation

DripDrop / MySocial access tokens are **EdDSA** JWTs from salt-service:

| Claim / field | Value |
|---------------|--------|
| `iss` | `https://salt.testnet.mysocial.network` |
| `alg` | `EdDSA` |
| Wallet | `wallet_address` |
| JWKS | `https://salt.testnet.mysocial.network/.well-known/jwks.json` |

Configure (or rely on built-in salt testnet defaults):

```bash
MYSOCIAL_AUTH_ISSUER=https://salt.testnet.mysocial.network
MYSOCIAL_AUTH_JWKS_URI=https://salt.testnet.mysocial.network/.well-known/jwks.json
```

The API also accepts optional HS256 tokens when `JWT_SIGNING_KEY` matches, and RS256/EdDSA via any issuer registered in those env vars.

## Architecture

```
Auth (salt-service) → Profile on-chain
Identity Verification → relayer txs → profile.move events
myso-indexer-alt-social → GraphQL / social-server (read path)
In-API scheduler → process pending share campaigns at exact check_after (Redis ZSET)
```

Share campaigns enqueue a job with `check_after = tweet.created_at + 24h`. A background scheduler loop in the API sleeps until the next deadline, then re-checks the tweet and assigns the badge. No external cron service is required.

## Railway deployment

Single always-on service:

| Service | Start command | Notes |
|---------|---------------|-------|
| `identity-verification-api` | `./myso-identity-verification` | `/health`, in-process campaign scheduler |

Add-ons: **Redis** (OAuth token store + pending campaign queue — not source of truth).

## Environment variables

See [`.env.example`](.env.example).

The MySocial on-chain package ID is hardcoded to `0x50c1` (testnet) in `src/config.rs` — no env var needed.

### X credentials

Only OAuth app credentials are required:

| Variable | Purpose |
|----------|---------|
| `X_CLIENT_ID` + `X_CLIENT_SECRET` | OAuth 2.0 app credentials for `/oauth/x/connect` and `/oauth/x/callback` |
| `X_CALLBACK_URL` | OAuth redirect URI registered in the X Developer Portal |

After OAuth, refresh tokens are stored in Redis (encrypted) and used server-side for tweet lookups, following lists, and 24h re-checks. Clients never pass X access tokens to this service.

Create an OAuth 2.0 Web App (Confidential Client) in the X Developer Portal with callback URL matching `X_CALLBACK_URL` and scopes: `tweet.read`, `users.read`, `follows.read`, `offline.access`.

### X OAuth callback responses

`GET /oauth/x/callback` returns JSON:

| Outcome | Example response |
|---------|------------------|
| Success | `{ "status": "verified", "tx_digest": "...", "x_username": "..." }` |
| User denied on X | `{ "error": "x oauth denied: access_denied — ..." }` |
| Callback hit without completing OAuth | `{ "error": "missing authorization code — start from POST /oauth/x/connect and approve the X app" }` |

Always start a fresh flow with `POST /oauth/x/connect` and open the returned `authorize_url` in a browser.

## API

| Method | Path | Description |
|--------|------|-------------|
| GET | `/health` | Liveness |
| POST | `/oauth/x/connect` | Start X OAuth (requires session JWT) |
| GET | `/oauth/x/callback` | Complete X OAuth, issue verified badge + x_username |
| GET | `/verification/x?address=0x...` | Read verification status from indexer |
| GET | `/social-graph/x/matches?address=0x...` | Match X following to MySocial profiles (requires prior X OAuth) |
| POST | `/campaigns/share/start` | Enqueue share campaign (early_adopter / ambassador) |
| GET | `/campaigns/share/status?address=0x...` | Share campaign status for all badges |
| GET | `/campaigns/share/status?address=0x...&badge=early_adopter\|ambassador` | Share campaign status for one badge |

### Share campaign status

`GET /campaigns/share/status` requires `address`. `badge` is optional.

**All badges** (omit `badge`):

```bash
curl "http://localhost:3007/campaigns/share/status?address=0x..."
```

```json
{
  "early_adopter": { "status": "not_started" },
  "ambassador": { "status": "not_started" }
}
```

**Single badge** (`early_adopter` or `ambassador`):

```bash
curl "http://localhost:3007/campaigns/share/status?address=0x...&badge=early_adopter"
```

```json
{ "status": "not_started" }
```

Possible `status` values: `not_started`, `pending` (includes `check_after`), `completed` (includes optional `tx_digest`), `failed` (includes `reason`).

## Local development

```bash
cp .env.example .env
# fill in values
cargo run -p myso-identity-verification
```

Stop the server with `Ctrl+C` (or SIGTERM in Docker/Railway). If the port is still in use after stopping, kill the orphaned process: `lsof -ti :3007 | xargs kill`.

## Ecosystem app integration

1. User logs in via MySocial Auth (salt service).
2. App calls `POST /oauth/x/connect` with `Authorization: Bearer <session_access_token>`.
3. Redirect user to returned `authorize_url`.
4. After callback, read `xUsername` and badges from **GraphQL** (not this service).
5. For share badges: `POST /campaigns/share/start`, poll `GET /campaigns/share/status` or wait for indexer badge.
6. For follow recommendations: `GET /social-graph/x/matches?address=0x...` with session JWT.

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
