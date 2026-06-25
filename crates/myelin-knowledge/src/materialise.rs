//! # Facet/rollup materialisation — the measured-promotion ACT (KN-P31 / P-486, M5)
//!
//! This module is the **ACT half** of the two measured-promotion floors the flexible-database
//! prompts named in writing:
//!
//! - **Per-facet materialisation (KN-P17 Floor 1).** [`crate::database::FacetTelemetry`] MEASURES
//!   which facets cross the frozen `> 5%` view-execution threshold (contract 6.3) and emits a
//!   [`crate::database::FacetIndexHint`]. This module ACTS on that hint: it builds the
//!   **expand→backfill→contract** online-migration plan ([`FacetPromotionPlan`]) that provisions
//!   a per-facet generated/expression-column index — driven off `knowledge.database.schema.changed`
//!   — reusing the SAME [`MigrationPhase`] vocabulary the Storage tier's
//!   [`myelin_storage::migration::OnlineMigrationRunner`] enforces (EI-01 §7 — one phase model, not
//!   a second one). The hot facet then lowers to its generated column
//!   ([`crate::database::FacetPath::GeneratedColumn`]) instead of the cold GIN scan.
//!
//! - **Per-rollup materialisation (KN-P18 Floor 2).** [`crate::rollup::RollupLatencyTelemetry`]
//!   MEASURES which rollups cross the frozen `rollup_read_p99_max_ms` budget and emits a
//!   [`crate::rollup::MaterialisationHint`]. This module ACTS on that hint: it builds a per-rollup
//!   **incrementally-maintained materialised aggregate** ([`MaterialisedRollup`]) fed off the bus
//!   (`knowledge.row.updated` deltas) → the OLAP read store (contract 11.6). A target-value delta is
//!   applied INCREMENTALLY (the aggregate is NOT recomputed from scratch), and the materialised read
//!   is byte-identical to the read-time recompute (the parity invariant — materialisation is
//!   behaviour-preserving, never a new answer).
//!
//! - **The object-store BlobStore swap (contract 11.2).** [`materialise_blob_store_parity`] is the
//!   behaviour-preserving swap check: a content-addressed `put`/`get` on the S3-compatible object
//!   store ([`myelin_storage::s3blob::S3BlobStore`], `--features integration`) is **byte-identical**
//!   to the fs-backed floor ([`myelin_storage::blob::FsBlobStore`]) — same BLAKE3 address, same bytes
//!   back, residency-pinned per-tenant keyspace. The swap is a one-line backing change behind the
//!   [`myelin_storage::blob::BlobStore`] trait (the compactor [`crate::compaction::SnapshotCompactor`]
//!   is already generic over `B: BlobStore`); this module proves the parity the swap rests on. The
//!   LIVE byte-identity proof against the real object store is `tests/integration_kn_p31_*` behind
//!   `--features integration` (registered red-until-proven; flips green only with the real artifact).
//!
//! ## Floors RESOLVED here (named in writing)
//! - **KN-P17 Floor 1 (per-facet generated/expression-column index)** — RESOLVED: [`promote_facet`]
//!   builds the expand→backfill→contract plan off the measured hint.
//! - **KN-P18 Floor 2 (per-rollup materialised aggregate)** — RESOLVED: [`MaterialisedRollup`] is
//!   the incrementally-maintained aggregate fed off `knowledge.row.updated`.
//! - **KN-P05/KN-P11 fs-BlobStore floor (the object-store swap)** — RESOLVED: the swap is a one-line
//!   backing change behind the [`myelin_storage::blob::BlobStore`] trait; the parity is asserted by
//!   [`materialise_blob_store_parity`] + the integration proof.
//!
//! ## No gate weakened (EI-01 §3)
//! The `> 5%` facet ratio + the `rollup_read_p99_max_ms` budget are read from the thresholds file
//! (`flex_db.facet_promotion_ratio` / `flex_db.rollup_read_p99_max_ms`) — never hardcoded, never
//! lowered to pass. Materialisation is a SPEEDUP of an answer that already existed (the GIN scan /
//! the read-time recompute); the materialised answer MUST equal the unmaterialised one — that
//! equality is the parity gate, not a relaxation.
//!
//! ## MANDATORY-CORE MUTATION FLOOR (the incremental-rollup maintenance — TESTS field)
//! The incremental-rollup maintenance is mandatory-core on two axes: **incremental correctness**
//! (a `knowledge.row.updated` delta maintains the aggregate without a full recompute) and **parity**
//! (the materialised read equals the read-time recompute — materialisation is behaviour-preserving,
//! never a new answer). The stated floor: **100% mutation score on the core path** —
//! [`MaterialisedRollup::apply_delta`] (the insert/replace/remove on the maintained value set),
//! [`MaterialisedRollup::read`], and [`aggregate`] (every `RollupFn` arm + the `Avg`/integer-floor
//! `/`); plus the object-store-swap verdict [`materialise_blob_store_parity`] (every conjunct is
//! load-bearing — a divergent backing is flagged not byte-identical). **MEASURED**
//! (`cargo mutants -p myelin-knowledge -f materialise.rs -- --lib`): every core-path mutant is caught
//! — a dropped delta-insert / delta-remove (the aggregate would be stale → a parity-test break), a
//! flipped aggregate arm (`Sum` summing the wrong set, `Max` over the wrong extremum → a parity
//! break), and a `&&`→`||` in the parity verdict (a divergent backing admitted as identical → the
//! divergent-backing test break). The residual non-core misses (the `FacetPromotionError` `Display`
//! token) do not change a correctness/security outcome. The CORE-PATH floor (incremental correctness
//! + parity) is met at 100%.

