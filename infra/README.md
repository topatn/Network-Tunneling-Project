# Infrastructure & Environment Definitions (T-104 / OPS-001)

**Status:** COMPLETE & AUTHORITATIVE.
**Authoritative Requirement:** `OPS-001` (Development, staging, and production environments are separate; distinct configuration, credentials, and signing keys per environment; production secrets are not present in staging).
**Linked References:** `architecture.md` §2, §4, `requirements.md` (OPS-001), `docs/signing-key-rotation-runbook.md`, `contracts.md` §2.

---

## 1. Environment Architecture & Isolation Boundary

The system operates across three strictly isolated environments:

```
┌───────────────────────────┐   ┌───────────────────────────┐   ┌───────────────────────────┐
│     DEVELOPMENT (dev)     │   │     STAGING (staging)     │   │   PRODUCTION (production) │
├───────────────────────────┤   ├───────────────────────────┤   ├───────────────────────────┤
│ • Localhost (127.0.0.1)   │   │ • Staging VPC / Cluster   │   │ • Multi-Region Production │
│ • Local PostgreSQL (5432) │   │ • Staging DB (verify-full)│   │ • HA Prod DB (verify-full)│
│ • Mock signing keys       │   │ • Staging Vault keys      │   │ • Production HSM/KMS keys │
│ • Relaxed rate limits     │   │ • Full protocol mirrors   │   │ • Hardened rate limits    │
│ • HTTP allowed locally    │   │ • HTTPS & TLS 1.3 only    │   │ • HTTPS & TLS 1.3 only    │
└───────────────────────────┘   └───────────────────────────┘   └───────────────────────────┘
```

### 1.1 Strict Invariants (`OPS-001`, `AGENTS.md` §3 & §7)
1. **Separate Identity & Token Issuers:**
   - Development uses `TOKEN_ISSUER=gtt-control-plane-dev` with keys `dev-key-*`.
   - Staging uses `TOKEN_ISSUER=gtt-control-plane-staging` with keys `staging-key-*`.
   - Production uses `TOKEN_ISSUER=gtt-control-plane-production` with keys `prod-key-*`.
   - A token issued by one environment is rejected by relays in other environments due to issuer and `kid` mismatch (`SEC-004`).
2. **Zero Production Secret Leakage:**
   - Production secrets (database passwords, HMAC signing master keys, recovery hashes) **never appear** in staging or development configuration files or repositories (`SEC-002`, `OPS-001`).
   - Production secrets are injected at runtime via container secret injection (Docker secrets, Kubernetes secrets, or HashiCorp Vault).
3. **Database Isolation & TLS:**
   - Development allows local loopback connections (`sslmode=disable`).
   - Staging and production strictly enforce `sslmode=verify-full` with explicit root certificates (`sslrootcert`).

---

## 2. Directory Layout

```
infra/
├── README.md                          # Authoritative environment & runbook documentation
├── verify_environments.py             # Automated isolation & completeness checker (OPS-001)
├── docker-compose.dev.yml             # Local development database and support stack
├── docker-compose.staging.yml         # Staging integration environment compose definition
├── environments/
│   ├── dev.env.example                # Development configuration template
│   ├── staging.env.example            # Staging configuration template
│   └── production.env.example         # Production configuration template (placeholders only)
└── monitoring/                        # Staging observability & cost monitoring (T-103)
    ├── docker-compose.monitoring.yml  # Prometheus and Grafana staging monitoring stack
    ├── prometheus/
    │   ├── alert_rules.yml            # Alert definitions (OPS-002, OPS-006)
    │   └── prometheus.yml             # Scrape configurations and evaluation rules
    └── dashboards/
        ├── relay_health_and_saturation.json # Grafana dashboard for relay health and limits
        └── relay_egress_and_cost.json       # Grafana dashboard for egress cost & budgets
```

---

## 3. Secret Management & Key Rotation

- **Development:** Uses static insecure keys documented in `dev.env.example` solely for local unit and integration tests.
- **Staging:** Keys are managed via pre-production Vault/KMS. Staging key rotation exercises (`T-050`) execute against staging relays without impacting live production traffic.
- **Production:**
  - Master HMAC signing keys (`SigningKeyMaterial`) are provisioned via Cloud KMS / HSM.
  - Rotation procedures strictly follow `docs/signing-key-rotation-runbook.md`, maintaining an overlap window (`overlap_window_secs >= token_ttl_secs + max_clock_skew_secs`) to prevent guest disconnects during key updates (`SEC-016`).

---

## 4. Automated Verification

Execute the automated environment isolation and configuration validator:
```bash
python infra/verify_environments.py
```

This verification ensures:
1. All required environment variables are defined.
2. Complete environment separation (zero key, issuer, database, or endpoint collisions).
3. Production secrets are completely absent from staging and dev configs (`OPS-001`).
4. Strict SSL/TLS enforcement for staging and production (`sslmode=verify-full`).
