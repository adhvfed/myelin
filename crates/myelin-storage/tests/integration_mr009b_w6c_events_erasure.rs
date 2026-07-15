//! # MR-009b W6c-events — the durable Bus erasure ledger (contract 10.8), proven against LIVE Postgres.
//!
//! These are the silent-resurrection GATES for the events `BusErasureLedger` (SI-039), proven against
//! the live docker-compose Postgres (:5433), NOT modeled in memory. `myelin-events` is a §2.9 DAG SINK
//! (it cannot name a `PgPool`), so the flip uses the **DedupLedger trait-seam pattern**: the
//! `DurableBusErasure` trait is defined IN events, the PG impl (`DurableBusErasureBacking`) + the
//! NON-shred-erasable, NO-RLS `bus_erasure_ledger` table (migration 0053) live here in storage, wired at
//! the `EventsRuntime` composition root via `BusErasureLedger::durable`.
//!
//! The four proofs (the W6c-events gate):
//!   1. **Records SURVIVE reconstruction from a FRESH pool** — an erasure recorded through one durable
//!      ledger is STILL remembered by a brand-new ledger over a FRESH pool (the "process restart"): an
//!      in-memory `BTreeMap` would be empty → the re-erasure pass would replay nothing → a restored
//!      pre-erase backup could silently resurrect the subject.
//!   2. **Idempotent `key_refs` MERGE (deduped)** — recording the same subject twice with OVERLAPPING
//!      ref sets yields ONE row whose `key_refs` are the UNION, de-duplicated + sorted, keeping the
//!      FIRST `erased_at` (the `ON CONFLICT … DO UPDATE` array-merge).
//!   3. **Partition ISOLATION** — tenant A's erasure ledger is INVISIBLE to tenant B's `(tenant,
//!      region)` scope (the explicit predicate on every statement; no RLS on this table by design).
//!   4. **`re_erase_after_restore` drives off the DURABLE ledger** — after a restore resurrects a
//!      pre-erase backup, a BusHolder replays a FRESH durable ledger (only the PG rows, no in-memory
//!      state) and re-destroys the resurrected key → 0 resurrected (the BUS-D8 threshold).
//!
//! Run against the dev stack:
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-storage --features integration \
//!     --test integration_mr009b_w6c_events_erasure -- --nocapture
#![cfg(feature = "integration")]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use myelin_config::MyelinConfig;
use myelin_storage::events_durable::{DurableBusErasureBacking, BUS_ERASURE_LEDGER_MIGRATION};
use myelin_storage::tenant_tx::connect_pool_with_reset;

use myelin_events::{
    derive_envelope, Actor, AggregateKey, ArtifactRef, BusErasureLedger, BusEventLog, BusHolder,
    CausedBy, DataRole, DurableBusErasure, EmitContext, EraseReceipt, EventDraft, EventEnvelope,
    EventId, EventType, IdMinter, InMemoryShredder, InlinePiiShredder, MonotonicMinter, OutboxStore,
    PiiKeyRef, Region, TenantId, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};

// ----------------------------------------------------------------------------------------------
// shared helpers
// ----------------------------------------------------------------------------------------------