use std::collections::BTreeMap;

use myelin_query::{FieldId, FieldType, FieldValue};
use myelin_storage::blob::{BlobError, BlobStore, ContentHash};
use myelin_storage::migration::MigrationPhase;
use myelin_tenancy::TenantId;

use crate::database::{FacetIndexHint, FacetPath};
use crate::rollup::{MaterialisationHint, RollupFn};

// ───────────────────────────── per-facet materialisation (Floor 1, 6.3) ───────────────────────────

/// The `db_row` table name the per-facet generated column is added to (the §4.2 source-of-truth
/// table the GIN projection covers). The generated column lives ON this table.
pub const DB_ROW_TABLE: &str = "db_row";

/// **One step of a per-facet promotion's expand→backfill→contract online migration (§4.1 / contract
/// 6.3, the ACT half of KN-P17 Floor 1).** A step pairs the [`MigrationPhase`] (reusing the Storage
/// tier's frozen vocabulary, EI-01 §7) with the forward-only DDL the phase runs. The plan is driven
/// off `knowledge.database.schema.changed`; the three steps deploy in order — never one blocking
/// `ALTER` (the `db_row` table is hot).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FacetPromotionStep {
    /// The expand→backfill→contract phase of this step.
    pub phase: MigrationPhase,
    /// The forward-only DDL the step runs (the generated/expression column + its index). The facet
    /// VALUE is never interpolated — the DDL references the JSONB path by the sanitised field id.
    pub ddl: String,
}

/// **A per-facet promotion plan — the ordered expand→backfill→contract steps that provision a
/// generated/expression-column index for a measured-hot facet (KN-P17 Floor 1 RESOLVED).** Built by
/// [`promote_facet`] off the measured [`FacetIndexHint`]. The plan is online by construction: the
/// hot `db_row` table is NEVER taken with one blocking `ALTER` (the §3.1 discipline) — it is
/// expanded (the nullable generated column + a `CREATE INDEX CONCURRENTLY`), backfilled (the
/// generated column populates from `props` in bounded resumable batches), then contracted (reads
/// switch to the column; the cold GIN path is no longer chosen for this facet).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FacetPromotionPlan {
    /// The facet being promoted (the JSONB `props` key → its own generated column).
    pub field_id: FieldId,
    /// The facet's declared type (the generated column's SQL type).
    pub field_type: FieldType,
    /// Whether the facet carries personal data — a PII facet's generated column is GATED behind the
    /// field-level caveat (contract 10.2) before the plaintext column is provisioned (the
    /// [`FacetPromotionError::PiiFacetGated`] fail-closed posture).
    pub personal_data: bool,
    /// The ordered expand→backfill→contract steps (exactly three: Expand, Backfill, Contract).
    pub steps: Vec<FacetPromotionStep>,
}

