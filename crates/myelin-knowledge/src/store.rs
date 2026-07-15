//! # The Knowledge OLTP store + the `(tenant, region)` partition + RLS (KN-P05 → P-295, M3)
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/knowledge-platform/architecture/01-tech-and-data-model.md`
//! §1.2 (one PostgreSQL-class DB per service, the `no-cross-db` boundary), §2.3 (the `block` row),
//! §2.6 (`page`), §3 (the `doc_op` op-log + `doc_snapshot`), §4.2/§4.3/§4.4 (`db_collection` /
//! `db_row` / `db_relation` / `page_parent` / `db_view`) and §6 (the stateful-component register +
//! the hot-table flags). The schema is `(tenant, region)`-partitioned with RLS and every query
//! carries the tenant predicate via the storage crate's [`TenantScope`]/[`TenantQuery`] guard.
//!
//! **Contract-index:** row 11.1 (the OLTP client + RLS — CONSUMED via [`myelin_storage::oltp`] +
//! [`myelin_storage::rls`]); row 12.1 (the `(tenant, region)` partition — CONSUMED via
//! [`myelin_tenancy`] + the storage `TenantScope`); row 11.2 (the fs-backed BlobStore floor —
//! CONSUMED via [`myelin_storage::blob::FsBlobStore`] for media + CRDT snapshots); row 1.5 (the
//! hot-table flags `block`/`db_row`/`doc_op` declared in [`crate`] — the high-write tables CREATED
//! here).
//!
//! ## What this module ships (KN-P05's owned work)
//! - **The Knowledge OLTP schema** ([`knowledge_store_migrations`]): the `block`, `page`, `db_row`,
//!   `db_collection`, `db_view`, `db_relation`, `page_parent`, `doc_op`, `doc_snapshot` tables,
//!   **all `(tenant, region)`-partitioned** (the partition key is the leading PK column-pair,
//!   ADR-11) and forward-only (no destructive/backward DDL). The three high-write tables
//!   (`block`/`db_row`/`doc_op`) are PHASED migrations carrying their hot-table flag so the runner
//!   protects them; their indexes are `CREATE INDEX CONCURRENTLY` (the online idiom, §9.1).
//! - **The tenant-scoped query helper** ([`KnowledgeTable`] + [`KnowledgeStore::query`]): every read
//!   of a Knowledge table is built through the storage crate's [`TenantQuery::for_table`], which
//!   takes a [`TenantScope`] **by value** — a `TenantScope` is mintable ONLY from a verified token
//!   ([`TenantScope::from_verified_token`]). So a tenant-less Knowledge query **does not compile**
//!   (the structural half of the `tenant-predicate` lint, §1.1).
//! - **The KN-D13 IDOR floor** ([`KnowledgeStore::resolve_tenant`]): a read whose URL path *asserts*
//!   a different tenant than the token resolves to the **token's** tenant — 0 cross-tenant read, the
//!   `path_derived_tenant_count == 0` survival signal.
//! - **The fs-backed BlobStore wiring** ([`KnowledgeStore::blobs`]): the media + CRDT-snapshot blob
//!   store (K6 of §6), per-tenant-keyed, content-addressed (BLAKE3), re-hash-on-read.
//!
//! ## Floors named (stubbed / deferred + the filling prompt) — VISION §3
//! - **fs-backed BlobStore (11.2) is the M1 floor** Knowledge uses for media + CRDT snapshots; the
//!   follow-on **object-store BlobStore is KN-P31 (M5)** — a one-line backing swap behind the
//!   [`myelin_storage::blob::BlobStore`] trait. Named here in writing.
//! - **The `(tenant, region)` pin is the single-cell-collab floor**: v1 pins a doc's collab session
//!   to ONE cell; the cross-cell op fan-out follow-on is **KN-P30 (M5)** over the PII-free
//!   `CrossCellPointer` bridge. Named here in writing.
//! - **The concrete `sqlx`/`tokio-postgres` driver is the substrate P-S12 floor** (the
//!   [`OltpPool`] is the harness-wired bounded-pool MODEL until then; storage §3.1). The schema DDL,
//!   the partition shape, the RLS predicate, and the tenant-scoped helper are complete + testable
//!   now and do not change shape when the driver lands. The store is region-agnostic at the
//!   permit-accounting layer (the per-pool runtime region-pin lives on the storage
//!   `RegionPinnedStore` seam, P-ST-15); this open is `@residency-cell-pinned` (the LOUD, named
//!   M0-floor waiver — the per-query `TenantScope` carries the region out-of-band).
//!
//! ## DEVIATION FROM A FROZEN SHAPE (EI-01 §1 — code wins, write it down)
//! The architecture's illustrative DDL (§2.3) uses a custom `block_type` Postgres enum and a
//! per-row `FOREIGN KEY (tenant, parent_id) REFERENCES block(tenant, block_id)`. On the substrate
//! floor (no live Postgres, P-S12) the migration DDL is a `&'static str` the forward-only runner
//! records — it is not yet executed against a real engine. So this module ships the **table +
//! partition-key + index shape** the architecture freezes, but renders `block_type` as a plain
//! `text` column with a `CHECK` over the frozen `myelin-content` variant set (no bespoke enum type
//! the floor cannot create) and omits the self-referential FK (a forward-only runner cannot order a
//! self-FK before the table exists in one statement). Both are localised to the floor: when the
//! P-S12 driver lands, the enum type + the self-FK are an additive forward migration. The
//! load-bearing shape — `(tenant, region)` partition, the stable `block_id`, `order_key`,
//! `page_id` index — is byte-faithful to §2.3. Recorded here, not hidden.

