#![cfg(feature = "integration")]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use myelin_config::MyelinConfig;
use myelin_storage::events_durable::{DurableBusErasureBacking, BUS_ERASURE_LEDGER_MIGRATION};
use myelin_storage::tenant_tx::connect_pool_with_reset;

use myelin_events::{
    derive_envelope, Actor, AggregateKey, ArtifactRef, BusErasureError, BusErasureLedger,
    BusEventLog, BusHolder, CausedBy, DataRole, DurableBusErasure, EmitContext, EraseReceipt,
    ErasureLedgerError, EventDraft, EventEnvelope, EventId, EventType, IdMinter, InMemoryShredder,
    InlinePiiShredder, MonotonicMinter, OutboxStore, PiiKeyRef, Region, TenantId, Timestamp,
    Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};

fn admin_url(cfg: &MyelinConfig) -> String {
    cfg.database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

fn uniq() -> String {
    static N: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    )
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
    for _ in 0..8 {
        let _ = sqlx::raw_sql(BUS_ERASURE_LEDGER_MIGRATION)
            .execute(pool)
            .await;
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

fn durable_ledger(
    tenant: &str,
    pool: &sqlx::PgPool,
    rt: &tokio::runtime::Handle,
) -> BusErasureLedger {
    BusErasureLedger::durable(
        TenantId(tenant.into()),
        region(),
        Arc::new(DurableBusErasureBacking::new(pool.clone(), rt.clone()))
            as Arc<dyn DurableBusErasure>,
    )
}

fn available<T>(result: Result<T, ErasureLedgerError>) -> T {
    result.expect("bus erasure ledger is available")
}

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
    let refs = vec![keyref("b"), keyref("a"), keyref("a")];

    {
        let ledger = durable_ledger(&tenant, &pool_write, &rt);
        available(ledger.record(&subject, &refs, now()));
        assert!(
            available(ledger.is_erased(&subject)),
            "recorded in the writing ledger"
        );
        pool_write.close().await;
    }

    let pool_fresh = connect_pool_with_reset(&admin_url(&cfg), &cfg.region, 4)
        .await
        .expect("connect a FRESH pool");
    let ledger2 = durable_ledger(&tenant, &pool_fresh, &rt);
    assert!(
        available(ledger2.is_erased(&subject)),
        "after a FRESH pool the erasure record SURVIVED (an in-memory BTreeMap would be empty)"
    );
    let entries = available(ledger2.entries());
    assert_eq!(entries.len(), 1, "exactly one recorded subject");
    assert_eq!(entries[0].subject, subject);
    assert_eq!(
        entries[0].key_refs,
        vec![keyref("a"), keyref("b")],
        "the shredded key refs survived NORMALIZED (unsorted+duplicated input came back sorted, \
         deduped - first-insert-path parity with the memory arm, the W6c verifier finding)"
    );
    assert_eq!(
        entries[0].erased_at.0,
        now().0,
        "the erased_at timestamp survived"
    );

    println!(
        "[MR-009b/W6c-events] PASS  test=RECORDS-SURVIVE-FRESH-POOL  tenant={tenant} \
         subjects=1 refs=2  backend=real-PG bus_erasure_ledger"
    );

    cleanup(&pool_fresh, &tenant).await;
}

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

    available(ledger.record(&subject, &[keyref("a"), keyref("b")], now()));
    available(ledger.record(&subject, &[keyref("b"), keyref("c")], later()));

    let entries = available(ledger.entries());
    assert_eq!(
        entries.len(),
        1,
        "idempotent: still ONE row for the subject"
    );
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

    available(ledger_a.record(&subject, &[keyref("a")], now()));

    assert!(
        available(ledger_a.is_erased(&subject)),
        "tenant A sees its own erasure"
    );
    assert!(
        !available(ledger_b.is_erased(&subject)),
        "tenant B's scope does NOT see tenant A's erasure (partition isolation)"
    );
    assert_eq!(
        available(ledger_a.entries()).len(),
        1,
        "A's replay set has the subject"
    );
    assert!(
        available(ledger_b.entries()).is_empty(),
        "B's replay set is EMPTY (the explicit (tenant, region) predicate isolates it)"
    );

    println!(
        "[MR-009b/W6c-events] PASS  test=PARTITION-ISOLATION  tenant_a={tenant_a} \
         tenant_b={tenant_b} a_sees=1 b_sees=0  backend=real-PG"
    );

    cleanup(&pool, &tenant_a).await;
    cleanup(&pool, &tenant_b).await;
}

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

    let (mut live_log, shredder) = seeded(&[subject], &tenant);
    let holder = BusHolder::new(TenantId(tenant.clone()), region(), shredder.clone());
    let write_ledger = durable_ledger(&tenant, &pool, &rt);
    let mut outbox = OutboxStore::new();
    let _receipt: EraseReceipt = holder
        .erase_and_record(
            subject,
            &mut live_log,
            &mut outbox,
            minter(),
            &write_ledger,
            now(),
        )
        .expect("erase+record");
    let key = keyref(subject);
    assert!(!shredder.is_live(&key), "key dead in the live cell");

    let (mut restored_log, _) = seeded(&[subject], &tenant);
    shredder.seal(&key);
    assert!(shredder.is_live(&key), "the restore RESURRECTED u42's DEK");

    let pool_fresh = connect_pool_with_reset(&admin_url(&cfg), &cfg.region, 4)
        .await
        .expect("connect a FRESH pool");
    let restart_ledger = durable_ledger(&tenant, &pool_fresh, &rt);
    assert!(
        available(restart_ledger.is_erased(subject)),
        "the fresh durable ledger remembers the erasure (drives the replay)"
    );

    let mut reerase_outbox = OutboxStore::new();
    let receipt = holder
        .re_erase_after_restore(
            &restart_ledger,
            &mut restored_log,
            &mut reerase_outbox,
            minter(),
            now(),
        )
        .expect("re-erase after restore");

    assert!(
        !shredder.is_live(&key),
        "the key stays destroyed across the restore (re-erasure re-shredded it)"
    );
    assert_eq!(
        receipt.re_erased_subjects, 1,
        "one ledger subject replayed (from PG)"
    );
    assert_eq!(
        receipt.keys_resurrected_by_restore, 1,
        "the restore brought the key back (the honest signal)"
    );
    assert_eq!(
        receipt.resurrected, 0,
        "THE GATE: 0 resurrected keys post-restore"
    );
    assert!(
        receipt.is_green(),
        "the Bus's BUS-D8 restore-verify leg is GREEN off the durable ledger"
    );

    println!(
        "[MR-009b/W6c-events] PASS  test=RE-ERASE-AFTER-RESTORE-OFF-DURABLE-LEDGER  tenant={tenant} \
         replayed=1 resurrected_by_restore=1 resurrected_after=0  backend=real-PG bus_erasure_ledger"
    );

    cleanup(&pool_fresh, &tenant).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ledger_outage_withholds_both_erase_receipts_and_each_retry_finishes() {
    let cfg = MyelinConfig::dev();
    let rt = tokio::runtime::Handle::current();
    let tag = uniq();
    let tenant = format!("acme-w6c-outage-{tag}");
    let subject = "u42";
    let key = keyref(subject);

    let unavailable_pool = connect_pool_with_reset(&admin_url(&cfg), &cfg.region, 2)
        .await
        .expect("connect Postgres");
    ensure_table(&unavailable_pool).await;
    cleanup(&unavailable_pool, &tenant).await;
    let unavailable_ledger = durable_ledger(&tenant, &unavailable_pool, &rt);
    unavailable_pool.close().await;

    assert_eq!(
        unavailable_ledger.record(subject, std::slice::from_ref(&key), now()),
        Err(ErasureLedgerError::Unavailable),
        "a failed durable write is not reported as an erasure record"
    );
    assert_eq!(
        unavailable_ledger.is_erased(subject),
        Err(ErasureLedgerError::Unavailable),
        "a failed point read is not reported as an unerased subject"
    );
    assert_eq!(
        unavailable_ledger.entries(),
        Err(ErasureLedgerError::Unavailable),
        "a failed replay-set read is not reported as an empty ledger"
    );
    assert_eq!(
        unavailable_ledger.len(),
        Err(ErasureLedgerError::Unavailable)
    );
    assert_eq!(
        unavailable_ledger.is_empty(),
        Err(ErasureLedgerError::Unavailable)
    );

    let (mut live_log, shredder) = seeded(&[subject], &tenant);
    let holder = BusHolder::new(TenantId(tenant.clone()), region(), shredder.clone());
    let mut interrupted_outbox = OutboxStore::new();
    let interrupted = holder
        .erase_and_record(
            subject,
            &mut live_log,
            &mut interrupted_outbox,
            minter(),
            &unavailable_ledger,
            now(),
        )
        .expect_err("the caller cannot receive an erase receipt without a durable ledger record");
    assert_eq!(
        interrupted,
        BusErasureError::Ledger(ErasureLedgerError::Unavailable)
    );
    assert!(
        !shredder.is_live(&key),
        "the irreversible key destruction may finish before the ledger outage is observed"
    );

    let recovered_pool = connect_pool_with_reset(&admin_url(&cfg), &cfg.region, 2)
        .await
        .expect("reconnect Postgres");
    let recovered_ledger = durable_ledger(&tenant, &recovered_pool, &rt);
    let mut retry_outbox = OutboxStore::new();
    holder
        .erase_and_record(
            subject,
            &mut live_log,
            &mut retry_outbox,
            minter(),
            &recovered_ledger,
            later(),
        )
        .expect("retry completes the missing durable erasure record");
    assert!(
        available(recovered_ledger.is_erased(subject)),
        "the retry leaves the restore obligation durable"
    );

    let (mut restored_log, _) = seeded(&[subject], &tenant);
    shredder.seal(&key);
    recovered_pool.close().await;
    let mut interrupted_reerase_outbox = OutboxStore::new();
    let interrupted_reerase = holder
        .re_erase_after_restore(
            &recovered_ledger,
            &mut restored_log,
            &mut interrupted_reerase_outbox,
            minter(),
            later(),
        )
        .expect_err("an unreadable replay set cannot produce a green restore receipt");
    assert_eq!(
        interrupted_reerase,
        BusErasureError::Ledger(ErasureLedgerError::Unavailable)
    );
    assert!(
        shredder.is_live(&key),
        "the resurrected key remains visible until a retry can discover its obligation"
    );

    let retry_pool = connect_pool_with_reset(&admin_url(&cfg), &cfg.region, 2)
        .await
        .expect("reconnect Postgres for the restore retry");
    let retry_ledger = durable_ledger(&tenant, &retry_pool, &rt);
    let mut retry_reerase_outbox = OutboxStore::new();
    let receipt = holder
        .re_erase_after_restore(
            &retry_ledger,
            &mut restored_log,
            &mut retry_reerase_outbox,
            minter(),
            later(),
        )
        .expect("restore retry reads the obligation and re-destroys the key");
    assert!(
        receipt.is_green(),
        "the completed retry is truthfully green"
    );
    assert_eq!(receipt.re_erased_subjects, 1);
    assert_eq!(receipt.keys_resurrected_by_restore, 1);
    assert_eq!(receipt.resurrected, 0);
    assert!(
        !shredder.is_live(&key),
        "the recovered sweep leaves no resurrected key"
    );

    cleanup(&retry_pool, &tenant).await;
}