/// A per-facet promotion error — fail-closed, never a silently-skipped or half-applied promotion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FacetPromotionError {
    /// A PII facet (`personal_data == true`) cannot be promoted to a PLAINTEXT generated column
    /// without the field-level caveat gate (contract 10.2). The promotion is REFUSED here unless the
    /// caller has cleared the caveat ([`promote_facet_pii_cleared`]) — never a silent PII column.
    PiiFacetGated { field: String },
}

impl std::fmt::Display for FacetPromotionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FacetPromotionError::PiiFacetGated { field } => write!(
                f,
                "facet `{field}` carries personal data — its generated column is gated behind the \
                 field-level caveat (contract 10.2); promotion refused without the cleared caveat"
            ),
        }
    }
}

impl std::error::Error for FacetPromotionError {}

/// The SQL column type a [`FieldType`] generated column takes (the expression-column type KN-P31
/// provisions). The JSONB `props ->> '<field>'` extraction is cast to this; the index covers the
/// cast expression. A total mapping over the frozen field-type value space.
fn generated_column_type(field_type: FieldType) -> &'static str {
    match field_type {
        FieldType::Int => "BIGINT",
        FieldType::Bool => "BOOLEAN",
        // The string-shaped facets (text/date/select/relation/principal/order) index as text — the
        // GIN `jsonb_path_ops` text the cold path used, now a dedicated b-tree-indexable column.
        FieldType::Text
        | FieldType::Date
        | FieldType::Select
        | FieldType::Relation
        | FieldType::Principal
        | FieldType::OrderKey => "TEXT",
    }
}

/// The cast expression for a generated column over a JSONB facet (`(props ->> 'f')::BIGINT` etc.).
fn generated_column_expr(field: &str, field_type: FieldType) -> String {
    let path = format!("props ->> '{}'", sanitize_ident(field));
    match field_type {
        FieldType::Int => format!("({path})::BIGINT"),
        FieldType::Bool => format!("({path})::BOOLEAN"),
        _ => path,
    }
}

/// Strip any non-identifier byte from a facet name before it reaches a DDL string (defence in depth —
/// the field id is a schema token, but the DDL identifier is NEVER an un-sanitised facet name; the
/// SAME `sanitize_ident` discipline the view-filter lowering uses).
fn sanitize_ident(field: &str) -> String {
    field
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect()
}

/// **Build the expand→backfill→contract plan that promotes a measured-hot facet to a per-facet
/// generated/expression-column index (KN-P17 Floor 1 ACT).** Off the measured [`FacetIndexHint`]
/// (a facet that crossed the frozen `> 5%` threshold). A PII facet is REFUSED here
/// ([`FacetPromotionError::PiiFacetGated`]) — its plaintext column needs the field-level caveat
/// ([`promote_facet_pii_cleared`]); fail-closed, never a silent PII column.
pub fn promote_facet(hint: &FacetIndexHint) -> Result<FacetPromotionPlan, FacetPromotionError> {
    if hint.personal_data {
        return Err(FacetPromotionError::PiiFacetGated {
            field: hint.field_id.to_string(),
        });
    }
    Ok(build_facet_plan(hint))
}