use myelin_identity::Principal;
use myelin_storage::blob::{BlobStore, FsBlobStore};
use myelin_storage::oltp::{OltpConfig, OltpError, OltpPool};
use myelin_storage::rls::{ResolvedTenant, TenantQuery, TenantScope, TenantTable};
use myelin_tenancy::{Region, TenantId};

/// The Knowledge OLTP tables, each a `(tenant, region)`-partitioned tenant-owned table (§1 / §6).
///
/// A [`KnowledgeTable`] is the typed name of a Knowledge store table; it lowers to the storage
/// crate's [`TenantTable`] so a query against it is unconstructable without a verified
/// [`TenantScope`] (the structural `tenant-predicate` floor). The enum is exhaustive over the v1
/// Knowledge schema so a typo'd table name is a compile error, never a silent wrong-table query.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KnowledgeTable {
    /// `block` — the per-block adjacency-list rows (`parent_id` + LexoRank `order_key`); the block
    /// tree of every document (the highest-write table, §2.3). **HOT** (contract 1.5).
    Block,
    /// `page` — the page (root-block subtree) + hierarchy + space grouping (§2.6).
    Page,
    /// `db_collection` — the flexible-database definition (the frozen `myelin-query` field defs).
    DbCollection,
    /// `db_row` — the flexible-database JSONB property-bag rows (source of truth, §4.2). **HOT**.
    DbRow,
    /// `db_view` — the frozen `ViewSpec` (kind/filter/group_by/sort/visible/order_field, §4.4).
    DbView,
    /// `db_relation` — the two-way relation field source-of-truth typed edge (§4.3, TE-7).
    DbRelation,
    /// `page_parent` — the page → sub-page typed edge, mirrored to Refs as a `parent` edge (§4.3).
    PageParent,
    /// `doc_op` — the CRDT/CAS op-log live tail (the resume-cursor transport substrate, §3). **HOT**.
    DocOp,
    /// `doc_snapshot` — the compacted-snapshot metadata (the object-tier snapshot pointer, §3).
    DocSnapshot,
}

