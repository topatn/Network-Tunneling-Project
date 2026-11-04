# PostgreSQL Backup & Disaster Recovery Procedure (T-106 / OPS-007)

**Status:** APPROVED & AUTHORITATIVE.
**Authoritative Requirement:** `OPS-007` (Database backup and restore are tested. Acceptance: A restore from backup into a clean database succeeds and the application passes a smoke test).
**Linked References:** `architecture.md` §2, `data-model.md` §4, `infra/environments/production.env.example`.
**Date:** 2026-11-02.

---

## 1. Objectives & RPO / RTO Targets

| Metric | Target | Method |
|---|---|---|
| **Recovery Point Objective (RPO)** | $\le 15\text{ minutes}$ | Continuous Write-Ahead Log (WAL) archiving to object storage |
| **Recovery Time Objective (RTO)** | $\le 30\text{ minutes}$ | Automated physical or logical snapshot restore with automated smoke test |
| **Integrity Verification** | 100% table & foreign key match | Checksummed dump artifacts + post-restore constraint queries |

---

## 2. Backup Architecture & Schedule

The production PostgreSQL database uses a dual-layer backup strategy:

1. **Daily Logical / Physical Snapshots:**
   - Scheduled daily at `03:00 UTC` during lowest player traffic.
   - Generated using `pg_dump -Fc` (custom compressed format) or cloud-managed storage snapshots.
   - Backups are encrypted at rest with `AES-256-GCM` before being uploaded to an off-site, immutable object storage bucket (e.g., AWS S3 with Object Lock or GCS with Retention Policy).
2. **Continuous WAL Archiving (Point-in-Time Recovery - PITR):**
   - PostgreSQL `archive_mode = on` with `archive_command = 'pgbackrest --stanza=gtt archive-push %p'`.
   - WAL segments are archived every 16 MB or every 5 minutes (`archive_timeout = 300`).
3. **Retention Policy:**
   - Daily snapshots retained for **30 days**.
   - Weekly snapshots retained for **90 days** (matching usage and audit event retention in `PRIV-003`).

---

## 3. Step-by-Step Disaster Recovery Runbook

When a disaster or catastrophic database corruption occurs, the operator follows these exact steps:

### 3.1 Step 1: Provision Clean Target Database
```bash
# Set environment variables for target PostgreSQL instance
export PGHOST="db.prod.internal"
export PGPORT="5432"
export PGUSER="postgres"
export PGPASSWORD="<DB_ADMIN_PASSWORD>"

# Create clean database for restoration
psql -c "DROP DATABASE IF EXISTS gtt_production_restored;"
psql -c "CREATE DATABASE gtt_production_restored;"
```

### 3.2 Step 2: Download & Decrypt Backup Artifact
```bash
# Retrieve latest encrypted snapshot from offsite object store
aws s3 cp s3://gtt-prod-backups/snapshots/gtt_prod_snapshot_latest.dump.enc ./

# Decrypt using backup key from KMS / Vault
openssl enc -d -aes-256-gcm -pbkdf2 \
  -in gtt_prod_snapshot_latest.dump.enc \
  -out gtt_prod_snapshot.dump \
  -pass file:/etc/vault/backup_decrypt_key.txt

# Verify checksum
sha256sum gtt_prod_snapshot.dump
```

### 3.3 Step 3: Restore Logical Backup
```bash
# Restore schema and data into clean target database
pg_restore -h "$PGHOST" -p "$PGPORT" -U "$PGUSER" \
  -d gtt_production_restored \
  --clean --if-exists --exit-on-error \
  gtt_prod_snapshot.dump
```

### 3.4 Step 4: Execute Post-Restore Smoke Test
Run the automated verification suite against the restored database:
```bash
python scripts/backup_restore_test.py \
  --host "$PGHOST" \
  --port "$PGPORT" \
  --user "gtt_prod_app" \
  --source-db "gtt_production_restored" \
  --target-db "gtt_smoke_test_verify"
```

The smoke test queries verify:
1. All 14 authoritative tables exist (`users`, `tunnels`, `mappings`, `relays`, `allocations`, `audit_events`, `admins`, etc.).
2. Foreign key consistency (0 orphaned tunnels or allocations).
3. `INV-1` compliance: 0 columns matching `%local_%` or `%target_%`.
4. `PRIV-006` compliance: 0 columns matching `%email%`.

### 3.5 Step 5: Switch Traffic
Once smoke tests pass:
1. Update application configuration `DATABASE_URL` in Vault or Kubernetes ConfigMap to point to `gtt_production_restored`.
2. Perform rolling restart of `gtt-control-plane` instances.
3. Verify `/v1/me` and `/admin/v1/audit-events` routes respond normally.

---

## 4. Periodic Rehearsal & Verification

- **CI Automation:** Automated backup and restore smoke test runs in `.github/workflows/ci.yml`.
- **Manual Exercise:** Quarterly dry-run and live restore rehearsals into staging environments.