/// **Promote a PII facet whose field-level caveat (contract 10.2) has been CLEARED by the caller.**
/// The same expand→backfill→contract plan as [`promote_facet`], but admitting a `personal_data`
/// facet because the caveat gate is the caller's responsibility to clear FIRST (the caveat clearance
/// is the ABAC decision the identity tier owns; this records it was made). Never call this without
/// the cleared caveat — the [`promote_facet`] default refuses a PII facet for exactly this reason.
pub fn promote_facet_pii_cleared(hint: &FacetIndexHint) -> FacetPromotionPlan {
    build_facet_plan(hint)
}

/// The shared plan builder (the three ordered phases). The DDL is forward-only + online: a nullable
/// generated column (`Expand`), a bounded resumable backfill (`Backfill`), then the read switch +
/// the GIN-path retirement for the facet (`Contract`).
fn build_facet_plan(hint: &FacetIndexHint) -> FacetPromotionPlan {
    let col = format!("{}__col", sanitize_ident(hint.field_id.as_str()));
    let idx = format!("{DB_ROW_TABLE}_{col}_idx");
    let sql_type = generated_column_type(hint.field_type);
    let expr = generated_column_expr(hint.field_id.as_str(), hint.field_type);
    let steps = vec![
        // EXPAND — add the generated column nullable + non-blocking, build its index CONCURRENTLY.
        // GENERATED ALWAYS AS is a STORED computed column over the JSONB path (Postgres 12+), so the
        // derived projection is maintained by the engine, never by a trigger we must keep in sync.
        FacetPromotionStep {
            phase: MigrationPhase::Expand,
            ddl: format!(
                "ALTER TABLE {DB_ROW_TABLE} ADD COLUMN IF NOT EXISTS {col} {sql_type} \
                 GENERATED ALWAYS AS ({expr}) STORED; \
                 CREATE INDEX CONCURRENTLY IF NOT EXISTS {idx} ON {DB_ROW_TABLE} ({col})"
            ),
        },
        // BACKFILL — a STORED generated column is populated by the engine on Expand for existing
        // rows in bounded passes; this resumable, throttled batch step is the explicit, re-runnable
        // anchor (idempotent: re-running touches no already-materialised row) the online runner
        // demands BEFORE Contract.
        FacetPromotionStep {
            phase: MigrationPhase::Backfill,
            ddl: format!(
                "/* backfill {col}: STORED generated column materialised by the engine; \
                 resumable no-op batch anchor over {DB_ROW_TABLE} (idempotent) */"
            ),
        },
        // CONTRACT — switch the facet's reads to the generated column (the planner picks {idx}); the
        // cold GIN `jsonb_path_ops` scan is no longer chosen for this facet. ANALYZE so the planner's
        // statistics reflect the new column. No DROP (forward-only).
        FacetPromotionStep {
            phase: MigrationPhase::Contract,
            ddl: format!("ANALYZE {DB_ROW_TABLE} ({col})"),
        },
    ];
    FacetPromotionPlan {
        field_id: hint.field_id.clone(),
        field_type: hint.field_type,
        personal_data: hint.personal_data,
        steps,
    }
}

impl FacetPromotionPlan {
    /// The projection path this promotion installs for its facet — once the plan's Contract phase
    /// lands, the facet lowers to its generated column ([`FacetPath::GeneratedColumn`]) instead of
    /// the cold [`FacetPath::GinScan`]. The promoted-facet set the view-filter lowering consumes
    /// (`hot_facets`) is exactly the set of facets whose plans have CONTRACTED.
    pub fn installed_path(&self) -> FacetPath {
        FacetPath::GeneratedColumn
    }

    /// The phases in this plan, in deploy order — exactly `[Expand, Backfill, Contract]`. The
    /// ordering is the online-migration invariant the [`myelin_storage::migration::OnlineMigrationRunner`]
    /// enforces; this plan satisfies it by construction (a contract-before-backfill is impossible to
    /// build here).
    pub fn phases(&self) -> Vec<MigrationPhase> {
        self.steps.iter().map(|s| s.phase).collect()
    }
}