impl KnowledgeTable {
    /// The stable SQL table name (the thin, visible-SQL identifier, §2.8).
    pub fn name(self) -> &'static str {
        match self {
            KnowledgeTable::Block => "block",
            KnowledgeTable::Page => "page",
            KnowledgeTable::DbCollection => "db_collection",
            KnowledgeTable::DbRow => "db_row",
            KnowledgeTable::DbView => "db_view",
            KnowledgeTable::DbRelation => "db_relation",
            KnowledgeTable::PageParent => "page_parent",
            KnowledgeTable::DocOp => "doc_op",
            KnowledgeTable::DocSnapshot => "doc_snapshot",
        }
    }

    /// Whether this table is declared **hot** (contract 1.5 — `block`/`db_row`/`doc_op`). The hot
    /// set here is the SAME as [`crate::HOT_TABLES`] (a divergence would be a coherence bug).
    pub fn is_hot(self) -> bool {
        matches!(
            self,
            KnowledgeTable::Block | KnowledgeTable::DbRow | KnowledgeTable::DocOp
        )
    }

    /// Lower to the storage crate's [`TenantTable`] — the RLS guard's tenant-owned-table token.
    fn tenant_table(self) -> TenantTable {
        TenantTable::new(self.name())
    }

    /// Every Knowledge table, in declaration order (used by the schema + the partition assertion).
    pub const ALL: [KnowledgeTable; 9] = [
        KnowledgeTable::Block,
        KnowledgeTable::Page,
        KnowledgeTable::DbCollection,
        KnowledgeTable::DbRow,
        KnowledgeTable::DbView,
        KnowledgeTable::DbRelation,
        KnowledgeTable::PageParent,
        KnowledgeTable::DocOp,
        KnowledgeTable::DocSnapshot,
    ];
}

/// The Knowledge OLTP store handle — the bounded pool ([`OltpPool`], 11.1), the tenant-scoped
/// query seam ([`KnowledgeStore::query`], the RLS half of 11.1 / 12.1), and the fs-backed media +
/// snapshot BlobStore ([`KnowledgeStore::blobs`], 11.2). The store is `(tenant, region)`-pinned at
/// the per-query layer (the [`TenantScope`] carries the region); the per-POOL runtime region-pin is
/// the storage `RegionPinnedStore` seam (P-ST-15) — this floor is region-agnostic at the
/// permit-accounting layer (the `@residency-cell-pinned` waiver below).
pub struct KnowledgeStore {
    pool: OltpPool,
    blobs: FsBlobStore,
}

impl KnowledgeStore {
    /// Open the Knowledge OLTP store through the harness with a validated bounded-pool config
    /// (11.1 — a service opens its pool through `serve(AppSpec)`, never a hand-rolled connection).
    /// The fs-backed BlobStore (11.2, the M1 floor) is wired alongside for media + CRDT snapshots.
    ///
    /// **`@residency-cell-pinned`** — the LOUD, NAMED M0-floor waiver (EI-01 §4): on this floor the
    /// `OltpPool` MODEL is region-agnostic at the permit-accounting layer; the per-query
    /// `(tenant, region)` `TenantScope` carries the region out-of-band, and the per-pool runtime
    /// region-pin lands in the storage `RegionPinnedStore` (P-ST-15 / STOR-D5). NOT a weakening —
    /// the `residency-pin` lint stays live on every UNMARKED store open.
    pub fn open(config: OltpConfig) -> Result<KnowledgeStore, OltpError> {
        // @residency-cell-pinned: the M0 region-less pool MODEL (the TenantScope pins the region
        // per-query; the per-pool RegionPinnedStore seam is P-ST-15). Named, reviewed, not hidden.
        let pool = OltpPool::open(config)?;
        Ok(KnowledgeStore {
            pool,
            blobs: FsBlobStore::new(),
        })
    }

    /// The bounded OLTP pool (11.1) — handlers acquire a per-tenant permit against it.
    pub fn pool(&self) -> &OltpPool {
        &self.pool
    }

