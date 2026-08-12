use sqlx::postgres::PgPool;
use sqlx::Row;

use crate::pg::PgError;

pub const CELL_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS cell (
    cell_id            text PRIMARY KEY,
    region             text   NOT NULL,
    status             text   NOT NULL,
    isolation_kind     text   NOT NULL,
    tenants_max        bigint NOT NULL,
    write_qps_max      bigint NOT NULL,
    storage_bytes_max  bigint NOT NULL,
    utilisation        smallint NOT NULL,
    version            bigint NOT NULL,
    endpoint           text   NOT NULL
);";

pub const TENANT_PLACEMENT_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS tenant_placement (
    tenant_id      text PRIMARY KEY,
    region         text   NOT NULL,
    home_cell      text   NOT NULL,
    isolation_tier text   NOT NULL,
    slug           text   NOT NULL,
    status         text   NOT NULL,
    member_cells   text[] NOT NULL
);";

pub const PLACEMENT_INVARIANT_FN: &str = "\
CREATE OR REPLACE FUNCTION myelin_placement_invariant() RETURNS trigger AS $myelin$
DECLARE
    cell_to_check text;
    found_region  text;
BEGIN
    -- (3) region immutability (§5.3 layer 1): a region change is a new-tenant/new-cell, never UPDATE.
    IF TG_OP = 'UPDATE' AND OLD.region IS DISTINCT FROM NEW.region THEN
        RAISE EXCEPTION 'placement invariant REJECTED tenant %: region is immutable (§5.3 layer 1) — a region change is a new-tenant-+-DSR, never an UPDATE (was %, attempted %)', NEW.tenant_id, OLD.region, NEW.region
            USING ERRCODE = 'check_violation';
    END IF;
    -- {home_cell} ∪ member_cells — the home cell is always part of the checked set.
    FOREACH cell_to_check IN ARRAY (ARRAY[NEW.home_cell] || NEW.member_cells)
    LOOP
        SELECT region INTO found_region FROM cell WHERE cell_id = cell_to_check;
        IF NOT FOUND THEN
            -- (1) fail-closed: never admit a placement whose region pin cannot be verified.
            RAISE EXCEPTION 'placement invariant REJECTED tenant %: cell % is not registered — a placement whose region pin cannot be verified is refused (fail-closed, §5.3)', NEW.tenant_id, cell_to_check
                USING ERRCODE = 'check_violation';
        END IF;
        IF found_region <> NEW.region THEN
            -- (2) the headline invariant: 0 cross-region member cells (single-region by construction).
            RAISE EXCEPTION 'placement invariant REJECTED tenant %: cell % is in region % but the tenant is pinned to region % — every cell in {home_cell} U member_cells must be in the tenant region (multi-cell is single-region by construction, §5.1). 0 cross-region member cells are admitted.', NEW.tenant_id, cell_to_check, found_region, NEW.region
                USING ERRCODE = 'check_violation';
        END IF;
    END LOOP;
    RETURN NEW;
END;
$myelin$ LANGUAGE plpgsql;";

pub const PLACEMENT_INVARIANT_TRIGGER: &str = "\
CREATE OR REPLACE TRIGGER trg_placement_invariant \
    BEFORE INSERT OR UPDATE ON tenant_placement \
    FOR EACH ROW EXECUTE FUNCTION myelin_placement_invariant();";

pub const REPO_PLACEMENT_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS repo_placement (
    repo_ref      text PRIMARY KEY,
    tenant_id     text NOT NULL
                  REFERENCES tenant_placement(tenant_id) ON DELETE RESTRICT,
    cell_id       text NOT NULL,
    storage_group text NOT NULL
);";

pub const CELL_PROVISIONING_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS cell_provisioning (
    id          bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    cell_id     text NOT NULL,
    step        text NOT NULL,
    outcome     text NOT NULL,
    recorded_at timestamptz NOT NULL DEFAULT now()
);";

pub const LOCAL_TENANT_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS local_tenant (
    cell_id        text NOT NULL,
    tenant_id      text NOT NULL,
    isolation_tier text NOT NULL,
    active         boolean NOT NULL,
    PRIMARY KEY (cell_id, tenant_id)
);";

pub const REPO_PLACEMENT_INVARIANT_FN: &str = "\
CREATE OR REPLACE FUNCTION myelin_repo_placement_invariant() RETURNS trigger AS $myelin$
DECLARE
    tenant_region text;
    cell_region   text;
