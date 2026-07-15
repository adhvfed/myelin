//! # Durable PG backing for the control-plane placement registry (SI-011/SI-028, MR-024, the P-522/523 floor)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/tenancy-and-control-plane.md` §5.1 (the three
//! PII-free control-plane tables + the **HARD placement invariant** — every cell in
//! `{home_cell} ∪ member_cells` has `cell.region == tenant_placement.region`, specified as a **DB
//! trigger**) + §5.3 layer 1 (region immutable — no UPDATE path). Closes census SI-011 ("the
//! placement registry is an in-memory `BTreeMap`, lost on restart → every tenant's routing is lost")
//! and SI-028 ("the `MisrouteAudit` sink is `Arc<Mutex<Vec<…>>>`, lost on restart") at the
//! *production* level: this module is the REAL durable backing the control-plane
//! [`myelin_control_plane::registry::Registry`] + [`myelin_control_plane::placement_of::MisrouteAudit`]
//! bind to (the Pg arm of their backend enum), so tenant→cell routing + the misroute audit trail
//! **survive a process restart**.
//!
//! ## The placement invariant IS a REAL DB TRIGGER (the "in code on this floor → real trigger" upgrade)
//! The control-plane registry encodes the invariant in code ([`Registry::check_placement_invariant`]).
//! On this floor it becomes a real Postgres **`BEFORE INSERT OR UPDATE` trigger**
//! ([`PLACEMENT_INVARIANT_TRIGGER`] over [`PLACEMENT_INVARIANT_FN`]): a `tenant_placement` whose
//! `{home_cell} ∪ member_cells` contains a cell in a DIFFERENT region than the tenant — OR an
//! UNKNOWN cell — is **rejected by the database**, not merely by application code. A direct `INSERT`
//! (psql / a scratch test, bypassing all Rust) is refused; fail-closed on an unknown cell (the region
//! pin cannot be verified → never admit). This is the architecture's stated intent realized.
//!
//! ## Region immutability (architecture §5.3 layer 1) — there is NO update-region path
//! The trigger ALSO rejects any `UPDATE` that changes `region` (a region change is a
//! new-tenant-+-DSR / a new cell, never an UPDATE). The backing preserves the no-update-region rule:
//! it exposes no `update_*_region` method, and [`DurablePlacementBacking::place_tenant`]'s upsert
//! never writes `region` on the conflict path (`region` is set once at insert).
//!
//! ## Isolation posture — control-plane ROUTING infra, NOT a per-request tenant data store
//! The placement registry routes **ALL** tenants to cells; every query is cross-tenant by design (the
//! gateway asks "which cell homes tenant X?" for any X). It is PII-free (opaque ids only — the
//! `control-plane-pii-free` invariant) and is NOT a per-request tenant data store. So — exactly like
//! [`crate::pgrelay`] (the relay-internal outbox) and [`crate::events_durable`] (the
//! `(consumer, event_id)` dedup ledger) — it does NOT acquire through the per-request
//! [`crate::tenant_tx::with_tenant_tx`] / RLS convention (that convention is for per-tenant data
//! stores like `principal`/`rebac_tuple`). It connects to the OLTP pool directly. Its queries
//! therefore carry no per-row tenant predicate; this file is a NAMED, LOUD exclusion in the
//! `tenant-predicate` workspace scanner (documented here + in `tests/workspace_clean.rs`), never a
//! silent skip — and the lint stays FULLY live over the genuine tenant data stores (`pg.rs` /
//! `identity_durable.rs`). The `tenant_id` column here is the ROUTING KEY, not an RLS predicate.
//!
//! ## PII-free (architecture §3.3 / `control-plane-pii-free`)
//! Every column is an opaque id / region code / status enum / non-personal slug / aggregate count.
//! There is NO name/email/body anywhere — the human tenant name + admin email are born INSIDE the
//! assigned cell (two-phase signup, §6), never here. The durable rows mirror the frozen contract-12.3
//! `cell` / `tenant_placement` shapes ([`myelin_control_plane::schema`]); the storage layer holds
//! them as opaque text/ints (the control-plane layer owns the typed enums).
//!
//! Feature-gated `integration` (it pulls the real sqlx client), like the rest of the live-PG code.