fn admin_url(cfg: &MyelinConfig) -> String {
    cfg.database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

fn uniq() -> String {
    static N: AtomicU64 = AtomicU64::new(0);
    format!("{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst))
}

fn region() -> Region {
    Region("fr-par".into())
}
fn now() -> Timestamp {
    Timestamp("2026-07-15T00:00:00Z".into())
}
fn later() -> Timestamp {
    Timestamp("2026-07-15T12:00:00Z".into())
}

fn keyref(subject: &str) -> PiiKeyRef {
    PiiKeyRef(format!("kms://acme/0/subject:{subject}"))
}

async fn ensure_table(pool: &sqlx::PgPool) {
    // `CREATE TABLE IF NOT EXISTS` races across the four concurrent `#[tokio::test]`s on the FIRST run
    // (Postgres raises a duplicate-`pg_type` / "tuple concurrently updated" error for the losers of the
    // DDL race). Tolerate the error, then CONFIRM the table exists — the winner created it. (Production
    // boot applies this via the advisory-locked `SubstrateProvider::migrate`, which serializes DDL; the
    // test applies it directly so it is self-contained.)
    for _ in 0..8 {
        let _ = sqlx::raw_sql(BUS_ERASURE_LEDGER_MIGRATION).execute(pool).await;
        let exists: bool =
            sqlx::query_scalar("SELECT to_regclass('public.bus_erasure_ledger') IS NOT NULL")
                .fetch_one(pool)
                .await
                .unwrap_or(false);
        if exists {
            return;
        }
    }
    panic!("bus_erasure_ledger table could not be ensured (DDL race did not settle)");
}

async fn cleanup(pool: &sqlx::PgPool, tenant: &str) {
    sqlx::query("DELETE FROM bus_erasure_ledger WHERE tenant = $1")
        .bind(tenant)
        .execute(pool)
        .await
        .ok();
}

fn durable_ledger(tenant: &str, pool: &sqlx::PgPool, rt: &tokio::runtime::Handle) -> BusErasureLedger {
    BusErasureLedger::durable(
        TenantId(tenant.into()),
        region(),
        Arc::new(DurableBusErasureBacking::new(pool.clone(), rt.clone())) as Arc<dyn DurableBusErasure>,
    )
}

// --- BusHolder harness (mirrors the events-crate reerase unit tests) ---

fn actor_for(id: &str, tenant: &str) -> Actor {
    Actor(Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    ))
}

fn inline_pii(event_id: &str, subject: &str, tenant: &str) -> EventEnvelope {
    let draft = EventDraft {
        type_: EventType("chat.message.created".into()),
        subject: ArtifactRef(format!("myelin://acme/chat/message/{event_id}")),
        aggregate: AggregateKey(format!("chat.message:{event_id}")),
        payload: serde_json::json!({ "ref": format!("myelin://acme/chat/message/{event_id}") }),
        data_role: DataRole::Processor,
        visibility: Visibility::Internal,
        contains_personal_data: true,
        pii_key_ref: Some(keyref(subject)),
    };
    let ctx = EmitContext {
        event_id: EventId(event_id.into()),
        tenant: TenantId(tenant.into()),
        region: region(),
        actor: actor_for(subject, tenant),
        schema_ver: 1,
        occurred_at: now(),
        recorded_at: now(),
        caused_by: Some(CausedBy("human:h".into())),
    };
    derive_envelope(draft, ctx, None)
}

/// Seal a log + shredder for `subjects`, one inline-PII event each (every DEK live — pre-erase state).
fn seeded(subjects: &[&str], tenant: &str) -> (BusEventLog, InMemoryShredder) {
    let mut log = BusEventLog::new();
    let shredder = InMemoryShredder::new();
    for (i, s) in subjects.iter().enumerate() {
        let ev = inline_pii(&format!("01J-{tenant}-{i}"), s, tenant);
        if let Some(k) = &ev.pii_key_ref {
            shredder.seal(k);
        }
        log.append(ev);
    }
    (log, shredder)
}

fn minter() -> Arc<dyn IdMinter> {
    Arc::new(MonotonicMinter::new())
}

// ==============================================================================================
// TEST 1 — records SURVIVE reconstruction from a FRESH pool (the durability / "restart" proof)
// ==============================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn w6c_records_survive_reconstruction_from_a_fresh_pool() {
    let cfg = MyelinConfig::dev();
    let rt = tokio::runtime::Handle::current();
    let tag = uniq();
    let tenant = format!("acme-w6c1-{tag}");

    let pool_write = connect_pool_with_reset(&admin_url(&cfg), &cfg.region, 4)
        .await
        .expect("connect Postgres (is the stack up?)");
    ensure_table(&pool_write).await;
    cleanup(&pool_write, &tenant).await;

    let subject = format!("subject:{tag}");
    // ADVERSARIAL input (W6c verifier finding, probe-proven pre-fix): UNSORTED + DUPLICATED refs
    // on the FIRST insert — the no-conflict INSERT arm used to store the bound array VERBATIM
    // (only the DO UPDATE arm normalized), diverging from the memory arm and double-counting a
    // duplicated ref in the re-erasure receipt. The refs must come back sorted + deduped.
    let refs = vec![keyref("b"), keyref("a"), keyref("a")];

    // Record through one durable ledger, then DROP its pool entirely.
    {
        let ledger = durable_ledger(&tenant, &pool_write, &rt);
        ledger.record(&subject, &refs, now());
        assert!(ledger.is_erased(&subject), "recorded in the writing ledger");
        pool_write.close().await;
    }

    // A brand-NEW pool + a brand-NEW ledger (the process restart): the record SURVIVED in PG.
    let pool_fresh = connect_pool_with_reset(&admin_url(&cfg), &cfg.region, 4)
        .await
        .expect("connect a FRESH pool");
    let ledger2 = durable_ledger(&tenant, &pool_fresh, &rt);
    assert!(
        ledger2.is_erased(&subject),
        "after a FRESH pool the erasure record SURVIVED (an in-memory BTreeMap would be empty)"
    );
    let entries = ledger2.entries();
    assert_eq!(entries.len(), 1, "exactly one recorded subject");
    assert_eq!(entries[0].subject, subject);
    assert_eq!(
        entries[0].key_refs,
        vec![keyref("a"), keyref("b")],
        "the shredded key refs survived NORMALIZED (unsorted+duplicated input came back sorted, \
         deduped — first-insert-path parity with the memory arm, the W6c verifier finding)"
    );
    assert_eq!(entries[0].erased_at.0, now().0, "the erased_at timestamp survived");

    println!(
        "[MR-009b/W6c-events] PASS  test=RECORDS-SURVIVE-FRESH-POOL  tenant={tenant} \
         subjects=1 refs=2  backend=real-PG bus_erasure_ledger"
    );

    cleanup(&pool_fresh, &tenant).await;
}