BEGIN
    SELECT region INTO tenant_region FROM tenant_placement WHERE tenant_id = NEW.tenant_id;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'repo placement REJECTED: repo % — its tenant % is not placed; a repo cannot be homed onto a region of record that does not exist (fail-closed, §5.2)', NEW.repo_ref, NEW.tenant_id
            USING ERRCODE = 'check_violation';
    END IF;
    SELECT region INTO cell_region FROM cell WHERE cell_id = NEW.cell_id;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'repo placement REJECTED: repo % — cell % is not registered; a placement whose region pin cannot be verified is refused (fail-closed, §5.3)', NEW.repo_ref, NEW.cell_id
            USING ERRCODE = 'check_violation';
    END IF;
    IF cell_region <> tenant_region THEN
        RAISE EXCEPTION 'repo placement REJECTED: repo % — cell % is in region % but the repo tenant % is pinned to region % (the residency pin holds at repo grain, §5.2; a repo never leaves its tenant region)', NEW.repo_ref, NEW.cell_id, cell_region, NEW.tenant_id, tenant_region
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END;
$myelin$ LANGUAGE plpgsql;";

pub const REPO_PLACEMENT_INVARIANT_TRIGGER: &str = "\
CREATE OR REPLACE TRIGGER trg_repo_placement_invariant \
    BEFORE INSERT OR UPDATE ON repo_placement \
    FOR EACH ROW EXECUTE FUNCTION myelin_repo_placement_invariant();";

pub const MISROUTE_AUDIT_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS misroute_audit (
    id              bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id       text NOT NULL,
    received_by_cell text NOT NULL,
    home_cell       text,
    recorded_at     timestamptz NOT NULL DEFAULT now()
);";

