//! # MR-007 — durable identity stores, proven against LIVE Postgres.
//!
//! Gated behind the `integration` cargo feature so the DEFAULT `cargo test` stays DB-free. Runs ONLY
//! against the docker-compose dev stack (the make-it-real env):
//!
//!   DATABASE_URL=postgres://myelin_app:myelin_app_pw@localhost:5433/myelin \
//!     cargo test -p myelin-identity-service --features integration \
//!       --test integration_mr007_identity_durable -- --nocapture
//!
//! It proves the MR-007 deliverables — each MUST hit the live DB (a pass on the in-memory model would
//! not count, so every assertion goes through a `with_pg` store over the real pool):
//!   1. **Durability (store-layer restart proof):** principals + tuples written through ONE store
//!      instance over the real pool are read back by a FRESH store instance over the SAME live pool
//!      (simulating a restart — the real kill-9 is MR-009). Present + correct.
//!   2. **Tenant isolation:** through the `myelin_app` (NOBYPASSRLS) role, a principal/tuple written
//!      for tenant A is invisible to tenant B on a predicate-less read (RLS enforced via
//!      with_tenant_tx), and no GUC bleeds after the connection is released.
//!   3. **Outbox co-commit on the tuple write:** a committed durable write emits exactly one
//!      `identity.tuple.written` event (emit-iff-the-durable-write-succeeded).
//!
//! KMS/profile boundary (respected): the profile ciphertext persists in PG, but decrypt-across-
//! PROCESS-restart depends on the durable KMS root (MR-025) — so the fresh-instance reads here SHARE
//! one in-process `KmsEngine` (the durable-root stand-in). The full profile-decrypt-across-restart
//! proof is MR-009's job (after MR-025); this test scopes its durability claim to the principal ROW
//! + the tuple edge + tenant isolation, exactly per the prompt.
#![cfg(feature = "integration")]

use std::sync::Arc;

use myelin_config::MyelinConfig;
use myelin_events::{EventEnvelope, Timestamp};
use myelin_identity::iam_events::IDENTITY_TUPLE_WRITTEN;
use myelin_identity::{
    DataRole, ObjectId, Precondition, Principal, PrincipalId, PrincipalKind, PrincipalStatus,
    RelName, RelationTuple, TupleDelta, Zookie,
};
use myelin_identity_service::principal_store::{PrincipalProfile, PrincipalStore};
use myelin_identity_service::tuple_store::TupleStore;
use myelin_storage::migration::HotTables;
use myelin_storage::{
    identity_durable_migrations, DurablePrincipalBacking, DurableTupleBacking, KmsEngine,
    SubstrateProvider,
};
use myelin_tenancy::{Region, TenantId};

/// DDL runs as the migration/owner role (PG16 revokes `CREATE` on `public` for the app role).
fn admin_config(cfg: &MyelinConfig) -> MyelinConfig {
    let mut c = cfg.clone();
    c.database_url = c
        .database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw");
    c
}