// ==============================================================================================
// TEST 2 — idempotent `key_refs` MERGE (union, deduped, first erased_at kept)
// ==============================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn w6c_idempotent_key_refs_merge_dedups_and_keeps_first_erased_at() {
    let cfg = MyelinConfig::dev();
    let rt = tokio::runtime::Handle::current();
    let tag = uniq();
    let tenant = format!("acme-w6c2-{tag}");

    let pool = connect_pool_with_reset(&admin_url(&cfg), &cfg.region, 4)
        .await
        .expect("connect Postgres");
    ensure_table(&pool).await;
    cleanup(&pool, &tenant).await;

    let subject = format!("subject:{tag}");
    let ledger = durable_ledger(&tenant, &pool, &rt);

    // First record: {a, b} at `now`.
    ledger.record(&subject, &[keyref("a"), keyref("b")], now());
    // Second record of the SAME subject with an OVERLAPPING set {b, c} at a LATER time.
    ledger.record(&subject, &[keyref("b"), keyref("c")], later());

    let entries = ledger.entries();
    assert_eq!(entries.len(), 1, "idempotent: still ONE row for the subject");
    let e = &entries[0];
    assert_eq!(
        e.key_refs,
        vec![keyref("a"), keyref("b"), keyref("c")],
        "key_refs MERGED to the UNION, de-duplicated (`b` once) and sorted"
    );
    assert_eq!(
        e.erased_at.0,
        now().0,
        "the FIRST erased_at is KEPT (a later erase merges refs without moving the recorded time)"
    );

    println!(
        "[MR-009b/W6c-events] PASS  test=IDEMPOTENT-KEY-REFS-MERGE  tenant={tenant} \
         merged={{a,b,c}} deduped=b first_erased_at=kept  backend=real-PG"
    );

    cleanup(&pool, &tenant).await;
}

// ==============================================================================================
// TEST 3 — partition ISOLATION (tenant A's ledger invisible to tenant B's scope)
// ==============================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn w6c_partition_isolation_tenant_a_invisible_to_tenant_b() {
    let cfg = MyelinConfig::dev();
    let rt = tokio::runtime::Handle::current();
    let tag = uniq();
    let tenant_a = format!("acme-w6c3a-{tag}");
    let tenant_b = format!("acme-w6c3b-{tag}");

    let pool = connect_pool_with_reset(&admin_url(&cfg), &cfg.region, 4)
        .await
        .expect("connect Postgres");
    ensure_table(&pool).await;
    cleanup(&pool, &tenant_a).await;
    cleanup(&pool, &tenant_b).await;

    let subject = format!("subject:{tag}");
    let ledger_a = durable_ledger(&tenant_a, &pool, &rt);
    let ledger_b = durable_ledger(&tenant_b, &pool, &rt);

    // Tenant A records an erasure; tenant B (SAME pool, SAME subject id, SAME region) must NOT see it.
    ledger_a.record(&subject, &[keyref("a")], now());

    assert!(ledger_a.is_erased(&subject), "tenant A sees its own erasure");
    assert!(
        !ledger_b.is_erased(&subject),
        "tenant B's scope does NOT see tenant A's erasure (partition isolation)"
    );
    assert_eq!(ledger_a.entries().len(), 1, "A's replay set has the subject");
    assert!(
        ledger_b.entries().is_empty(),
        "B's replay set is EMPTY (the explicit (tenant, region) predicate isolates it)"
    );

    println!(
        "[MR-009b/W6c-events] PASS  test=PARTITION-ISOLATION  tenant_a={tenant_a} \
         tenant_b={tenant_b} a_sees=1 b_sees=0  backend=real-PG"
    );

    cleanup(&pool, &tenant_a).await;
    cleanup(&pool, &tenant_b).await;
}