use sqlx::postgres::PgPool;
use sqlx::Row;

use crate::pg::PgError;

// =================================================================================================
// Migrations — the `cell` + `tenant_placement` tables (frozen contract-12.3 shape) + the REAL
// placement-invariant TRIGGER + the `misroute_audit` sink. Applied via MR-022 `apply_validated`.
// =================================================================================================

/// The `cell` inventory table (architecture §5.1; contract 12.3). PII-free: opaque `cell_id` PK, the
/// **immutable** `region`, lifecycle `status`, `isolation_kind`, the aggregate capacity vector,
/// `utilisation`, schema `version`, and the PII-free routing `endpoint`. Forward-only (`IF NOT EXISTS`).
/// Mirrors [`myelin_control_plane::schema::Cell`]; the capacity vector is flattened to three columns.
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

/// The `tenant_placement` table (architecture §5.1; contract 12.3 — the registry-schema half).
/// PII-free: opaque `tenant_id` PK, the **immutable** `region`, the `home_cell`, the served
/// `isolation_tier`, the non-personal routing `slug`, the placement `status`, and the multi-cell
/// `member_cells` fan-out set (single-element in v1 — the floor). Forward-only (`IF NOT EXISTS`).
/// Mirrors [`myelin_control_plane::schema::TenantPlacement`]. The HARD placement invariant is the
/// TRIGGER below (NOT a column constraint — it is a cross-row predicate against `cell`).
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

/// **The HARD placement invariant, as a REAL Postgres trigger function (architecture §5.1 / §5.3).**
/// Realizes — at the DATABASE — the predicate the in-memory registry checks in code:
///   1. **Unknown cell → REJECT (fail-closed, §5.3).** A `home_cell`/`member_cell` not present in
///      `cell` cannot have its region pin verified, so the placement is refused.
///   2. **Cross-region cell → REJECT (the headline invariant, §5.1).** Every cell in
///      `{home_cell} ∪ member_cells` must be in the tenant's region; 0 cross-region cells admitted
///      (multi-cell is single-region by construction).
///   3. **Region immutable on UPDATE → REJECT (§5.3 layer 1).** A region change is a
///      new-tenant-+-DSR, never an UPDATE; an UPDATE that changes `region` is refused.
/// Each rejection RAISEs with SQLSTATE `check_violation` (23514) + a loud, named reason (EI-01 §3).
/// `CREATE OR REPLACE FUNCTION` is idempotent + forward-only-legal (no DROP).
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

/// **The placement-invariant trigger** (BEFORE INSERT OR UPDATE on `tenant_placement`) — fires
/// [`PLACEMENT_INVARIANT_FN`] for every row write, so a cross-region / unknown-cell / region-change
/// placement is rejected by the DATABASE before the row lands. `CREATE OR REPLACE TRIGGER` (PG14+;
/// the dev/prod stack is PG16) is idempotent + forward-only-legal.
pub const PLACEMENT_INVARIANT_TRIGGER: &str = "\
CREATE OR REPLACE TRIGGER trg_placement_invariant \
    BEFORE INSERT OR UPDATE ON tenant_placement \
    FOR EACH ROW EXECUTE FUNCTION myelin_placement_invariant();";

/// The `misroute_audit` sink (architecture §5.3 layer 4 — SI-028). PII-free: opaque tenant id, the
/// cell that received+rejected the request, and the home cell (NULL for an unknown tenant). An
/// auto-increment `id` keeps the append order; `recorded_at` is a server timestamp. Forward-only.
/// This is the durable form of [`myelin_control_plane::placement_of::MisrouteAudit`] so the audit
/// trail (the evidence the layer-4 defence fired) survives a process restart.
pub const MISROUTE_AUDIT_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS misroute_audit (
    id              bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id       text NOT NULL,
    received_by_cell text NOT NULL,
    home_cell       text,
    recorded_at     timestamptz NOT NULL DEFAULT now()
);";

