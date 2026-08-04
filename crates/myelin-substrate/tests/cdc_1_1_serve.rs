use myelin_events::relay::InProcessBus;
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, Consumer, ConsumerName, DataRole, DedupLedger,
    EmitContextBase, EventDraft, EventEnvelope, EventHandler, EventType, HandleOutcome, IdMinter,
    MonotonicMinter, OutboxStore, OutboxTx, PrefetchBound, SubjectPattern, Subscription, Timestamp,
    Visibility,
};
use myelin_harness::{Predicate, SignalName, SignalSource};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_substrate::serve::{boot, AppSpec, ConsumerReg, OutboxSpec, Surface};
use myelin_substrate::{
    Config, CriticalDependencies, HolderRegistration, HotTables, InternalRpc, Migrations,
    PublicRoutes, StoreKind, StoreManifest,
};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

static SUBJECTS: &[SubjectPattern] = &[];

struct Indexer {
    runs: Arc<AtomicU32>,
}
impl EventHandler for Indexer {
    fn subjects(&self) -> &'static [SubjectPattern] {
        SUBJECTS
    }
    fn handle(&self, _ev: &EventEnvelope, _tx: &mut myelin_events::HandlerTx<'_>) -> HandleOutcome {
        self.runs.fetch_add(1, Ordering::SeqCst);
        HandleOutcome::Done
    }
}

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: myelin_tenancy::TenantId("acme".into()),
        region: myelin_tenancy::Region("eu-west".into()),
        actor: Actor(Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            myelin_tenancy::TenantId("acme".into()),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-19T00:00:01Z".into()),
        caused_by: None,
    }
}

fn draft() -> EventDraft {
    EventDraft {
        type_: EventType("issues.issue.created".into()),
        subject: ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()),
        aggregate: AggregateKey("issue:PROJ-1".into()),
        payload: serde_json::json!({ "ref": "PROJ-1" }),
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

#[test]
fn cdc_1_1_hello_world_main_boots_emits_consumes_drains_and_emits_signals() {
    let outbox = OutboxStore::new();
    let runs = Arc::new(AtomicU32::new(0));
    let sub = Subscription::bind(
        ConsumerName("indexer".into()),
        &["myelin://acme/issues/"],
        PrefetchBound::DEFAULT,
    )
    .unwrap();
    let consumer = Consumer::new(Indexer { runs: runs.clone() }, sub, DedupLedger::new());

    let spec = AppSpec {
        name: "hello",
        config: Config::default(),
        migrations: Migrations::new([("0010_hello", "CREATE TABLE IF NOT EXISTS hello (id TEXT)")]),
        hot_tables: HotTables::none(),
        public: PublicRoutes::default(),
        internal: InternalRpc::default(),
        consumers: vec![ConsumerReg::new(consumer)],
        holders: AppSpec::auto(),
        stores: StoreManifest::new(),
        outbox: OutboxSpec::new(outbox.clone(), InProcessBus::new()),
        critical: CriticalDependencies::default(),
    };

    let handle = boot(spec).expect("the hello-world service boots from serve(AppSpec)");
    assert_eq!(
        handle.surfaces(),
        &[Surface::Public, Surface::Internal, Surface::MetricsHealth],
        "the three-surface topology opened in the lifecycle (1.2 seam, SUB-D7/D9 are P-S13/P-S14)"
    );
    assert_eq!(
        handle.registered_holders(),
        &[HolderRegistration {
            kind: StoreKind::Oltp,
            name: "hello"
        }],
        "the opened store auto-registered as a holder (§3.4)"
    );

    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let mut tx = outbox.begin(minter, ctx_base());
    tx.stage_state_change("hello created");
    tx.emit(draft(), None).unwrap();
    tx.commit().unwrap();

    let delivered = handle.tick();
    assert_eq!(delivered, vec![(ConsumerName("indexer".into()), 1)]);
    assert_eq!(
        runs.load(Ordering::SeqCst),
        1,
        "the consumer processed the event exactly once"
    );

    handle.signal_drain();

    let t = handle.telemetry();
    let mut src = SignalSource::new();
    src.set_scalar(SignalName::OutboxDepth, t.outbox_depth());
    src.set_scalar(SignalName::DeadLetterCount, t.dead_letter_count());
    src.set_labelled(
        SignalName::ConsumerLag,
        vec![myelin_harness::Label::new("consumer", "indexer")],
        t.consumer_lag("indexer")
            .expect("the indexer consumer's lag is exported"),
    );

    src.assert_signal(SignalName::OutboxDepth, Predicate::Eq(0))
        .expect_green();
    src.assert_signal(SignalName::DeadLetterCount, Predicate::Eq(0))
        .expect_green();
    src.assert_labelled(
        SignalName::ConsumerLag,
        vec![myelin_harness::Label::new("consumer", "indexer")],
        Predicate::Eq(0),
    )
    .expect_green();
}