pub fn placement_durable_migrations() -> crate::migration::Migrations {
    use crate::migration::{Migration, Migrations};
    Migrations::of([
        Migration::plain("0030_cell", CELL_MIGRATION),
        Migration::plain("0031_tenant_placement", TENANT_PLACEMENT_MIGRATION),
        Migration::plain("0032_placement_invariant_fn", PLACEMENT_INVARIANT_FN),
        Migration::plain(
            "0033_placement_invariant_trigger",
            PLACEMENT_INVARIANT_TRIGGER,
        ),
        Migration::plain("0034_misroute_audit", MISROUTE_AUDIT_MIGRATION),
        Migration::plain("0035_repo_placement", REPO_PLACEMENT_MIGRATION),
        Migration::plain("0036_cell_provisioning", CELL_PROVISIONING_MIGRATION),
        Migration::plain("0037_local_tenant", LOCAL_TENANT_MIGRATION),
        Migration::plain(
            "0038_repo_placement_invariant_fn",
            REPO_PLACEMENT_INVARIANT_FN,
        ),
        Migration::plain(
            "0039_repo_placement_invariant_trigger",
            REPO_PLACEMENT_INVARIANT_TRIGGER,
        ),
    ])
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableCellRow {
    pub cell_id: String,
    pub region: String,
    pub status: String,
    pub isolation_kind: String,
    pub tenants_max: i64,
    pub write_qps_max: i64,
    pub storage_bytes_max: i64,
    pub utilisation: i16,
    pub version: i64,
    pub endpoint: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurablePlacementRow {
    pub tenant_id: String,
    pub region: String,
    pub home_cell: String,
    pub isolation_tier: String,
    pub slug: String,
    pub status: String,
    pub member_cells: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableRepoPlacementRow {
    pub repo_ref: String,
    pub tenant_id: String,
    pub cell_id: String,
    pub storage_group: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableCellProvisioningRow {
    pub cell_id: String,
    pub step: String,
    pub outcome: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableLocalTenantRow {
    pub cell_id: String,
    pub tenant_id: String,
    pub isolation_tier: String,
    pub active: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableMisrouteRecord {
    pub tenant_id: String,
    pub received_by_cell: String,
    pub home_cell: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlacementWriteError {
    InvariantRejected(String),
    Db(String),
}

impl core::fmt::Display for PlacementWriteError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PlacementWriteError::InvariantRejected(why) => write!(
                f,
                "placement write REJECTED by the DB placement-invariant trigger (the write did NOT \
                 land): {why}"
            ),
            PlacementWriteError::Db(why) => {
                write!(
                    f,
                    "placement write failed (DB error, the write did NOT land): {why}"
                )
            }
        }
    }
}

impl std::error::Error for PlacementWriteError {}

#[derive(Clone)]
pub struct DurablePlacementBacking {
    pool: PgPool,
}

impl DurablePlacementBacking {
    pub fn new(pool: PgPool) -> DurablePlacementBacking {
        DurablePlacementBacking { pool }
    }

    pub async fn insert_cell(&self, c: &DurableCellRow) -> Result<(), PgError> {
        sqlx::query(
            "INSERT INTO cell \
               (cell_id, region, status, isolation_kind, tenants_max, write_qps_max, \
                storage_bytes_max, utilisation, version, endpoint) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) \
             ON CONFLICT (cell_id) DO UPDATE SET \
               status = EXCLUDED.status, isolation_kind = EXCLUDED.isolation_kind, \
               tenants_max = EXCLUDED.tenants_max, write_qps_max = EXCLUDED.write_qps_max, \
               storage_bytes_max = EXCLUDED.storage_bytes_max, utilisation = EXCLUDED.utilisation, \
               version = EXCLUDED.version, endpoint = EXCLUDED.endpoint",
        )
        .bind(&c.cell_id)
        .bind(&c.region)
        .bind(&c.status)
        .bind(&c.isolation_kind)
        .bind(c.tenants_max)
        .bind(c.write_qps_max)
        .bind(c.storage_bytes_max)
        .bind(c.utilisation)
        .bind(c.version)
        .bind(&c.endpoint)
        .execute(&self.pool)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(())
    }

    pub async fn get_cell(&self, cell_id: &str) -> Result<Option<DurableCellRow>, PgError> {
        let row = sqlx::query(
            "SELECT cell_id, region, status, isolation_kind, tenants_max, write_qps_max, \
                    storage_bytes_max, utilisation, version, endpoint \
             FROM cell WHERE cell_id = $1",
        )
        .bind(cell_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(row.map(|r| DurableCellRow {
            cell_id: r.get("cell_id"),
            region: r.get("region"),
            status: r.get("status"),
            isolation_kind: r.get("isolation_kind"),
            tenants_max: r.get("tenants_max"),
            write_qps_max: r.get("write_qps_max"),
            storage_bytes_max: r.get("storage_bytes_max"),
            utilisation: r.get("utilisation"),
            version: r.get("version"),
            endpoint: r.get("endpoint"),
        }))
    }

    pub async fn all_cells(&self) -> Result<Vec<DurableCellRow>, PgError> {
        let rows = sqlx::query(
            "SELECT cell_id, region, status, isolation_kind, tenants_max, write_qps_max, \
                    storage_bytes_max, utilisation, version, endpoint \
             FROM cell ORDER BY cell_id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(rows
            .iter()
            .map(|r| DurableCellRow {
                cell_id: r.get("cell_id"),
                region: r.get("region"),
                status: r.get("status"),
                isolation_kind: r.get("isolation_kind"),
                tenants_max: r.get("tenants_max"),
                write_qps_max: r.get("write_qps_max"),
                storage_bytes_max: r.get("storage_bytes_max"),
                utilisation: r.get("utilisation"),
                version: r.get("version"),
                endpoint: r.get("endpoint"),
            })
            .collect())
    }

    pub async fn cell_count(&self) -> Result<i64, PgError> {
        sqlx::query_scalar("SELECT COUNT(*) FROM cell")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| PgError::Query(e.to_string()))
    }

    pub async fn place_tenant(&self, p: &DurablePlacementRow) -> Result<(), PlacementWriteError> {
        let res = sqlx::query(
            "INSERT INTO tenant_placement \
               (tenant_id, region, home_cell, isolation_tier, slug, status, member_cells) \
             VALUES ($1,$2,$3,$4,$5,$6,$7) \
             ON CONFLICT (tenant_id) DO UPDATE SET \
               home_cell = EXCLUDED.home_cell, isolation_tier = EXCLUDED.isolation_tier, \
               slug = EXCLUDED.slug, status = EXCLUDED.status, member_cells = EXCLUDED.member_cells",
        )
        .bind(&p.tenant_id)
        .bind(&p.region)
        .bind(&p.home_cell)
        .bind(&p.isolation_tier)
        .bind(&p.slug)
        .bind(&p.status)
        .bind(&p.member_cells)
        .execute(&self.pool)
        .await;
        match res {
            Ok(_) => Ok(()),
            Err(e) => {
                let is_invariant = e
                    .as_database_error()
                    .and_then(|d| d.code())
                    .map(|c| c == "23514")
                    .unwrap_or(false);
                if is_invariant {
                    Err(PlacementWriteError::InvariantRejected(e.to_string()))
                } else {
                    Err(PlacementWriteError::Db(e.to_string()))
                }
            }
        }
    }

    pub async fn get_placement(
        &self,
        tenant_id: &str,
    ) -> Result<Option<DurablePlacementRow>, PgError> {
        let row = sqlx::query(
            "SELECT tenant_id, region, home_cell, isolation_tier, slug, status, member_cells \
             FROM tenant_placement WHERE tenant_id = $1",
        )
        .bind(tenant_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(row.map(|r| DurablePlacementRow {
            tenant_id: r.get("tenant_id"),
            region: r.get("region"),
            home_cell: r.get("home_cell"),
            isolation_tier: r.get("isolation_tier"),
            slug: r.get("slug"),
            status: r.get("status"),
            member_cells: r.get("member_cells"),
        }))
    }

    pub async fn placement_count(&self) -> Result<i64, PgError> {
        sqlx::query_scalar("SELECT COUNT(*) FROM tenant_placement")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| PgError::Query(e.to_string()))
    }

    pub async fn all_placements(&self) -> Result<Vec<DurablePlacementRow>, PgError> {
        let rows = sqlx::query(
            "SELECT tenant_id, region, home_cell, isolation_tier, slug, status, member_cells \
             FROM tenant_placement ORDER BY tenant_id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(rows.iter().map(placement_from_row).collect())
    }

    pub async fn get_placement_by_slug(
        &self,
        slug: &str,
    ) -> Result<Option<DurablePlacementRow>, PgError> {
        let row = sqlx::query(
            "SELECT tenant_id, region, home_cell, isolation_tier, slug, status, member_cells \
             FROM tenant_placement WHERE slug = $1 ORDER BY tenant_id LIMIT 1",
        )
        .bind(slug)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(row.as_ref().map(placement_from_row))
    }

    pub async fn set_cell_status(&self, cell_id: &str, status: &str) -> Result<bool, PgError> {
        let res = sqlx::query("UPDATE cell SET status = $2 WHERE cell_id = $1")
            .bind(cell_id)
            .bind(status)
            .execute(&self.pool)
            .await
            .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(res.rows_affected() > 0)
    }

    pub async fn set_placement_status(
        &self,
        tenant_id: &str,
        status: &str,
    ) -> Result<bool, PgError> {
        let res = sqlx::query("UPDATE tenant_placement SET status = $2 WHERE tenant_id = $1")
            .bind(tenant_id)
            .bind(status)
            .execute(&self.pool)
            .await
            .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(res.rows_affected() > 0)
    }

    pub async fn upsert_repo_placement(
        &self,
        r: &DurableRepoPlacementRow,
    ) -> Result<(), PlacementWriteError> {
        let res = sqlx::query(
            "INSERT INTO repo_placement (repo_ref, tenant_id, cell_id, storage_group) \
             VALUES ($1,$2,$3,$4) \
             ON CONFLICT (repo_ref) DO UPDATE SET \
               tenant_id = EXCLUDED.tenant_id, cell_id = EXCLUDED.cell_id, \
               storage_group = EXCLUDED.storage_group",
        )
        .bind(&r.repo_ref)
        .bind(&r.tenant_id)
        .bind(&r.cell_id)
        .bind(&r.storage_group)
        .execute(&self.pool)
        .await;
        match res {
            Ok(_) => Ok(()),
            Err(e) => {
                let is_invariant = e
                    .as_database_error()
                    .and_then(|d| d.code())
                    .map(|c| c == "23514")
                    .unwrap_or(false);
                if is_invariant {
                    Err(PlacementWriteError::InvariantRejected(e.to_string()))
                } else {
                    Err(PlacementWriteError::Db(e.to_string()))
                }
            }
        }
    }

    pub async fn get_repo_placement(
        &self,
        repo_ref: &str,
    ) -> Result<Option<DurableRepoPlacementRow>, PgError> {
        let row = sqlx::query(
            "SELECT repo_ref, tenant_id, cell_id, storage_group \
             FROM repo_placement WHERE repo_ref = $1",
        )
        .bind(repo_ref)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(row.map(|r| DurableRepoPlacementRow {
            repo_ref: r.get("repo_ref"),
            tenant_id: r.get("tenant_id"),
            cell_id: r.get("cell_id"),
            storage_group: r.get("storage_group"),
        }))
    }

    pub async fn log_provisioning(&self, e: &DurableCellProvisioningRow) -> Result<(), PgError> {
        sqlx::query("INSERT INTO cell_provisioning (cell_id, step, outcome) VALUES ($1,$2,$3)")
            .bind(&e.cell_id)
            .bind(&e.step)
            .bind(&e.outcome)
            .execute(&self.pool)
            .await
            .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(())
    }

    pub async fn provisioning_log(&self) -> Result<Vec<DurableCellProvisioningRow>, PgError> {
        let rows = sqlx::query("SELECT cell_id, step, outcome FROM cell_provisioning ORDER BY id")
            .fetch_all(&self.pool)
            .await
            .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(rows
            .iter()
            .map(|r| DurableCellProvisioningRow {
                cell_id: r.get("cell_id"),
                step: r.get("step"),
                outcome: r.get("outcome"),
            })
            .collect())
    }

    pub async fn upsert_local_tenant(
        &self,
        e: &DurableLocalTenantRow,
    ) -> Result<Option<DurableLocalTenantRow>, PgError> {
        let prior = self.get_local_tenant(&e.cell_id, &e.tenant_id).await?;
        sqlx::query(
            "INSERT INTO local_tenant (cell_id, tenant_id, isolation_tier, active) \
             VALUES ($1,$2,$3,$4) \
             ON CONFLICT (cell_id, tenant_id) DO UPDATE SET \
               isolation_tier = EXCLUDED.isolation_tier, active = EXCLUDED.active",
        )
        .bind(&e.cell_id)
        .bind(&e.tenant_id)
        .bind(&e.isolation_tier)
        .bind(e.active)
        .execute(&self.pool)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(prior)
    }

    pub async fn get_local_tenant(
        &self,
        cell_id: &str,
        tenant_id: &str,
    ) -> Result<Option<DurableLocalTenantRow>, PgError> {
        let row = sqlx::query(
            "SELECT cell_id, tenant_id, isolation_tier, active \
             FROM local_tenant WHERE cell_id = $1 AND tenant_id = $2",
        )
        .bind(cell_id)
        .bind(tenant_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(row.map(|r| local_tenant_from_row(&r)))
    }

    pub async fn local_tenants(
        &self,
        cell_id: &str,
    ) -> Result<Vec<DurableLocalTenantRow>, PgError> {
        let rows = sqlx::query(
            "SELECT cell_id, tenant_id, isolation_tier, active \
             FROM local_tenant WHERE cell_id = $1 ORDER BY tenant_id",
        )
        .bind(cell_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(rows.iter().map(local_tenant_from_row).collect())
    }
}

fn placement_from_row(r: &sqlx::postgres::PgRow) -> DurablePlacementRow {
    DurablePlacementRow {
        tenant_id: r.get("tenant_id"),
        region: r.get("region"),
        home_cell: r.get("home_cell"),
        isolation_tier: r.get("isolation_tier"),
        slug: r.get("slug"),
        status: r.get("status"),
        member_cells: r.get("member_cells"),
    }
}

fn local_tenant_from_row(r: &sqlx::postgres::PgRow) -> DurableLocalTenantRow {
    DurableLocalTenantRow {
        cell_id: r.get("cell_id"),
        tenant_id: r.get("tenant_id"),
        isolation_tier: r.get("isolation_tier"),
        active: r.get("active"),
    }
}

#[derive(Clone)]
pub struct DurableMisrouteAuditBacking {
    pool: PgPool,
}

impl DurableMisrouteAuditBacking {
    pub fn new(pool: PgPool) -> DurableMisrouteAuditBacking {
        DurableMisrouteAuditBacking { pool }
    }

    pub async fn record(
        &self,
        tenant_id: &str,
        received_by_cell: &str,
        home_cell: Option<&str>,
    ) -> Result<(), PgError> {
        sqlx::query(
            "INSERT INTO misroute_audit (tenant_id, received_by_cell, home_cell) \
             VALUES ($1, $2, $3)",
        )
        .bind(tenant_id)
        .bind(received_by_cell)
        .bind(home_cell)
        .execute(&self.pool)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(())
    }

    pub async fn records(&self) -> Result<Vec<DurableMisrouteRecord>, PgError> {
        let rows = sqlx::query(
            "SELECT tenant_id, received_by_cell, home_cell FROM misroute_audit ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(rows
            .iter()
            .map(|r| DurableMisrouteRecord {
                tenant_id: r.get("tenant_id"),
                received_by_cell: r.get("received_by_cell"),
                home_cell: r.get("home_cell"),
            })
            .collect())
    }

    pub async fn count(&self) -> Result<i64, PgError> {
        sqlx::query_scalar("SELECT COUNT(*) FROM misroute_audit")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| PgError::Query(e.to_string()))
    }
}