/// The full forward-only migration set the durable placement registry binds to: the `cell` +
/// `tenant_placement` tables, the placement-invariant FUNCTION + TRIGGER, and the `misroute_audit`
/// sink. Applied via the MR-022 [`crate::provider::SubstrateProvider::migrate`] (validate → execute,
/// race-safe, version-recorded) at boot. Ids are stable + idempotent on re-boot. NOTE: NO RLS policy
/// is installed (control-plane routing infra is cross-tenant by design — see the module docs).
pub fn placement_durable_migrations() -> crate::migration::Migrations {
    use crate::migration::{Migration, Migrations};
    Migrations::of([
        Migration::plain("0030_cell", CELL_MIGRATION),
        Migration::plain("0031_tenant_placement", TENANT_PLACEMENT_MIGRATION),
        Migration::plain("0032_placement_invariant_fn", PLACEMENT_INVARIANT_FN),
        Migration::plain("0033_placement_invariant_trigger", PLACEMENT_INVARIANT_TRIGGER),
        Migration::plain("0034_misroute_audit", MISROUTE_AUDIT_MIGRATION),
    ])
}

// =================================================================================================
// Opaque storage-layer rows (the control-plane layer owns the typed enums; here every field is
// opaque text/int — PII-free). Mirror the frozen contract-12.3 shapes.
// =================================================================================================

/// A durable `cell` inventory row (opaque columns; the control-plane maps to/from its typed `Cell`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableCellRow {
    /// The opaque cell-routing id (PK).
    pub cell_id: String,
    /// The cell's **immutable** residency region.
    pub region: String,
    /// The lifecycle status (opaque text — the control-plane owns the enum).
    pub status: String,
    /// The isolation tier (opaque text).
    pub isolation_kind: String,
    /// Max tenants (aggregate).
    pub tenants_max: i64,
    /// Max sustained write QPS (aggregate).
    pub write_qps_max: i64,
    /// Max stored bytes (aggregate).
    pub storage_bytes_max: i64,
    /// Aggregate utilisation 0..=100.
    pub utilisation: i16,
    /// Deployed schema/software version.
    pub version: i64,
    /// PII-free routing endpoint.
    pub endpoint: String,
}

/// A durable `tenant_placement` row (opaque columns). The HARD invariant is enforced by the DB
/// TRIGGER at write time, not here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurablePlacementRow {
    /// The opaque tenant id (PK + routing key).
    pub tenant_id: String,
    /// The tenant's **immutable** residency region.
    pub region: String,
    /// The tenant's primary (home) cell.
    pub home_cell: String,
    /// The served isolation tier (opaque text).
    pub isolation_tier: String,
    /// The non-personal routing slug.
    pub slug: String,
    /// The placement lifecycle status (opaque text).
    pub status: String,
    /// The multi-cell fan-out set (single-element in v1).
    pub member_cells: Vec<String>,
}

/// One durable misroute audit record (PII-free — opaque ids only).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableMisrouteRecord {
    /// The tenant the misrouted request was for.
    pub tenant_id: String,
    /// The cell that received (and rejected) the request.
    pub received_by_cell: String,
    /// The cell that actually homes the tenant (`None` for an unknown tenant).
    pub home_cell: Option<String>,
}

/// The error of a `place_tenant` write — distinguishes the DB **placement-invariant trigger
/// rejection** (the load-bearing refusal) from an ordinary DB fault, so a caller (and the verifier)
/// can tell "the trigger fired" from "the DB was down".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlacementWriteError {
    /// The placement-invariant **DB TRIGGER** rejected the write — a cross-region cell, an unknown
    /// cell (fail-closed), or a region change. Carries the trigger's loud, named reason.
    InvariantRejected(String),
    /// A non-invariant DB error (connection/query fault) — the write did NOT succeed.
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
                write!(f, "placement write failed (DB error, the write did NOT land): {why}")
            }
        }
    }
}

impl std::error::Error for PlacementWriteError {}

// =================================================================================================
// DurablePlacementBacking — the `cell` + `tenant_placement` tables over the OLTP pool (NO RLS/tx —
// control-plane routing infra, cross-tenant by design; see the module docs).
// =================================================================================================

/// The REAL durable control-plane placement backing over the OLTP `PgPool`. Cloneable (the pool is an
/// `Arc`-backed handle). The control-plane [`Registry`] binds to this as the `Pg` arm of its backend
/// enum; the in-memory `BTreeMap` registry is the explicit test-double. Connects to the pool DIRECTLY
/// (no `with_tenant_tx`/RLS — control-plane infra is cross-tenant routing, not a tenant data store).
#[derive(Clone)]
pub struct DurablePlacementBacking {
    pool: PgPool,
}