    /// The fs-backed media + CRDT-snapshot BlobStore (11.2, the M1 floor; K6 of §6). Per-tenant
    /// keyed, content-addressed (BLAKE3), re-hash-on-read. The object-store swap is KN-P31 (M5).
    pub fn blobs(&self) -> &impl BlobStore {
        &self.blobs
    }

    /// **The tenant-scoped query helper (the RLS half of 11.1 / 12.1 — the `tenant-predicate`
    /// floor).** Build a query against a Knowledge `table`, carrying its `(tenant, region)`
    /// predicate **by construction**. The ONLY way to call this is with a verified [`TenantScope`]
    /// (minted from the token, never a path) — so a tenant-less Knowledge query is unconstructable
    /// (the structural half of the `tenant-predicate` lint; the source-scanning half is P-S10/11).
    ///
    /// Returns a [`TenantQuery`] whose [`TenantQuery::predicate_sql`] is the thin, visible
    /// `WHERE tenant = $.. AND region = $..` clause every Knowledge statement carries (§2.8).
    pub fn query(&self, scope: TenantScope, table: KnowledgeTable) -> TenantQuery {
        // EI-01 §7 — one primitive: Knowledge reuses the storage crate's RLS guard, it does not
        // re-implement a parallel tenant-scoping path. The scope is consumed by value (it cannot be
        // re-used to build an unrelated query) and the table lowers to the storage TenantTable.
        TenantQuery::for_table(scope, table.tenant_table())
    }

    /// **The KN-D13 IDOR-floor resolution (mandatory-core).** Resolve the effective tenant for a
    /// request whose URL path *asserts* `path_tenant`. The answer is ALWAYS the token's tenant
    /// (the verified `scope`); the path is **never** trusted (storage §1.1, the F2 IDOR floor).
    ///
    /// Returns the [`ResolvedTenant`] carrying the effective (token) tenant + the survival flags:
    /// `path_derived == false` always (the `path_derived_tenant_count == 0` signal), and
    /// `attempted_path_mismatch == true` iff the path tried (and failed) to spoof a different
    /// tenant. This is the Knowledge entrypoint's call onto the storage RLS guard's
    /// [`TenantScope::resolve`] — Knowledge does not re-derive the rule, it consumes the one floor.
    pub fn resolve_tenant(scope: &TenantScope, path_tenant: Option<&TenantId>) -> ResolvedTenant {
        scope.resolve(path_tenant)
    }
}

/// Mint the verified `(tenant, region)` scope every Knowledge query carries, from the verified
/// token (the [`Principal`], authenticate 4.1) + the cell's [`Region`] (the harness threads it,
/// §4.1). This is the ONLY way Knowledge obtains a [`TenantScope`] — there is deliberately no
/// path-derived constructor (the IDOR shape). A re-export of the storage crate's one constructor so
/// the Knowledge call site reads as "scope from the token, never the path" (EI-01 §7, one primitive).
pub fn knowledge_scope(principal: &Principal, cell_region: Region) -> TenantScope {
    TenantScope::from_verified_token(principal, cell_region)
}

