//! **CDC 11.1 + 12.1 — the Knowledge OLTP store + the `(tenant, region)` partition + RLS pair**
//! (KN-P05 → global P-295, M3). Contract-index rows **11.1** (the OLTP tier client + the
//! `(tenant, region)`-first RLS tenant-scoping guard) and **12.1** (the `(tenant, region)` partition
//! key from the verified token). Plus the **KN-D13 drill-harness scenario** (a cross-tenant
//! path-tenant spoof → rejected, 0 cross-tenant read).
//!
//! Architecture: `storage.md` §1.1 (tenant is the first column / partition key; tenant from the
//! verified token, never the path; no cross-tenant query path), §3.1 (the bounded OLTP pool + the
//! RLS guard = the IDOR floor), and the Knowledge architecture `01 §1`/`§6` (the `(tenant, region)`
//! + page_id shard key on every K1/K2/K3 store table) + the testing-strategy KN-D13 drill row.
//!
//! ## What this CDC pair proves (the cross-crate contract, both sides)
//! - **PROVIDER (11.1):** `myelin-storage` ships the OLTP bounded pool ([`OltpPool`]) + the
//!   `(tenant, region)`-first RLS guard ([`TenantScope`]/[`TenantQuery`] — the tenant-predicate
//!   floor). The CONSUMER (the Knowledge store, [`KnowledgeStore`]) opens its OWN pool through that
//!   seam (the `no-cross-db` boundary) and builds every query through the guard — it does NOT
//!   re-implement a parallel store/RLS path.
//! - **PROVIDER (12.1):** `myelin-tenancy` exports the `(tenant, region)` partition-key value types
//!   ([`Region`]) and the storage `TenantScope` carries the region. The CONSUMER (the Knowledge
//!   store) mints its scope ONLY from a verified token (the IDOR-safe constructor) so every
//!   Knowledge query is `(tenant, region)`-pinned — a tenant-less query does not compile.
//!
//! ## The KN-D13 dated GREEN artifact (2026-06-22)
//! `drill_kn_d13_cross_tenant_path_spoof_is_rejected_zero_cross_tenant` is the dated green: a read
//! whose URL path asserts a DIFFERENT tenant than the verified token resolves to the **token's**
//! tenant — 0 cross-tenant read, `path_derived_tenant_count == 0`. The tenant-predicate lint is RED
//! on a deliberately tenant-less query fixture and GREEN on the Knowledge store query path. No
//! threshold weakened.

use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_knowledge::{knowledge_scope, KnowledgeStore, KnowledgeTable};
use myelin_lints::lints::tenant_predicate;
use myelin_storage::{OltpConfig, TenantScope};
use myelin_tenancy::{Region, TenantId};

