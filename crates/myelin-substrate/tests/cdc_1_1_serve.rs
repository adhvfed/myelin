//! # CDC 1.1 — the `serve(AppSpec)` provider ⇄ a hello-world `main` consumer (P-S12 → P-010)
//!
//! **Contract:** index row 1.1 (`serve(AppSpec)` — boot → migrate → outbox relay → consumers →
//! three ports → graceful drain). The contract-coverage scanner (P-S21) reads BOTH halves:
//! - **provider** = `myelin_substrate::serve` / `boot` (the lifecycle), unit-tested in
//!   `src/serve.rs`;
//! - **consumer** = a hello-world `main`-shaped caller that constructs an `AppSpec` and calls
//!   the lifecycle — THIS file. It also exercises the **producer side of the contract-1.8
//!   telemetry signal set** (architecture §3.5) by reading the signals `serve` exports back
//!   through the harness's telemetry-assertion library (P-S04) — the SAME `SignalName` set —
//!   so the green artifact is "the lifecycle ran AND the §10.2 signal set was produced".
//!
//! This is the dated green artifact the P-S12 GATE/DRILLS names: the hello-world boot
//! (boot → emit → consume → drain) passing and the telemetry signal set being produced.

use myelin_events::relay::InProcessBus;
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, Consumer, ConsumerName, DataRole, DedupLedger, EmitContextBase,
    EventDraft, EventEnvelope, EventHandler, EventType, HandleOutcome, IdMinter, MonotonicMinter,
    OutboxStore, OutboxTx, PrefetchBound, SubjectPattern, Subscription, Timestamp, Visibility,
};
use myelin_harness::{Predicate, SignalName, SignalSource};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_substrate::serve::{boot, AppSpec, ConsumerReg, OutboxSpec, Surface};
use myelin_substrate::{
    Config, CriticalDependencies, HolderRegistration, HotTables, InternalRpc, Migrations,
    PublicRoutes, StoreKind,
};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

static SUBJECTS: &[SubjectPattern] = &[];

/// The hello-world consumer body (a service's `EventHandler`) — counts what it processed.
struct Indexer {
    runs: Arc<AtomicU32>,
}
impl EventHandler for Indexer {
    fn subjects(&self) -> &'static [SubjectPattern] {
        SUBJECTS
    }
    fn handle(&self, _ev: &EventEnvelope) -> HandleOutcome {
        self.runs.fetch_add(1, Ordering::SeqCst);
        HandleOutcome::Done
    }
}

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: myelin_tenancy::TenantId("acme".into()),
        region: myelin_tenancy::Region("eu-west".into()),
        actor: Actor(Principal::stub(PrincipalId("p".into()), PrincipalKind::Human, myelin_tenancy::TenantId("acme".into()))),
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

/// **CDC 1.1 — the consumer side.** A hello-world `main`-shaped caller constructs an `AppSpec`,
/// boots the lifecycle, emits one event through the outbox, the relay publishes it, the consumer
/// processes it, and the graceful drain leaves the producer telemetry reading green:
/// `outbox_depth == 0`, `dead_letter_count == 0`, `consumer_lag{indexer} == 0` — read off the
/// producer side through the harness's telemetry-assertion library (the §10.2 signal set).
#[test]
fn cdc_1_1_hello_world_main_boots_emits_consumes_drains_and_emits_signals() {
    // A service's own outbox store (its handlers emit into it; the relay drains it).
    let outbox = OutboxStore::new();
    let runs = Arc::new(AtomicU32::new(0));
    let sub = Subscription::bind(
        ConsumerName("indexer".into()),
        &["myelin://acme/issues/"],
        PrefetchBound::DEFAULT,
    )
    .unwrap();
    let consumer = Consumer::new(Indexer { runs: runs.clone() }, sub, DedupLedger::new());

    // The hello-world AppSpec (the §3.1 verbatim shape: name/config/migrations/public/internal/
    // consumers/holders/outbox).
    let spec = AppSpec {
        name: "hello",
        config: Config::default(),
        migrations: Migrations::new([("0010_hello", "CREATE TABLE IF NOT EXISTS hello (id TEXT)")]),
        hot_tables: HotTables::none(),
        public: PublicRoutes::default(),
        internal: InternalRpc::default(),
        consumers: vec![ConsumerReg::new(consumer)],
        holders: AppSpec::auto(),
        outbox: OutboxSpec::new(outbox.clone(), InProcessBus::new()),
        critical: CriticalDependencies::default(),
    };

    // boot the lifecycle (provider side).
    let handle = boot(spec).expect("the hello-world service boots from serve(AppSpec)");
    assert_eq!(
        handle.surfaces(),
        &[Surface::Public, Surface::Internal, Surface::MetricsHealth],
        "the three-surface topology opened in the lifecycle (1.2 seam, SUB-D7/D9 are P-S13/P-S14)"
    );
    assert_eq!(
        handle.registered_holders(),
        &[HolderRegistration { kind: StoreKind::Oltp, name: "hello" }],
        "the opened store auto-registered as a holder (§3.4)"
    );

    // emit one event through the outbox (a handler's co-committed state-change + event).
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let mut tx = outbox.begin(minter, ctx_base());
    tx.stage_state_change("hello created");
    tx.emit(draft(), None).unwrap();
    tx.commit().unwrap();

    // serve one tick: the relay publishes the event + the consumer processes it.
    let delivered = handle.tick();
    assert_eq!(delivered, vec![(ConsumerName("indexer".into()), 1)]);
    assert_eq!(runs.load(Ordering::SeqCst), 1, "the consumer processed the event exactly once");

    // graceful drain.
    handle.signal_drain();

    // --- the producer side of the contract-1.8 signal set, asserted via the P-S04 library ---
    // Populate the harness's SignalSource from the producer telemetry serve exports (the SAME
    // SignalName set the harness reads); assert each survival signal is green.
    let t = handle.telemetry();
    let mut src = SignalSource::new();
    src.set_scalar(SignalName::OutboxDepth, t.outbox_depth());
    src.set_scalar(SignalName::DeadLetterCount, t.dead_letter_count());
    src.set_labelled(
        SignalName::ConsumerLag,
        vec![myelin_harness::Label::new("consumer", "indexer")],
        t.consumer_lag("indexer").expect("the indexer consumer's lag is exported"),
    );

    // outbox drained to 0 (nothing committed left unpublished — the SUB-D1 zero, read via serve).
    src.assert_signal(SignalName::OutboxDepth, Predicate::Eq(0)).expect_green();
    // no dead letters on the happy path.
    src.assert_signal(SignalName::DeadLetterCount, Predicate::Eq(0)).expect_green();
    // the consumer fully caught up (rule 7 lag recovered to 0).
    src.assert_labelled(
        SignalName::ConsumerLag,
        vec![myelin_harness::Label::new("consumer", "indexer")],
        Predicate::Eq(0),
    )
    .expect_green();
}