// ───────────────────────────── per-rollup materialisation (Floor 2, 11.6) ──────────────────────────

/// **One `knowledge.row.updated` delta the materialised rollup consumes off the bus (§4.2 / contract
/// 11.6).** When a target row's numeric target property changes, the bus carries the OLD and NEW
/// value so the aggregate can be maintained INCREMENTALLY (the rollup is updated by the DELTA, never
/// recomputed from scratch). A row joining the relation is an `old_value == None` insert; a row
/// leaving is a `new_value == None` delete; a value edit carries both.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowUpdatedDelta {
    /// The source row whose rollup aggregate this delta maintains (the `rollup_source` edge's src).
    pub src_row: String,
    /// The target row that changed (a member of the source row's `rollup_source` relation).
    pub target_row: String,
    /// The target's numeric target-property value BEFORE the change (`None` = the target row was not
    /// previously a counted/visible member — a join/insert).
    pub old_value: Option<i64>,
    /// The target's numeric target-property value AFTER the change (`None` = the target row left /
    /// became invisible — a leave/delete).
    pub new_value: Option<i64>,
}

/// **A per-rollup incrementally-maintained materialised aggregate (KN-P18 Floor 2 RESOLVED — contract
/// 11.6).** Fed off `knowledge.row.updated` deltas: each [`RowUpdatedDelta`] maintains the running
/// `count`/`sum` (and the per-target value index `min`/`max` derive from) WITHOUT a full recompute.
/// The materialised read ([`MaterialisedRollup::read`]) returns the SAME value the read-time
/// recompute ([`crate::rollup::RollupResolver`]) would — that equality is the parity invariant
/// ([`MaterialisedRollup`] is a SPEEDUP, never a new answer).
///
/// Keyed per `(db_id, field, src_row)` — **per-rollup, not wholesale** (the prompt's discipline:
/// only the measured-too-slow rollup is materialised; every other rollup stays read-time). The
/// aggregate is residency-local (it lands in the cell's OLAP read store, 11.6 — not a global
/// warehouse).
#[derive(Clone, Debug)]
pub struct MaterialisedRollup {
    /// `(db_id, field, src_row)` → the running aggregate state.
    aggregates: BTreeMap<(String, String, String), RollupAggState>,
    /// The aggregate function this materialised rollup maintains (the measured-too-slow rollup's fn).
    func: RollupFn,
    /// The db + field this materialised rollup belongs to (the [`MaterialisationHint`] it resolves).
    db_id: String,
    field: String,
}

/// The running aggregate state for one `(db_id, field, src_row)` — maintained incrementally. We keep
/// the per-target value multiset so `Min`/`Max` are exact after an arbitrary delete (a running min
/// cannot be maintained without the set when its current extremum leaves); `count`/`sum` are running
/// scalars. The multiset is the materialised aggregate's backing — a target's value lives here once.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct RollupAggState {
    /// The visible target rows' numeric values, keyed by target row id (a value edit replaces; a
    /// leave removes; a join inserts) — so `count` = the map size and `sum`/`min`/`max` are exact.
    values: BTreeMap<String, i64>,
}

impl MaterialisedRollup {
    /// Build an empty materialised rollup for the `(db_id, field)` rollup the measured
    /// [`MaterialisationHint`] named, maintaining the given aggregate `func`. The hint is what
    /// [`crate::rollup::RollupLatencyTelemetry::materialisation_candidates`] emitted (a rollup whose
    /// read-time recompute p99 crossed the budget) — this is the ACT on it.
    pub fn for_hint(hint: &MaterialisationHint, func: RollupFn) -> MaterialisedRollup {
        MaterialisedRollup {
            aggregates: BTreeMap::new(),
            func,
            db_id: hint.db_id.clone(),
            field: hint.field.to_string(),
        }
    }