fn principal(tenant: &str) -> Principal {
    Principal::stub(
        PrincipalId("p".into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    )
}

fn cfg() -> OltpConfig {
    OltpConfig {
        max_pool_size: 16,
        statement_timeout_ms: 3_000,
        per_tenant_in_flight_cap: 4,
    }
}

/// **CONSUMER side of 11.1 — the Knowledge store opens its OWN OLTP pool through the storage seam.**
/// The CONSUMER (Knowledge) opens a bounded pool through the PROVIDER's `OltpPool::open` seam — the
/// `no-cross-db` boundary holds (Knowledge owns its store; it reuses the SHARED guard mechanism, not
/// another subsystem's tables). The bounded-pool semantics (fast-fail on saturation) carry through.
#[test]
fn consumer_knowledge_store_opens_its_own_bounded_oltp_pool() {
    let store = KnowledgeStore::open(cfg()).expect("the Knowledge store opens its OLTP pool");
    assert_eq!(store.pool().config(), cfg());
    // The bounded pool is real: a per-tenant permit accounts against it (11.1 / storage §3.1).
    let acme = TenantId("acme".into());
    let permit = store.pool().acquire(&acme).expect("acquire a permit");
    assert_eq!(store.pool().in_flight(), 1);
    drop(permit);
    assert_eq!(
        store.pool().in_flight(),
        0,
        "dropping the permit releases it"
    );
}

/// **CONSUMER side of 11.1/12.1 — every Knowledge query carries its `(tenant, region)` predicate.**
/// The CONSUMER builds a query through the tenant-scoped helper; the PROVIDER's guard renders the
/// thin, visible `WHERE tenant = $.. AND region = $..` clause. The tenant + region are the verified
/// token's — never a path-derived value (12.1, the partition key from the token).
#[test]
fn consumer_every_knowledge_query_is_tenant_region_scoped() {
    let store = KnowledgeStore::open(cfg()).expect("open");
    let scope = knowledge_scope(&principal("acme"), Region::new("fr-par"));
    // Exercise the highest-write table (block) + a structured one (db_row) + the op-log (doc_op).
    for table in [
        KnowledgeTable::Block,
        KnowledgeTable::DbRow,
        KnowledgeTable::DocOp,
    ] {
        let q = store.query(scope.clone(), table);
        let sql = q.predicate_sql();
        assert!(sql.contains("tenant = 'acme'"), "{}: {sql}", table.name());
        assert!(sql.contains("region = 'fr-par'"), "{}: {sql}", table.name());
        assert_eq!(q.validate(), Ok(()));
    }
}

/// **PROVIDER side of 12.1 — the scope is mintable ONLY from the verified token.** The CONSUMER
/// obtains its `(tenant, region)` scope from the verified `Principal` + the cell region (the
/// harness threads it); there is no path-derived constructor (the IDOR shape). The scope's tenant
/// IS the token's, and the region IS the cell's — the partition key from the token, never the path.
#[test]
fn provider_scope_is_minted_from_the_verified_token_only() {
    let scope: TenantScope = knowledge_scope(&principal("acme"), Region::new("fr-par"));
    assert_eq!(scope.tenant(), &TenantId("acme".into()));
    assert_eq!(scope.region(), &Region::new("fr-par"));
}

/// **THE KN-D13 DRILL-HARNESS SCENARIO — a cross-tenant path-tenant spoof attempt → REJECTED.**
/// (testing-strategy/01 KN-D13: "Read a page/db/row across tenants via path-tenant spoofing → 0
/// cross-tenant read".) An attacker authenticates as tenant `acme` but crafts a request whose URL
/// path asserts they are reading tenant `evil-corp`'s page. The Knowledge store resolves the
/// effective tenant from the TOKEN — so the read stays in `acme`, sees 0 of `evil-corp`'s rows, and
/// the `path_derived_tenant_count` is 0 (the dated green survival signal). The spoof is FLAGGED
/// (the guard held). This is the F2 IDOR floor at the Knowledge boundary.
#[test]
fn drill_kn_d13_cross_tenant_path_spoof_is_rejected_zero_cross_tenant() {
    // The verified token is tenant `acme` in region `fr-par`.
    let scope = knowledge_scope(&principal("acme"), Region::new("fr-par"));

    // The attacker's request path asserts a DIFFERENT tenant (the classic IDOR / BOLA attempt):
    // `GET /t/evil-corp/pages/secret` while holding an `acme` token.
    let spoofed_path_tenant = TenantId("evil-corp".into());
    let resolved = KnowledgeStore::resolve_tenant(&scope, Some(&spoofed_path_tenant));

    // (1) The effective tenant is the TOKEN's — the read executes against `acme`, never `evil-corp`.
    assert_eq!(
        resolved.tenant,
        TenantId("acme".into()),
        "KN-D13: the effective tenant is the verified token's, never the spoofed path tenant"
    );
    // (2) 0 cross-tenant read: the tenant was NEVER taken from the path.
    assert!(
        !resolved.path_derived,
        "KN-D13: path_derived_tenant_count == 0 (the cross-tenant survival signal)"
    );
    // (3) The spoof attempt is observable (the guard held; EI-01 §3 — a rejected attack is loud).
    assert!(
        resolved.attempted_path_mismatch,
        "KN-D13: the cross-tenant spoof attempt is flagged (the read stayed in the token tenant)"
    );

    // (4) And the query the resolved tenant builds is pinned to `acme` — so even the SQL it would
    // run carries `WHERE tenant = 'acme'`, never `evil-corp` (the predicate is the token's).
    let store = KnowledgeStore::open(cfg()).expect("open");
    let q = store.query(scope, KnowledgeTable::Page);
    assert!(
        q.predicate_sql().contains("tenant = 'acme'") && !q.predicate_sql().contains("evil-corp"),
        "KN-D13: the resolved page query is pinned to the token tenant, never the spoofed path"
    );
}

/// **The `tenant-predicate` lint is RED on a tenant-less query and GREEN on a tenant-bound one
/// (KN-D13's compile-time half).** The drill's second clause is "the `tenant-predicate` lint catches
/// a tenant-less query at compile". This asserts the lint is NOT vacuously green: a query-builder
/// site with no `TenantId`/RLS-guard binder is flagged RED; the SAME shape with the tenant bound
/// (the storage `RlsGuard`/`TenantId`) is admitted GREEN. The Knowledge store's own query path is
/// structurally tenant-bound (it goes through `TenantQuery::for_table`, which requires a verified
/// `TenantScope`), so it can never be the RED shape — the lint's red fixture proves the gate bites.
#[test]
fn tenant_predicate_lint_is_red_on_tenantless_query_green_on_bound() {
    let lint = tenant_predicate();

    // RED: a Knowledge-shaped page read built straight off a query builder with NO tenant binder —
    // exactly the cross-tenant IDOR shape KN-D13 forbids. The lint MUST flag it.
    let red = r#"
        fn read_page_TENANTLESS(pool: &Pool, page_id: &str) -> Page {
            sqlx::query("SELECT * FROM page WHERE page_id = $1")
                .bind(page_id)
                .fetch_one(pool)
        }
    "#;
    assert!(
        !lint.run(red).is_empty(),
        "the tenant-predicate lint must flag a tenant-less Knowledge query (KN-D13 compile half)"
    );

    // GREEN: the SAME read with the tenant predicate threaded (a TenantId arg / the RLS guard) — the
    // shape the Knowledge store's TenantQuery enforces by construction. The lint MUST admit it.
    let green = r#"
        fn read_page_scoped(pool: &Pool, scope: &TenantScope, page_id: &str) -> Page {
            sqlx::query("SELECT * FROM page WHERE tenant = $1 AND page_id = $2")
                .bind(scope.tenant())
                .bind(page_id)
                .fetch_one(pool)
        }
    "#;
    assert!(
        lint.run(green).is_empty(),
        "the tenant-predicate lint must admit a tenant-bound Knowledge query"
    );
}
