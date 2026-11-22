# Cloudflare Edge Exposure (Workers VPC + Tunnel)

Saltation exposes only the static web SPA (`web:8088`) and the bank public listener (`bank:8080`) through Cloudflare Tunnel + Cloudflare Workers VPC. The generator listener (`:8081`), metrics listener (`:9100`), Oracle (`:1521`), MongoDB (`:27017`), and observability stack are never attached to tunnel ingress (`spec.md` § Security).

## Topology

| Component | Worker | VPC Service | Host Loopback Target |
| --- | --- | --- | --- |
| Mobile Web SPA (`web`) | `sal` (`workers/sal.js`, binding `SAL`) | `sal` | `127.0.0.1:50080` (`WEB_PUBLISH`) |
| Bank Public API (`bank`) | `sal-api` (`workers/sal-api.js`, binding `SAL_API`) | `sal-api` | `127.0.0.1:50081` (`BANK_PUBLISH`) |

Using `50080`/`50081` avoids colliding with co-tenant services on the OCI development host (e.g. `jenkins` on `127.0.0.1:8080`).

## Host `.env` settings (`infra/compose/.env`, untracked)

```dotenv
WEB_PUBLISH=127.0.0.1:50080
BANK_PUBLISH=127.0.0.1:50081
SALTATION_API_BASE=https://sal-api.<your-subdomain>.workers.dev
SALTATION_CORS_ORIGINS=https://sal.<your-subdomain>.workers.dev
```

## Security Verification

Run the automated Stage 8 negative test suite on the host after deployment:

```sh
python3 scripts/ops/security_negative_test.py \
  --web-url https://sal.<your-subdomain>.workers.dev \
  --api-url https://sal-api.<your-subdomain>.workers.dev
```

It verifies:
1. Public `/healthz` and CORS preflight succeed.
2. `/atm/v1/session`, `/card/v1/authorize`, `/internal/v1/clock`, `/v2.0/transfer/deposit/fin_num`, and `/metrics` return `404` through the tunnel.
3. Spoofed `X-Sim-Ip`, `X-Sim-Region`, and `X-Sim-Device` headers are ignored on the public listener (`bank_logs.events` records `listener: "public"`, `human: true`, and the real Cloudflare IP/country).
4. MongoDB RBAC denies `bank` on `truth` and `generator` on `bank_logs`.