/// A per-run unique suffix so a fresh run uses fresh `(tenant)` partitions (no cross-run collision).
fn uniq() -> String {
    format!(
        "{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn scope(tenant: &str, region: &str) -> myelin_storage::TenantScope {
    let p = Principal::stub(
        PrincipalId("p:admin".into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    myelin_storage::TenantScope::from_verified_token(&p, Region(region.into()))
}

fn actor() -> Principal {
    Principal::stub(
        PrincipalId("p:writer".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

/// A per-store-UNIQUE, lexically-monotonic id minter for the durable tuple stores. The default
/// `MonotonicMinter` resets to `0` per store, so every store's first co-committed `identity.tuple.written`
/// mints the SAME `event_id` — which the global `outbox` `UNIQUE(event_id)` collapses via
/// `ON CONFLICT DO NOTHING` when suites share the live DB (masking the co-commit). The production
/// wall-clock+random ULID source (P-S12) is globally unique; this test double reproduces that
/// property via a per-store `base` so the BUS-2-exact co-commit is observable in isolation.
struct UniqueMinter {
    base: String,
    n: std::sync::atomic::AtomicU64,
}

impl UniqueMinter {
    fn new(base: impl Into<String>) -> Self {
        UniqueMinter {
            base: base.into(),
            n: std::sync::atomic::AtomicU64::new(0),
        }
    }
}

impl myelin_events::IdMinter for UniqueMinter {
    fn mint(&self) -> myelin_events::Ulid {
        // `01J` prefix + the per-store base + a zero-padded counter: lexically-monotonic WITHIN the
        // store (per-aggregate ordering) and globally unique ACROSS stores (the base).
        let n = self.n.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        myelin_events::Ulid(format!("01J{}{n:012}", self.base))
    }
}

fn tuple(object: &str, relation: &str, subject: &str) -> RelationTuple {
    RelationTuple {
        object: ObjectId(object.into()),
        relation: RelName(relation.into()),
        subject: PrincipalId(subject.into()),
        caveat: None,
    }
}

fn profile(email_addr: &str, name: &str) -> PrincipalProfile {
    let email = email_addr.to_string();
    let display_name = name.to_string();
    PrincipalProfile {
        email,
        display_name,
    }
}

/// Build an app-role provider (NOBYPASSRLS, reset-on-release) for the stores; `None` (with a SKIP
/// note) if the DB is unreachable.
async fn app_provider() -> Option<SubstrateProvider> {
    match SubstrateProvider::connect(MyelinConfig::dev(), 6).await {
        Ok(p) => Some(p),
        Err(_) => {
            eprintln!("SKIP: dev Postgres unreachable (is the docker stack up?)");
            None
        }
    }
}

/// Read `myelin.tenant_id` on a freshly-acquired pooled connection (empty when unset).
async fn residual_guc(pool: &sqlx::PgPool) -> String {
    let mut conn = pool.acquire().await.expect("acquire");
    let v: Option<String> = sqlx::query_scalar("SELECT current_setting('myelin.tenant_id', true)")
        .fetch_one(&mut *conn)
        .await
        .expect("read GUC");
    v.unwrap_or_default()
}

// =================================================================================================
// 1 — Durability: a FRESH store instance over the SAME pool reads back what the first one wrote.
// =================================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn durable_principal_and_tuple_round_trip_across_a_fresh_store_instance() {
    // Migrate (admin role) — the principal/credential_link tables + the reused rebac_tuple + RLS.
    let admin = match SubstrateProvider::connect(admin_config(&MyelinConfig::dev()), 4).await {
        Ok(p) => p,
        Err(_) => {
            eprintln!("SKIP: dev Postgres unreachable (is the docker stack up?)");
            return;
        }
    };
    admin
        .migrate(&identity_durable_migrations(), &HotTables::none())
        .await
        .expect("identity durable migrations execute against the live DB");

    let Some(app) = app_provider().await else {
        return;
    };
    let region = app.config().region.clone();
    let handle = tokio::runtime::Handle::current();
    let suffix = uniq();
    let tenant = format!("mr007-dur-{suffix}");
    let s = scope(&tenant, &region);

    // A SHARED in-process KMS (the durable-root stand-in; cross-process decrypt is MR-025 / MR-009).
    let kms = Arc::new(KmsEngine::new());

    // ---- Write through store instance #1 ----
    let pstore1 = PrincipalStore::with_pg(
        kms.clone(),
        DurablePrincipalBacking::new(app.clone()),
        handle.clone(),
    );
    let tstore1 = TupleStore::with_pg_minter(
        Arc::new(UniqueMinter::new(format!("{suffix}w1"))),
        DurableTupleBacking::new(app.clone()),
        handle.clone(),
    );

    let alice = PrincipalId("p:alice".into());
    let written = pstore1
        .put_principal(
            &s,
            alice.clone(),
            PrincipalKind::Human,
            DataRole::Processor,
            PrincipalStatus::Active,
            Some(&profile("alice@acme.test", "Alice")),
        )
        .expect("durable principal write");
    assert!(written.profile_ref.is_some(), "a profiled principal has a profile_ref");
    // A machine principal (no profile) — proves the Agent/Service polymorphic kind round-trips.
    pstore1
        .put_principal(
            &s,
            PrincipalId("svc:deploy".into()),
            PrincipalKind::Service,
            DataRole::Controller,
            PrincipalStatus::Active,
            None,
        )
        .expect("durable service principal write");

    let z = tstore1
        .write_tuples(
            &s,
            &actor(),
            &[
                TupleDelta::Add(tuple("repo:core", "reader", "p:alice")),
                TupleDelta::Add(tuple("repo:core", "writer", "p:bob")),
            ],
            None,
            None,
            Timestamp("2026-06-26T00:00:00Z".into()),
        )
        .expect("durable tuple write");

    // ---- Read through a FRESH store instance #2 over the SAME live pool (restart simulation) ----
    let pstore2 = PrincipalStore::with_pg(
        kms.clone(),
        DurablePrincipalBacking::new(app.clone()),
        handle.clone(),
    );
    let tstore2 = TupleStore::with_pg(DurableTupleBacking::new(app.clone()), handle.clone());

    // Principal ROW durability (the MR-007 claim): kind/role/status/profile_ref present + correct.
    let read = pstore2
        .get_principal(&s, &alice)
        .expect("the principal row is durable across a fresh instance");
    assert_eq!(read.principal_id, alice);
    assert_eq!(read.kind, PrincipalKind::Human);
    assert_eq!(read.data_role, DataRole::Processor);
    assert_eq!(read.status, PrincipalStatus::Active);
    assert!(read.profile_ref.is_some(), "the erasable profile_ref persists (ciphertext durable)");
    assert_eq!(
        pstore2.principals_in(&s).len(),
        2,
        "both the human + service principals are durable"
    );
    // Profile decrypt via the SHARED in-process KMS (same-process; cross-restart is MR-009/MR-025).
    let prof = pstore2
        .get_profile(&s, &alice)
        .expect("profile read succeeds")
        .expect("the profile ciphertext is durable + decrypts under the shared KMS");
    assert_eq!(prof, profile("alice@acme.test", "Alice"), "the profile round-trips");

    // Tuple EDGE durability (the MR-007 claim): both edges read back from the fresh instance.
    let mut edges: Vec<(String, String, String)> = tstore2
        .tuples_in(&s)
        .into_iter()
        .map(|t| (t.tuple.object.0, t.tuple.relation.0, t.tuple.subject.0))
        .collect();
    edges.sort();
    assert_eq!(
        edges,
        vec![
            ("repo:core".into(), "reader".into(), "p:alice".into()),
            ("repo:core".into(), "writer".into(), "p:bob".into()),
        ],
        "both durable edges round-trip across a fresh store instance"
    );
    assert!(!z.0.is_empty(), "the write returned a monotonic zookie");

    // Cleanup (admin role — RLS-bypassing owner; a bare app-role DELETE outside a tenant tx matches
    // nothing under RLS).
    for sql in [
        "DELETE FROM rebac_tuple WHERE tenant_id = $1",
        "DELETE FROM principal WHERE tenant_id = $1",
        "DELETE FROM credential_link WHERE tenant_id = $1",
    ] {
        let _ = sqlx::query(sql).bind(&tenant).execute(admin.db_pool()).await;
    }
    // The co-committed identity.tuple.written rows for this tenant (BUS-2 exact now emits into the outbox).
    let _ = sqlx::query("DELETE FROM outbox WHERE aggregate LIKE $1")
        .bind(format!("identity:tuple:{tenant}:%"))
        .execute(admin.db_pool())
        .await;
    println!("OK [1]: principal row + profile ciphertext + tuple edges durable across a fresh instance.");
}

// =================================================================================================
// 2 — Tenant isolation through the NOBYPASSRLS app role + no GUC bleed.
// =================================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tenant_a_writes_are_invisible_to_tenant_b_and_no_guc_bleeds() {
    let admin = match SubstrateProvider::connect(admin_config(&MyelinConfig::dev()), 4).await {
        Ok(p) => p,
        Err(_) => {
            eprintln!("SKIP: dev Postgres unreachable (is the docker stack up?)");
            return;
        }
    };
    admin
        .migrate(&identity_durable_migrations(), &HotTables::none())
        .await
        .expect("identity durable migrations");

    let Some(app) = app_provider().await else {
        return;
    };
    let region = app.config().region.clone();
    let handle = tokio::runtime::Handle::current();
    let kms = Arc::new(KmsEngine::new());
    let suffix = uniq();
    let tenant_a = format!("mr007A-{suffix}");
    let tenant_b = format!("mr007B-{suffix}");
    let sa = scope(&tenant_a, &region);
    let sb = scope(&tenant_b, &region);

    let pstore =
        PrincipalStore::with_pg(kms.clone(), DurablePrincipalBacking::new(app.clone()), handle.clone());
    let tstore = TupleStore::with_pg_minter(
        Arc::new(UniqueMinter::new(format!("{suffix}w2"))),
        DurableTupleBacking::new(app.clone()),
        handle.clone(),
    );

    // Tenant A writes a principal + a tuple.
    let alice = PrincipalId("p:alice".into());
    pstore
        .put_principal(
            &sa,
            alice.clone(),
            PrincipalKind::Human,
            DataRole::Processor,
            PrincipalStatus::Active,
            Some(&profile("alice@a.test", "Alice")),
        )
        .expect("tenant A principal write");
    tstore
        .write_tuples(
            &sa,
            &actor(),
            &[TupleDelta::Add(tuple("repo:secret", "reader", "p:alice"))],
            None,
            None,
            Timestamp("2026-06-26T00:00:00Z".into()),
        )
        .expect("tenant A tuple write");

    // Tenant A sees its own rows (proving the read path + RLS admit the owner).
    assert_eq!(pstore.principals_in(&sa).len(), 1, "tenant A sees its principal");
    assert_eq!(tstore.tuples_in(&sa).len(), 1, "tenant A sees its tuple");

    // Tenant B — through the SAME app-role pool — sees NONE of tenant A's rows. The store reads are
    // predicate-less at the SQL level beyond the tenant binds; the ISOLATION is the DB FORCE-RLS
    // policy inside the with_tenant_tx transaction (a wrong-tenant session reads zero rows).
    assert!(
        pstore.get_principal(&sb, &alice).is_none(),
        "tenant B cannot see tenant A's principal (RLS via with_tenant_tx)"
    );
    assert!(pstore.principals_in(&sb).is_empty(), "tenant B's principal partition is empty");
    assert!(tstore.tuples_in(&sb).is_empty(), "tenant B cannot see tenant A's tuple (RLS)");

    // No GUC bleeds: after all the tenant-scoped ops above, a freshly-acquired connection from the
    // app pool carries NO residual tenant identity (SET LOCAL discarded at COMMIT + reset-on-release).
    assert!(
        residual_guc(app.db_pool()).await.is_empty(),
        "no residual myelin.tenant_id GUC after the tenant-scoped ops (no bleed)"
    );

    // A dangling credential link is refused durably (the principal must exist in THIS partition).
    let unknown = pstore.link_credential(&sa, "oidc", "sub-unknown", &PrincipalId("p:ghost".into()));
    assert!(
        matches!(
            unknown,
            Err(myelin_identity_service::principal_store::PrincipalError::UnknownPrincipal { .. })
        ),
        "a link to a non-existent principal is refused"
    );
    // A link to an existing principal resolves back (the credential index round-trips).
    pstore
        .link_credential(&sa, "oidc", "sub-alice", &alice)
        .expect("link a verified credential to an existing principal");
    let resolved = pstore
        .resolve_credential(&sa, "oidc", "sub-alice")
        .expect("the credential resolves to its principal");
    assert_eq!(resolved.principal_id, alice);
    // …but NOT cross-tenant (tenant B cannot resolve tenant A's credential).
    assert!(
        pstore.resolve_credential(&sb, "oidc", "sub-alice").is_none(),
        "a credential verified for tenant A never resolves into tenant B's directory"
    );

    for sql in [
        "DELETE FROM rebac_tuple WHERE tenant_id = $1 OR tenant_id = $2",
        "DELETE FROM principal WHERE tenant_id = $1 OR tenant_id = $2",
        "DELETE FROM credential_link WHERE tenant_id = $1 OR tenant_id = $2",
        "DELETE FROM outbox WHERE aggregate LIKE 'identity:tuple:' || $1 || ':%' \
         OR aggregate LIKE 'identity:tuple:' || $2 || ':%'",
    ] {
        let _ = sqlx::query(sql)
            .bind(&tenant_a)
            .bind(&tenant_b)
            .execute(admin.db_pool())
            .await;
    }
    println!("OK [2]: tenant A invisible to tenant B (RLS via with_tenant_tx); no GUC bleed; credential isolation.");
}

// =================================================================================================
// 3 — Outbox co-commit on the durable tuple write, into the SAME-DB outbox table (BUS-2 exact —
//     MR-009b W3b.3). A committed write lands EXACTLY one identity.tuple.written row in the co-located
//     `outbox` table (not a separate in-memory store); an aborted write (failed precondition)
//     lands NONE — commit/abort together (0 ghost / 0 lost).
// =================================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn durable_tuple_write_co_commits_exactly_one_outbox_event() {
    let admin = match SubstrateProvider::connect(admin_config(&MyelinConfig::dev()), 4).await {
        Ok(p) => p,
        Err(_) => {
            eprintln!("SKIP: dev Postgres unreachable (is the docker stack up?)");
            return;
        }
    };
    admin
        .migrate(&identity_durable_migrations(), &HotTables::none())
        .await
        .expect("identity durable migrations");
    // The co-located `outbox` table (the frozen 2.3 shape) the tuple write co-commits into.
    admin
        .migrate_foundation()
        .await
        .expect("foundation (outbox) migration");
    let Some(app) = app_provider().await else {
        return;
    };
    let region = app.config().region.clone();
    let handle = tokio::runtime::Handle::current();
    let suffix = uniq();
    let tenant = format!("mr007-ob-{suffix}");
    let s = scope(&tenant, &region);

    // The per-object aggregate key the identity.tuple.written draft stamps: `identity:tuple:<tenant>:<object>`.
    let aggregate = format!("identity:tuple:{tenant}:repo:core");

    // Count the co-located outbox rows for THIS write's aggregate (RLS-free infra table; read it
    // straight via the admin pool — the outbox carries no tenant column, contract 2.3).
    async fn outbox_count(pool: &sqlx::PgPool, aggregate: &str) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM outbox WHERE aggregate = $1")
            .bind(aggregate)
            .fetch_one(pool)
            .await
            .expect("count outbox rows")
    }

    let tstore = TupleStore::with_pg_minter(
        Arc::new(UniqueMinter::new(suffix.clone())),
        DurableTupleBacking::new(app.clone()),
        handle,
    );

    // Pre-clean any stale row for this aggregate left by an aborted prior run (idempotent).
    let _ = sqlx::query("DELETE FROM outbox WHERE aggregate = $1")
        .bind(&aggregate)
        .execute(admin.db_pool())
        .await;
    assert_eq!(
        outbox_count(admin.db_pool(), &aggregate).await,
        0,
        "no outbox row for this aggregate before the write"
    );

    // (a) A COMMITTED write → EXACTLY one identity.tuple.written row in the SAME-DB outbox table.
    let z = tstore
        .write_tuples(
            &s,
            &actor(),
            &[TupleDelta::Add(tuple("repo:core", "reader", "p:alice"))],
            None,
            None,
            Timestamp("2026-06-26T00:00:00Z".into()),
        )
        .expect("durable tuple write");
    assert_eq!(
        outbox_count(admin.db_pool(), &aggregate).await,
        1,
        "a committed durable write co-committed EXACTLY one identity.tuple.written row (BUS-2 exact)"
    );

    // The row's SHAPE is preserved (consumers cannot tell the durable path apart): same type, the
    // write's zookie in the payload, no inline PII.
    let env_json: serde_json::Value =
        sqlx::query_scalar("SELECT envelope FROM outbox WHERE aggregate = $1")
            .bind(&aggregate)
            .fetch_one(admin.db_pool())
            .await
            .expect("read the co-committed outbox envelope");
    let env: EventEnvelope =
        serde_json::from_value(env_json).expect("the outbox row is a canonical EventEnvelope");
    assert_eq!(env.type_.0, IDENTITY_TUPLE_WRITTEN, "the co-committed event is identity.tuple.written");
    assert_eq!(env.payload["zookie"], serde_json::json!(z.0), "it carries the write's zookie");
    assert!(!env.contains_personal_data, "the identity.* event carries no inline PII");
    assert_eq!(
        env.actor.0.principal_id.0, "p:writer",
        "attribution by opaque principal_id only"
    );

    // (b) An ABORTED write (stale precondition) co-commits NOTHING — the outbox count is UNCHANGED
    // (0 ghost: no event without its committed tuple write).
    let stale = Zookie("zk-00000000000000000000".into());
    let err = tstore
        .write_tuples(
            &s,
            &actor(),
            &[TupleDelta::Add(tuple("repo:core", "writer", "p:bob"))],
            Some(&Precondition {
                expected_zookie: Some(stale),
            }),
            None,
            Timestamp("2026-06-26T00:00:01Z".into()),
        )
        .expect_err("a stale precondition aborts the write");
    assert!(
        matches!(err, myelin_identity_service::tuple_store::WriteError::PreconditionFailed { .. }),
        "the aborted write is a precondition failure"
    );
    assert_eq!(
        outbox_count(admin.db_pool(), &aggregate).await,
        1,
        "the aborted write co-committed NO outbox row (0 ghost — commit/abort together)"
    );

    for sql in [
        "DELETE FROM rebac_tuple WHERE tenant_id = $1",
        "DELETE FROM outbox WHERE aggregate = $1",
    ] {
        let bind = if sql.contains("outbox") { &aggregate } else { &tenant };
        let _ = sqlx::query(sql).bind(bind).execute(admin.db_pool()).await;
    }
    println!(
        "OK [3]: a committed durable tuple write co-commits EXACTLY one identity.tuple.written row into \
         the SAME-DB outbox; an aborted write co-commits none (0 ghost)."
    );
}