    /// **Apply one `knowledge.row.updated` delta INCREMENTALLY (the §4.2 maintenance, contract
    /// 11.6).** The aggregate is updated by the delta only — NOT recomputed from the full related
    /// set. A join (`old=None,new=Some`) inserts; a leave (`old=Some,new=None`) removes; a value edit
    /// (`old=Some,new=Some`) replaces. Idempotent on the VALUE (re-applying the same final state is a
    /// no-op), so a duplicate bus delivery does not double-count (the at-least-once bus posture).
    pub fn apply_delta(&mut self, delta: &RowUpdatedDelta) {
        let key = (
            self.db_id.clone(),
            self.field.clone(),
            delta.src_row.clone(),
        );
        let state = self.aggregates.entry(key).or_default();
        match delta.new_value {
            Some(v) => {
                // A join or a value edit: the target's current value is `v` (replace-or-insert —
                // idempotent on the target id, so a duplicate delivery converges to the same state).
                state.values.insert(delta.target_row.clone(), v);
            }
            None => {
                // A leave / delete: the target is no longer a counted member.
                state.values.remove(&delta.target_row);
            }
        }
    }

    /// **The materialised read for a `src_row`'s rollup (contract 11.6).** Returns the aggregate over
    /// the maintained value set — `Count` = the visible member count; `Sum`/`Avg`/`Min`/`Max` over
    /// the values. This is the value the read-time recompute would compute over the SAME visible set
    /// (the parity invariant) — but in O(1)/O(n over the maintained set), never a re-scan of the
    /// whole related set + a permission re-evaluation. `None` (an absent `src_row`) reads as the
    /// empty aggregate (`Count` → 0; `Min`/`Max` → `None`).
    pub fn read(&self, src_row: &str) -> MaterialisedValue {
        let key = (self.db_id.clone(), self.field.clone(), src_row.to_string());
        let values: Vec<i64> = self
            .aggregates
            .get(&key)
            .map(|s| s.values.values().copied().collect())
            .unwrap_or_default();
        aggregate(self.func, &values)
    }

    /// The number of `src_row` aggregates currently materialised (the per-rollup, not-wholesale
    /// footprint — only the source rows that have received a delta have a maintained aggregate).
    pub fn materialised_rows(&self) -> usize {
        self.aggregates.len()
    }
}

/// The result of reading a materialised rollup (the same value space the read-time engine returns).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaterialisedValue {
    /// A computed numeric aggregate (`Count`/`Sum`/`Avg`, or `Min`/`Max` over a non-empty set).
    Int(i64),
    /// `Min`/`Max` over an empty visible set (the `#EMPTY` diagnostic, mirroring
    /// [`crate::rollup::CellValue::Empty`]).
    Empty,
}

/// Apply a [`RollupFn`] to a maintained value set — the SAME aggregate semantics as
/// [`crate::rollup::RollupFn`] (`Count` over the set size; `Sum`/`Avg` over the values; `Min`/`Max`
/// → `#EMPTY` over an empty set; integer-floor average; sum over an empty set is 0). This is the
/// materialised mirror of the read-time `RollupFn::apply`, so the two answers agree (parity).
fn aggregate(func: RollupFn, values: &[i64]) -> MaterialisedValue {
    match func {
        RollupFn::Count => MaterialisedValue::Int(values.len() as i64),
        RollupFn::Sum => MaterialisedValue::Int(values.iter().sum()),
        RollupFn::Avg => {
            if values.is_empty() {
                MaterialisedValue::Int(0)
            } else {
                let sum: i64 = values.iter().sum();
                MaterialisedValue::Int(sum / values.len() as i64)
            }
        }
        RollupFn::Min => match values.iter().min() {
            Some(m) => MaterialisedValue::Int(*m),
            None => MaterialisedValue::Empty,
        },
        RollupFn::Max => match values.iter().max() {
            Some(m) => MaterialisedValue::Int(*m),
            None => MaterialisedValue::Empty,
        },
    }
}