// ==============================================================================================
// TEST 4 — `re_erase_after_restore` drives off the DURABLE ledger (BUS-D8: 0 resurrected)
// ==============================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn w6c_re_erase_after_restore_drives_off_the_durable_ledger() {
    let cfg = MyelinConfig::dev();
    let rt = tokio::runtime::Handle::current();
    let tag = uniq();
    let tenant = format!("acme-w6c4-{tag}");

    let pool = connect_pool_with_reset(&admin_url(&cfg), &cfg.region, 4)
        .await
        .expect("connect Postgres");
    ensure_table(&pool).await;
    cleanup(&pool, &tenant).await;

    let subject = "u42";

    // (1) Erase u42 in the live cell AND record it in the DURABLE ledger.
    let (mut live_log, shredder) = seeded(&[subject], &tenant);
    let holder = BusHolder::new(TenantId(tenant.clone()), region(), shredder.clone());
    let write_ledger = durable_ledger(&tenant, &pool, &rt);
    let mut outbox = OutboxStore::new();
    let _receipt: EraseReceipt = holder
        .erase_and_record(subject, &mut live_log, &mut outbox, minter(), &write_ledger, now())
        .expect("erase+record");
    let key = keyref(subject);
    assert!(!shredder.is_live(&key), "key dead in the live cell");

    // (2) RESTORE an OLDER (pre-erase) backup: the DEK is LIVE again (re-sealed) and the log row is
    //     back WITHOUT its tombstone — exactly what restoring a backup taken before the erase does.
    let (mut restored_log, _) = seeded(&[subject], &tenant);
    shredder.seal(&key);
    assert!(shredder.is_live(&key), "the restore RESURRECTED u42's DEK");

    // (3) "Restart": a FRESH durable ledger over a FRESH pool — it has NO in-memory state, only the PG
    //     rows. `re_erase_after_restore` MUST drive off this durable ledger to know u42 was erased.
    let pool_fresh = connect_pool_with_reset(&admin_url(&cfg), &cfg.region, 4)
        .await
        .expect("connect a FRESH pool");
    let restart_ledger = durable_ledger(&tenant, &pool_fresh, &rt);
    assert!(
        restart_ledger.is_erased(subject),
        "the fresh durable ledger remembers the erasure (drives the replay)"
    );

    let mut reerase_outbox = OutboxStore::new();
    let receipt = holder
        .re_erase_after_restore(&restart_ledger, &mut restored_log, &mut reerase_outbox, minter(), now())
        .expect("re-erase after restore");

    assert!(
        !shredder.is_live(&key),
        "the key stays destroyed across the restore (re-erasure re-shredded it)"
    );
    assert_eq!(receipt.re_erased_subjects, 1, "one ledger subject replayed (from PG)");
    assert_eq!(
        receipt.keys_resurrected_by_restore, 1,
        "the restore brought the key back (the honest signal)"
    );
    assert_eq!(receipt.resurrected, 0, "THE GATE: 0 resurrected keys post-restore");
    assert!(receipt.is_green(), "the Bus's BUS-D8 restore-verify leg is GREEN off the durable ledger");

    println!(
        "[MR-009b/W6c-events] PASS  test=RE-ERASE-AFTER-RESTORE-OFF-DURABLE-LEDGER  tenant={tenant} \
         replayed=1 resurrected_by_restore=1 resurrected_after=0  backend=real-PG bus_erasure_ledger"
    );

    cleanup(&pool_fresh, &tenant).await;
}