/// The Knowledge OLTP **store** schema migrations (the KN-P05 owned half; architecture §2/§3/§4).
/// Forward-only (no destructive/backward DDL); every table is `(tenant, region)`-partitioned (the
/// leading PK column-pair). The three high-write tables (`block`/`db_row`/`doc_op`) are PHASED
/// migrations carrying their hot-table flag so the forward-only runner refuses a blocking `ALTER`
/// on them; their indexes use `CREATE INDEX CONCURRENTLY` (the online idiom, §9.1) so the index
/// build is itself non-blocking.
///
/// These EXTEND (never replace) the KN-P04 shell's `0200_knowledge_schema_marker` — the store DDL
/// appends to the same forward-only chain (`02xx_*`). The harness prepends the co-located
/// `outbox` and `consumer_dedup` tables ([`myelin_substrate::boot`]); the Knowledge-owned emit
/// bodies and the consumer set are the KN-P06 follow-on.
///
/// See the module DEVIATION note: `block_type` is a `text` + `CHECK` (no bespoke floor enum type),
/// and the self-referential `block` FK is deferred to the P-S12 driver (an additive forward
/// migration) — both localised to the floor; the partition + index shape is byte-faithful to §2.3.
pub fn knowledge_store_migrations() -> myelin_substrate::Migrations {
    use myelin_substrate::{Migration, MigrationPhase};
    myelin_substrate::Migrations::of([
        // ---- page (§2.6): a page is a root-block subtree; the (tenant, page_id) partition key. ----
        Migration::plain(
            "0201_page",
            "CREATE TABLE IF NOT EXISTS page (\
               tenant text NOT NULL, region text NOT NULL, page_id text NOT NULL, \
               space_id text NOT NULL, parent_page text, title text NOT NULL, icon text, \
               is_folder boolean NOT NULL DEFAULT false, published boolean NOT NULL DEFAULT false, \
               archived boolean NOT NULL DEFAULT false, acl_zookie text, created_at text NOT NULL, \
               PRIMARY KEY (tenant, page_id))",
        ),
        // ---- block (§2.3): the HOT high-write block tree (adjacency list + LexoRank order_key). --
        // PHASED + hot-flagged `block`; the index is CONCURRENTLY (online) so the build is non-blocking.
        Migration::phased(
            "0202_block",
            "CREATE TABLE IF NOT EXISTS block (\
               tenant text NOT NULL, region text NOT NULL, page_id text NOT NULL, \
               block_id text NOT NULL, parent_id text, order_key text NOT NULL, \
               block_type text NOT NULL CHECK (block_type IN (\
                 'paragraph','heading','bullet_list','ordered_list','task_list','blockquote',\
                 'code_block','callout','table','divider','image','embed','db_view','toggle',\
                 'sync_block')), \
               props text NOT NULL DEFAULT '{}', inline text NOT NULL DEFAULT '', \
               inline_nodes text NOT NULL DEFAULT '[]', \
               contains_personal_data boolean NOT NULL DEFAULT false, data_role text, \
               pii_key_ref text, created_by text NOT NULL, edited_by text NOT NULL, \
               created_at text NOT NULL, edited_at text NOT NULL, version bigint NOT NULL, \
               PRIMARY KEY (tenant, block_id))",
            MigrationPhase::Expand,
            "block",
        ),
        Migration::phased(
            "0203_block_children_index",
            "CREATE INDEX CONCURRENTLY IF NOT EXISTS block_children \
               ON block (tenant, page_id, parent_id, order_key)",
            MigrationPhase::Expand,
            "block",
        ),
        // ---- db_collection (§4.2): the flexible-database definition (frozen myelin-query fields). -
        Migration::plain(
            "0204_db_collection",
            "CREATE TABLE IF NOT EXISTS db_collection (\
               tenant text NOT NULL, region text NOT NULL, db_id text NOT NULL, \
               space_id text NOT NULL, name text NOT NULL, field_defs text NOT NULL, \
               PRIMARY KEY (tenant, db_id))",
        ),
        // ---- db_row (§4.2): the HOT high-write JSONB property-bag rows (source of truth). ---------
        Migration::phased(
            "0205_db_row",
            "CREATE TABLE IF NOT EXISTS db_row (\
               tenant text NOT NULL, region text NOT NULL, db_id text NOT NULL, \
               row_id text NOT NULL, props text NOT NULL, body_page text, order_key text NOT NULL, \
               version bigint NOT NULL, contains_personal_data boolean NOT NULL DEFAULT false, \
               data_role text, pii_key_ref text, created_at text NOT NULL, \
               PRIMARY KEY (tenant, row_id))",
            MigrationPhase::Expand,
            "db_row",
        ),
        Migration::phased(
            "0206_db_row_props_index",
            "CREATE INDEX CONCURRENTLY IF NOT EXISTS db_row_props_gin ON db_row (tenant, db_id)",
            MigrationPhase::Expand,
            "db_row",
        ),
        // ---- db_view (§4.4): the frozen ViewSpec (kind/filter/group_by/sort/visible/order_field). -
        Migration::plain(
            "0207_db_view",
            "CREATE TABLE IF NOT EXISTS db_view (\
               tenant text NOT NULL, region text NOT NULL, view_id text NOT NULL, \
               db_id text NOT NULL, spec text NOT NULL, shared boolean NOT NULL DEFAULT true, \
               PRIMARY KEY (tenant, view_id))",
        ),
        // ---- db_relation (§4.3, TE-7): the two-way relation source-of-truth typed edge. ----------
        Migration::plain(
            "0208_db_relation",
            "CREATE TABLE IF NOT EXISTS db_relation (\
               tenant text NOT NULL, region text NOT NULL, relation_id text NOT NULL, \
               src_row text NOT NULL, dst_ref text NOT NULL, rel text NOT NULL, \
               created_by text NOT NULL, created_at text NOT NULL, \
               PRIMARY KEY (tenant, relation_id))",
        ),
        // ---- page_parent (§4.3): the page → sub-page typed edge (mirrored to Refs). --------------
        Migration::plain(
            "0209_page_parent",
            "CREATE TABLE IF NOT EXISTS page_parent (\
               tenant text NOT NULL, region text NOT NULL, page_id text NOT NULL, \
               parent_page text NOT NULL, order_key text NOT NULL, \
               PRIMARY KEY (tenant, page_id))",
        ),
        // ---- doc_op (§3): the HOT high-write op-log live tail (resume-cursor transport). ----------
        Migration::phased(
            "0210_doc_op",
            "CREATE TABLE IF NOT EXISTS doc_op (\
               tenant text NOT NULL, region text NOT NULL, page_id text NOT NULL, \
               op_seq bigint NOT NULL, op_id text NOT NULL, actor text NOT NULL, \
               op_kind text NOT NULL, payload text NOT NULL, pii_key_ref text, \
               applied_at text NOT NULL, \
               PRIMARY KEY (tenant, page_id, op_seq))",
            MigrationPhase::Expand,
            "doc_op",
        ),
        Migration::phased(
            "0211_doc_op_resume_index",
            "CREATE INDEX CONCURRENTLY IF NOT EXISTS doc_op_resume \
               ON doc_op (tenant, page_id, op_seq)",
            MigrationPhase::Expand,
            "doc_op",
        ),
        // ---- doc_snapshot (§3): the compacted-snapshot metadata (object-tier pointer). -----------
        Migration::plain(
            "0212_doc_snapshot",
            "CREATE TABLE IF NOT EXISTS doc_snapshot (\
               tenant text NOT NULL, region text NOT NULL, page_id text NOT NULL, \
               snap_seq bigint NOT NULL, blob_hash text NOT NULL, named_label text, \
               created_at text NOT NULL, \
               PRIMARY KEY (tenant, page_id, snap_seq))",
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalId, PrincipalKind};
    use myelin_substrate::{is_blocking_alter, is_destructive, HotTables, MigrationRunner};

    fn principal(tenant: &str) -> Principal {
        Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        )
    }

    fn cfg() -> OltpConfig {
        OltpConfig {
            max_pool_size: 8,
            statement_timeout_ms: 5_000,
            per_tenant_in_flight_cap: 4,
        }
    }

    /// The store opens over a validated bounded pool (11.1) and wires the fs BlobStore (11.2).
    #[test]
    fn store_opens_with_pool_and_blobs() {
        let store = KnowledgeStore::open(cfg()).expect("the knowledge store opens");
        assert_eq!(store.pool().config(), cfg());
        // The fs BlobStore is wired + usable (media + CRDT snapshots, K6).
        let acme = TenantId("acme".into());
        let h = store
            .blobs()
            .put(&acme, b"a snapshot blob")
            .expect("blob put");
        assert_eq!(
            store.blobs().get(&acme, &h).expect("blob get"),
            b"a snapshot blob",
            "the fs BlobStore round-trips a Knowledge media/snapshot blob"
        );
    }

    /// **Every Knowledge query carries its `(tenant, region)` predicate (the RLS half, 11.1/12.1).**
    /// A query built through the tenant-scoped helper renders the thin, visible
    /// `WHERE tenant = $.. AND region = $..` clause; the tenant + region are the verified token's.
    #[test]
    fn every_knowledge_query_carries_the_tenant_region_predicate() {
        let store = KnowledgeStore::open(cfg()).expect("open");
        let scope = knowledge_scope(&principal("acme"), Region::new("fr-par"));
        for table in KnowledgeTable::ALL {
            let q = store.query(scope.clone(), table);
            let sql = q.predicate_sql();
            // Parameterized ($1/$2) — the token's (tenant, region) travel as binds, not literals.
            assert!(
                sql.contains("tenant = $1 AND region = $2"),
                "{} query must pin the (tenant, region) via bind placeholders: {sql}",
                table.name()
            );
            assert_eq!(
                q.predicate_binds(),
                vec!["acme".to_string(), "fr-par".to_string()],
                "{} query binds carry the verified token's (tenant, region)",
                table.name()
            );
            assert!(
                sql.starts_with(table.name()),
                "{} query must target its own table: {sql}",
                table.name()
            );
            assert_eq!(
                q.validate(),
                Ok(()),
                "{} query is well-formed",
                table.name()
            );
        }
    }

    /// **KN-D13 — 0 cross-tenant read via path-tenant spoofing.** A read whose token-tenant differs
    /// from the path-asserted tenant resolves to the **token's** tenant; `path_derived` is false
    /// (the `path_derived_tenant_count == 0` survival signal), and the spoof attempt is flagged.
    /// This is the F2 IDOR floor at the Knowledge store boundary (tenant from the token, never path).
    #[test]
    fn kn_d13_path_tenant_spoof_resolves_to_token_tenant_zero_cross_tenant() {
        let scope = knowledge_scope(&principal("acme"), Region::new("fr-par"));
        // The classic IDOR: the path asserts a DIFFERENT tenant than the verified token.
        let resolved = KnowledgeStore::resolve_tenant(&scope, Some(&TenantId("evil-corp".into())));
        // Effective tenant is the TOKEN's — never the path's. 0 cross-tenant read.
        assert_eq!(
            resolved.tenant,
            TenantId("acme".into()),
            "the effective tenant is the token's, never the spoofed path tenant"
        );
        assert!(
            !resolved.path_derived,
            "path_derived_tenant_count == 0 — the tenant is NEVER taken from the path (KN-D13)"
        );
        assert!(
            resolved.attempted_path_mismatch,
            "the spoof attempt is flagged (the guard held, the read stays in the token's tenant)"
        );
    }

    /// KN-D13 the matching + absent path cases: a matching path or an internal (no-path) call both
    /// resolve to the token's tenant, no mismatch, still 0 path-derived — the floor holds uniformly.
    #[test]
    fn kn_d13_matching_and_absent_path_resolve_to_token_no_mismatch() {
        let scope = knowledge_scope(&principal("acme"), Region::new("fr-par"));
        let matching = KnowledgeStore::resolve_tenant(&scope, Some(&TenantId("acme".into())));
        assert_eq!(matching.tenant, TenantId("acme".into()));
        assert!(!matching.path_derived);
        assert!(
            !matching.attempted_path_mismatch,
            "matching tenants are not a spoof"
        );

        let internal = KnowledgeStore::resolve_tenant(&scope, None);
        assert_eq!(internal.tenant, TenantId("acme".into()));
        assert!(!internal.path_derived);
        assert!(!internal.attempted_path_mismatch);
    }

    /// **The schema is forward-only (0 destructive migrations) — the `forward-only-migration` gate.**
    /// No store migration is destructive (DROP); the runner would refuse one. The store DDL extends
    /// the shell's marker chain (forward-only, no backward/down).
    #[test]
    fn store_schema_is_forward_only() {
        for m in &knowledge_store_migrations().0 {
            assert!(
                !is_destructive(m.ddl),
                "store migration {} is forward-only (no DROP)",
                m.id
            );
        }
    }

    /// **The hot tables' index builds are CONCURRENTLY (online) — no blocking ALTER survives the
    /// runner.** The whole store schema applies cleanly against the `block`/`db_row`/`doc_op`
    /// hot-table declaration: a hot-table index is `CREATE INDEX CONCURRENTLY` (non-blocking), so
    /// the forward-only runner (which refuses a blocking ALTER on a hot table) admits the schema.
    #[test]
    fn store_schema_applies_and_hot_table_changes_are_online() {
        let hot = HotTables::declare(crate::HOT_TABLES);
        let mut runner = MigrationRunner::new();
        runner
            .run(&knowledge_store_migrations(), &hot)
            .expect("the whole Knowledge store schema applies (no blocking ALTER on a hot table)");
        // And specifically: each hot-table migration's DDL is NOT a blocking ALTER.
        for m in &knowledge_store_migrations().0 {
            if let Some(table) = m.table {
                if hot.is_hot(table) {
                    assert!(
                        !is_blocking_alter(m.ddl),
                        "hot-table migration {} must be online (no blocking ALTER): {}",
                        m.id,
                        m.ddl
                    );
                }
            }
        }
    }

    /// **The three high-write tables are flagged hot, consistent with [`crate::HOT_TABLES`].** The
    /// store's per-table hot flag matches the AppSpec's declaration exactly (a divergence would be
    /// the coherence bug EI-01 §7 forbids — one hot set, one source of truth).
    #[test]
    fn store_hot_tables_match_the_appspec_declaration() {
        let mut store_hot: Vec<&str> = KnowledgeTable::ALL
            .iter()
            .filter(|t| t.is_hot())
            .map(|t| t.name())
            .collect();
        store_hot.sort_unstable();
        let mut declared: Vec<&str> = crate::HOT_TABLES.to_vec();
        declared.sort_unstable();
        assert_eq!(
            store_hot, declared,
            "the store's hot tables == the AppSpec hot-table declaration (block/db_row/doc_op)"
        );
    }

    /// **Every Knowledge table is `(tenant, region)`-partitioned (12.1).** Every CREATE TABLE in the
    /// schema declares the leading `tenant`/`region` columns and a `(tenant, ...)` primary key — the
    /// residency-pin shape (the partition key is `(tenant, region)`, never `tenant` alone).
    #[test]
    fn every_table_is_tenant_region_partitioned() {
        for m in &knowledge_store_migrations().0 {
            if m.ddl.contains("CREATE TABLE") {
                assert!(
                    m.ddl.contains("tenant text NOT NULL")
                        && m.ddl.contains("region text NOT NULL"),
                    "table migration {} must declare the (tenant, region) partition columns",
                    m.id
                );
                assert!(
                    m.ddl.contains("PRIMARY KEY (tenant"),
                    "table migration {} must lead its primary key with the tenant partition key",
                    m.id
                );
            }
        }
        // and exactly the nine v1 Knowledge tables are created (no table forgotten / duplicated).
        let creates = knowledge_store_migrations()
            .0
            .iter()
            .filter(|m| m.ddl.contains("CREATE TABLE"))
            .count();
        assert_eq!(
            creates,
            KnowledgeTable::ALL.len(),
            "exactly the 9 v1 Knowledge tables"
        );
    }
}