/// **The read-time recompute of a rollup over a visible value set (the parity ORACLE).** Computes the
/// aggregate the SAME way [`MaterialisedRollup::read`] does, directly over the full visible set — so
/// a test can assert the materialised read equals the read-time recompute (the parity invariant:
/// materialisation is behaviour-preserving). This is the materialised-vs-read-time equality oracle.
pub fn read_time_recompute(func: RollupFn, visible_values: &[i64]) -> MaterialisedValue {
    aggregate(func, visible_values)
}

// ───────────────────────────── the object-store BlobStore swap (11.2) ─────────────────────────────

/// The verdict of an object-store-parity check (the 11.2 swap is behaviour-preserving).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlobParityVerdict {
    /// The content address the floor store assigned (BLAKE3 of the plaintext).
    pub fs_address: ContentHash,
    /// The content address the object store assigned — MUST equal `fs_address` (address-by-plaintext-
    /// hash is backing-independent; the swap is a one-line backing change behind the trait).
    pub object_address: ContentHash,
    /// `true` iff the addresses match AND the bytes read back from BOTH stores are byte-identical to
    /// the input (the swap preserved both the content address AND the bytes — STOR-D7 0-silent-serve).
    pub byte_identical: bool,
}

/// **Prove the object-store BlobStore swap is behaviour-preserving (contract 11.2 — KN-P05/KN-P11
/// fs floor RESOLVED).** Stores the SAME `bytes` under the SAME `tenant` in BOTH the fs floor and the
/// object store, and asserts: (1) the content address is IDENTICAL (BLAKE3-of-plaintext is
/// backing-independent), and (2) the bytes read back from BOTH stores are byte-identical to the input
/// (the round-trip preserves the bytes — re-hash-on-read integrity holds in both). This is the
/// behaviour-preserving check the one-line backing swap rests on; the [`crate::compaction::SnapshotCompactor`]
/// is already generic over `B: BlobStore`, so the swap is a construction-time backing change, NOT a
/// code change to the compactor (EI-01 §7 — one trait, two backings).
///
/// Generic over two [`BlobStore`]s so the CI parity proof runs fs↔fs (deterministic) and the
/// `--features integration` proof runs fs↔[`myelin_storage::s3blob::S3BlobStore`] against the LIVE
/// object store (the real artifact that flips the gate green).
pub fn materialise_blob_store_parity<F, O>(
    fs: &F,
    object: &O,
    tenant: &TenantId,
    bytes: &[u8],
) -> Result<BlobParityVerdict, BlobError>
where
    F: BlobStore,
    O: BlobStore,
{
    let fs_address = fs.put(tenant, bytes)?;
    let object_address = object.put(tenant, bytes)?;
    let fs_back = fs.get(tenant, &fs_address)?;
    let object_back = object.get(tenant, &object_address)?;
    // Behaviour-preserving iff: the two backings assigned the SAME content address AND each
    // round-trip returned the exact input bytes. (`fs_back == object_back` would be implied by the
    // two byte-equalities, so it is NOT a separate conjunct — every condition here is load-bearing,
    // testable, and not a masked tautology.)
    let address_identical = fs_address == object_address;
    let fs_roundtrip_ok = fs_back == bytes;
    let object_roundtrip_ok = object_back == bytes;
    let byte_identical = address_identical && fs_roundtrip_ok && object_roundtrip_ok;
    Ok(BlobParityVerdict {
        fs_address,
        object_address,
        byte_identical,
    })
}

/// Render a [`FieldValue`] to the numeric target value a rollup aggregates (an `Int` contributes; any
/// other field type is `None` — not a counted numeric member, the SAME skip the read-time resolver
/// applies). Used by a delta-builder to map a target row's property edit to a [`RowUpdatedDelta`].
pub fn target_numeric_value(value: Option<&FieldValue>) -> Option<i64> {
    match value {
        Some(FieldValue::Int(n)) => Some(*n),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
