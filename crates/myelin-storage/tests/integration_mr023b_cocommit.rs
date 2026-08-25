#![cfg(feature = "integration")]

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use myelin_config::MyelinConfig;
use myelin_storage::events_durable::DurableDedupBacking;
use myelin_storage::tenant_tx::connect_pool_with_reset;

use myelin_events::consumer::{Consumer, Delivered, Message, Subscription};
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, Backoff, ConsumerName, CorrelationId, DataRole, DedupError,
    DedupLedger, DurableDedup, EventEnvelope, EventHandler, EventId, EventType, HandleOutcome,
    HandlerTx, PrefetchBound, SubjectPattern, Timestamp, Visibility, CONSUMER_DEDUP_MIGRATION,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{Region, TenantId};

fn admin_url(cfg: &MyelinConfig) -> String {
    cfg.database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

fn uniq() -> String {
    static N: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    )
}

fn principal() -> Principal {
    Principal::stub(
        PrincipalId("p".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

fn envelope(id: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(id.into()),
        type_: EventType("issues.issue.created".into()),
        schema_ver: 1,
        tenant: TenantId("acme".into()),
        region: Region("eu-west".into()),
        actor: Actor(principal()),
        subject: ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()),
        aggregate: AggregateKey("issue:PROJ-1".into()),
        causation_id: None,
        correlation_id: CorrelationId(id.into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-07-16T00:00:00Z".into()),
        recorded_at: Timestamp("2026-07-16T00:00:01Z".into()),
        payload: serde_json::json!({ "ref": "x" }),
    }
}

fn subscription(name: &str) -> Subscription {
    Subscription::bind(
        ConsumerName(name.into()),
        &["myelin://acme/issues/"],
        PrefetchBound::DEFAULT,
    )
    .expect("the test subscription is valid")
}

static SUBJECTS: &[SubjectPattern] = &[];

struct EffectHandler {
    table: String,
    rt: tokio::runtime::Handle,
    outcome: HandleOutcome,
    ran: AtomicU32,
}

impl EventHandler for EffectHandler {
    fn subjects(&self) -> &'static [SubjectPattern] {
        SUBJECTS
    }
    fn handle(&self, ev: &EventEnvelope, tx: &mut HandlerTx<'_>) -> HandleOutcome {
        self.ran.fetch_add(1, Ordering::SeqCst);
        let Some(conn) = tx.connection::<sqlx::PgConnection>() else {
            return HandleOutcome::Retry(Backoff { seconds: 1 });
        };
        let sql = format!(
            "INSERT INTO {} (event_id) VALUES ($1) ON CONFLICT DO NOTHING",
            self.table
        );
        let wrote = tokio::task::block_in_place(|| {
            self.rt.block_on(async {
                sqlx::query(&sql)
                    .bind(&ev.event_id.0)
                    .execute(&mut *conn)
                    .await
            })
        });
        if wrote.is_err() {
            return HandleOutcome::Retry(Backoff { seconds: 1 });
        }
        self.outcome.clone()
    }
}

async fn setup(pool: &sqlx::PgPool, effect_table: &str) {
    sqlx::raw_sql(CONSUMER_DEDUP_MIGRATION)
        .execute(pool)
        .await
        .expect("apply CONSUMER_DEDUP_MIGRATION");
    sqlx::raw_sql(&format!(
        "CREATE TABLE IF NOT EXISTS {effect_table} (event_id text PRIMARY KEY)"
    ))
    .execute(pool)
    .await
    .expect("create effect table");
}

async fn effect_present(pool: &sqlx::PgPool, table: &str, id: &str) -> bool {
    let exists: bool = sqlx::query_scalar(&format!(
        "SELECT EXISTS(SELECT 1 FROM {table} WHERE event_id = $1)"
    ))
    .bind(id)
    .fetch_one(pool)
    .await
    .expect("effect read");
    exists
}

async fn dedup_present(pool: &sqlx::PgPool, consumer: &str, id: &str) -> bool {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM consumer_dedup WHERE consumer = $1 AND event_id = $2)",
    )
    .bind(consumer)
    .bind(id)
    .fetch_one(pool)
    .await
    .expect("dedup read");
    exists
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mr023b_cocommit_primitive_crash_leaves_neither_commit_lands_both() {
    let cfg = MyelinConfig::dev();
    let pool = connect_pool_with_reset(&admin_url(&cfg), &cfg.region, 6)
        .await
        .expect("connect Postgres (is the dev stack up?)");
    let tag = uniq();
    let effect = format!("mr023b_effect_{tag}");
    setup(&pool, &effect).await;

    let backing = DurableDedupBacking::new(pool.clone(), tokio::runtime::Handle::current());
    let ledger = DedupLedger::durable(Arc::new(backing) as Arc<dyn DurableDedup>);
    let consumer = ConsumerName(format!("mr023b_consumer_{tag}"));
    let id = format!("mr023b-evt-{tag}");
    let tenant = TenantId("acme".into());
    let region = Region(cfg.region.clone());

    tokio::task::block_in_place(|| {
        let (mut cotx, fresh) = ledger
            .begin_co_commit(&consumer, &EventId(id.clone()), &tenant, &region)
            .expect("dedup storage is available");
        assert!(fresh, "first delivery marks the dedup row FRESH");
        let conn = cotx
            .connection()
            .expect("durable co-commit exposes a connection")
            .downcast_mut::<sqlx::PgConnection>()
            .expect("the erased connection is a PgConnection");
        tokio::runtime::Handle::current().block_on(async {
            sqlx::query(&format!("INSERT INTO {effect} (event_id) VALUES ($1)"))
                .bind(&id)
                .execute(&mut *conn)
                .await
                .expect("write effect on the co-commit tx");
        });
        cotx.rollback();
    });

    assert!(
        !dedup_present(&pool, &consumer.0, &id).await,
        "crash-before-commit: the dedup mark did NOT persist (rolled back with the effect)"
    );
    assert!(
        !effect_present(&pool, &effect, &id).await,
        "crash-before-commit: the effect did NOT persist (rolled back with the mark)"
    );

    tokio::task::block_in_place(|| {
        let (mut cotx, fresh) = ledger
            .begin_co_commit(&consumer, &EventId(id.clone()), &tenant, &region)
            .expect("dedup storage is available");
        assert!(
            fresh,
            "after the rollback the redelivery is STILL FRESH (0 lost)"
        );
        let conn = cotx
            .connection()
            .unwrap()
            .downcast_mut::<sqlx::PgConnection>()
            .unwrap();
        tokio::runtime::Handle::current().block_on(async {
            sqlx::query(&format!("INSERT INTO {effect} (event_id) VALUES ($1)"))
                .bind(&id)
                .execute(&mut *conn)
                .await
                .expect("write effect on the redelivery tx");
        });
        cotx.commit().expect("commit the co-commit tx");
    });

    assert!(
        dedup_present(&pool, &consumer.0, &id).await,
        "after commit: the dedup mark IS durable"
    );
    assert!(
        effect_present(&pool, &effect, &id).await,
        "after commit: the effect IS durable (exactly-once-with-effect)"
    );

    tokio::task::block_in_place(|| {
        let (cotx, fresh) = ledger
            .begin_co_commit(&consumer, &EventId(id.clone()), &tenant, &region)
            .expect("dedup storage is available");
        assert!(
            !fresh,
            "the committed mark makes a redelivery a DUPLICATE (deduped)"
        );
        cotx.rollback();
    });
    let effect_count: i64 = sqlx::query_scalar(&format!(
        "SELECT count(*) FROM {effect} WHERE event_id = $1"
    ))
    .bind(&id)
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(
        effect_count, 1,
        "exactly ONE effect row - no duplicate on redelivery"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mr023b_consumer_runtime_cocommit_retry_rolls_back_then_reruns() {
    let cfg = MyelinConfig::dev();
    let pool = connect_pool_with_reset(&admin_url(&cfg), &cfg.region, 6)
        .await
        .expect("connect Postgres");
    let tag = uniq();
    let effect = format!("mr023b_rt_effect_{tag}");
    setup(&pool, &effect).await;

    let rt = tokio::runtime::Handle::current();
    let make_ledger = || {
        let backing = DurableDedupBacking::new(pool.clone(), rt.clone());
        DedupLedger::durable(Arc::new(backing) as Arc<dyn DurableDedup>)
    };
    let cname = format!("mr023b_rt_{tag}");
    let id = format!("mr023b-rt-evt-{tag}");
    let msg = Message {
        subject: "myelin://acme/issues/issue/PROJ-1".into(),
        envelope: envelope(&id),
    };

    let retry_consumer = Consumer::new(
        EffectHandler {
            table: effect.clone(),
            rt: rt.clone(),
            outcome: HandleOutcome::Retry(Backoff { seconds: 1 }),
            ran: AtomicU32::new(0),
        },
        subscription(&cname),
        make_ledger(),
    );
    let out = tokio::task::block_in_place(|| retry_consumer.deliver(&msg));
    assert_eq!(out, Delivered::Retried(1), "a Retry is not acked");
    assert_eq!(
        retry_consumer.handler().ran.load(Ordering::SeqCst),
        1,
        "the handler ran"
    );
    assert!(
        !dedup_present(&pool, &cname, &id).await,
        "Retry rolled back the co-commit tx → the dedup mark is GONE"
    );
    assert!(
        !effect_present(&pool, &effect, &id).await,
        "Retry rolled back the co-commit tx → the effect is GONE (both vanish together)"
    );

    let done_consumer = Consumer::new(
        EffectHandler {
            table: effect.clone(),
            rt: rt.clone(),
            outcome: HandleOutcome::Done,
            ran: AtomicU32::new(0),
        },
        subscription(&cname),
        make_ledger(),
    );
    let out = tokio::task::block_in_place(|| done_consumer.deliver(&msg));
    assert_eq!(out, Delivered::Acked, "the redelivery ran + committed");
    assert!(
        dedup_present(&pool, &cname, &id).await,
        "Done committed the dedup mark"
    );
    assert!(
        effect_present(&pool, &effect, &id).await,
        "Done committed the effect (exactly-once-WITH-EFFECT - not deduped-and-lost)"
    );

    let out = tokio::task::block_in_place(|| done_consumer.deliver(&msg));
    assert_eq!(
        out,
        Delivered::Deduplicated,
        "the committed mark dedups the redelivery"
    );
    assert_eq!(
        done_consumer.handler().ran.load(Ordering::SeqCst),
        1,
        "the Done handler ran exactly once across the redelivery (dedup-skip)"
    );
    let effect_count: i64 = sqlx::query_scalar(&format!(
        "SELECT count(*) FROM {effect} WHERE event_id = $1"
    ))
    .bind(&id)
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(effect_count, 1, "exactly ONE effect row - no duplicate");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn database_outage_retries_before_the_handler_and_redelivery_commits_once() {
    let cfg = MyelinConfig::dev();
    let unavailable_pool = connect_pool_with_reset(&admin_url(&cfg), &cfg.region, 2)
        .await
        .expect("connect Postgres");
    let tag = uniq();
    let effect = format!("mr023b_outage_effect_{tag}");
    setup(&unavailable_pool, &effect).await;

    let rt = tokio::runtime::Handle::current();
    let backing = DurableDedupBacking::new(unavailable_pool.clone(), rt.clone());
    let consumer_name = ConsumerName(format!("mr023b_outage_{tag}"));
    let event_id = EventId(format!("mr023b-outage-evt-{tag}"));
    let tenant = TenantId("acme".into());
    let region = Region(cfg.region.clone());
    let message = Message {
        subject: "myelin://acme/issues/issue/PROJ-1".into(),
        envelope: envelope(&event_id.0),
    };

    unavailable_pool.close().await;

    tokio::task::block_in_place(|| {
        assert_eq!(
            backing.mark_handled(&consumer_name, &event_id),
            Err(DedupError::Unavailable),
            "an unavailable mark is not reported as fresh"
        );
        assert_eq!(
            backing.is_handled(&consumer_name, &event_id),
            Err(DedupError::Unavailable),
            "an unavailable read is not reported as unhandled"
        );
        assert_eq!(
            backing.revert(&consumer_name, &event_id),
            Err(DedupError::Unavailable),
            "an unavailable revert is not reported as complete"
        );
        assert_eq!(
            backing.forget(&consumer_name, &event_id),
            Err(DedupError::Unavailable),
            "an unavailable forget is not reported as absent"
        );
        assert!(
            matches!(
                backing.begin_co_commit(&consumer_name, &event_id, &tenant, &region),
                Err(DedupError::Unavailable)
            ),
            "an unavailable co-commit never creates a pretend transaction"
        );
    });

    let waiting_consumer = Consumer::new(
        EffectHandler {
            table: effect.clone(),
            rt: rt.clone(),
            outcome: HandleOutcome::Done,
            ran: AtomicU32::new(0),
        },
        subscription(&consumer_name.0),
        DedupLedger::durable(Arc::new(backing)),
    );

    let first_delivery = tokio::task::block_in_place(|| waiting_consumer.deliver(&message));
    assert_eq!(
        first_delivery,
        Delivered::Retried(2),
        "the delivery waits for durable dedup storage instead of running unsafely"
    );
    assert_eq!(
        waiting_consumer.handler().ran.load(Ordering::SeqCst),
        0,
        "the handler cannot create an effect before dedup acquisition succeeds"
    );
    assert_eq!(waiting_consumer.lag(), 1, "the delivery remains pending");

    let recovered_pool = connect_pool_with_reset(&admin_url(&cfg), &cfg.region, 2)
        .await
        .expect("reconnect Postgres");
    let recovered_consumer = Consumer::new(
        EffectHandler {
            table: effect.clone(),
            rt: rt.clone(),
            outcome: HandleOutcome::Done,
            ran: AtomicU32::new(0),
        },
        subscription(&consumer_name.0),
        DedupLedger::durable(Arc::new(DurableDedupBacking::new(
            recovered_pool.clone(),
            rt,
        ))),
    );

    let redelivery = tokio::task::block_in_place(|| recovered_consumer.deliver(&message));
    assert_eq!(
        redelivery,
        Delivered::Acked,
        "after recovery the same delivery commits its mark and effect together"
    );
    assert!(
        dedup_present(&recovered_pool, &consumer_name.0, &event_id.0).await,
        "the recovered delivery leaves a durable dedup mark"
    );
    assert!(
        effect_present(&recovered_pool, &effect, &event_id.0).await,
        "the recovered delivery leaves its intended effect"
    );
    assert_eq!(
        tokio::task::block_in_place(|| recovered_consumer.deliver(&message)),
        Delivered::Deduplicated,
        "later broker redelivery observes the committed mark"
    );
    assert_eq!(
        recovered_consumer.handler().ran.load(Ordering::SeqCst),
        1,
        "the user-visible effect happens exactly once across outage and recovery"
    );
}