impl DurablePlacementBacking {
    /// Wrap a pool as the durable placement backing. The caller must have applied
    /// [`placement_durable_migrations`] (via the MR-022 provider's `migrate`) so the `cell` /
    /// `tenant_placement` tables + the invariant trigger exist.
    pub fn new(pool: PgPool) -> DurablePlacementBacking {
        DurablePlacementBacking { pool }
    }

    /// Insert (upsert) a `cell` inventory row. A re-register (e.g. on restart) updates the MUTABLE
    /// columns (status / utilisation / version / endpoint / isolation_kind / capacity) but NEVER the
    /// `region` (immutable, §5.3 layer 1 — excluded from the conflict update). Idempotent.
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

    /// Read a `cell` by opaque id, or `None`.
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

    /// **Every `cell` inventory row, in stable id order.** The durable authority the control-plane
    /// [`myelin_control_plane`] `CellResolverRegistry` PROJECTS at boot (MR-009b W6c-cp): the
    /// cross-cell bridge rebuilds its live resolver handles from each cell's durable, PII-free routing
    /// `endpoint`, so the registry is a boot-time projection of this table, not an in-memory
    /// system-of-record. Ordered by `cell_id` so the projection is deterministic across reboots.
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

    /// The number of cells in the inventory.
    pub async fn cell_count(&self) -> Result<i64, PgError> {
        sqlx::query_scalar("SELECT COUNT(*) FROM cell")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| PgError::Query(e.to_string()))
    }

    /// **`place_tenant` — write a `tenant_placement` row; the DB TRIGGER enforces the HARD invariant.**
    /// The write is admitted IFF every cell in `{home_cell} ∪ member_cells` is registered AND in the
    /// tenant's region; otherwise the trigger REJECTS it ([`PlacementWriteError::InvariantRejected`])
    /// and nothing lands. Idempotent upsert; `region` is never overwritten (immutable — the trigger
    /// also rejects a region change). Returns `Ok(())` on a successful (admitted) write.
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
                // The trigger RAISEs with SQLSTATE check_violation (23514) — distinguish the
                // load-bearing invariant rejection from an ordinary DB fault.
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

    /// Read a `tenant_placement` row by opaque tenant id, or `None` (an unplaced/unknown tenant).
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

    /// The number of placed tenants.
    pub async fn placement_count(&self) -> Result<i64, PgError> {
        sqlx::query_scalar("SELECT COUNT(*) FROM tenant_placement")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| PgError::Query(e.to_string()))
    }
}

// =================================================================================================
// DurableMisrouteAuditBacking — the `misroute_audit` sink over the OLTP pool (SI-028).
// =================================================================================================

/// The REAL durable misroute audit backing over the OLTP `PgPool` (SI-028). Cloneable. The
/// control-plane [`MisrouteAudit`] binds to this so the audit trail survives a process restart.
/// PII-free (opaque ids only); cross-cell by design (the control plane audits routing for all cells),
/// so — like the placement tables — it carries no per-row tenant RLS predicate.
#[derive(Clone)]
pub struct DurableMisrouteAuditBacking {
    pool: PgPool,
}

impl DurableMisrouteAuditBacking {
    /// Wrap a pool as the durable misroute-audit backing. The caller must have applied
    /// [`placement_durable_migrations`] (which includes [`MISROUTE_AUDIT_MIGRATION`]).
    pub fn new(pool: PgPool) -> DurableMisrouteAuditBacking {
        DurableMisrouteAuditBacking { pool }
    }

    /// Record a rejected misroute (loud, never swallowed — the attempt IS evidence). Append-only.
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

    /// Every audited misroute, in append order (so a drill/test can assert the rejection was audited
    /// and survives a fresh instance).
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

    /// How many misroutes have been audited.
    pub async fn count(&self) -> Result<i64, PgError> {
        sqlx::query_scalar("SELECT COUNT(*) FROM misroute_audit")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| PgError::Query(e.to_string()))
    }
}
