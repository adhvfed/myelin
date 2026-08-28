pub mod config;
pub mod consumer;
pub mod dispatch;
pub mod migrations;
pub mod resolve;

pub use config::{
    parse_ci_config, parse_versioned_ci_config, CiConfigError, ConfigFormat, VersionedCiConfigError,
};

#[cfg(any(test, feature = "test-support", feature = "integration"))]
pub use consumer::OutboxReserveStore;
pub use consumer::{
    build_trigger_consumer, ci_run_insert_from_armed, ci_trigger_subjects, plan_dispatch,
    resolve_ci_config, ArmedRun, AuthoritativeGitRoot, CiTriggerHandler, CoCommitReserveStore,
    DispatchOutcome, DurableGitConfigReader, GitConfigReader, GitReadError, GitRootError,
    ReserveError, ReserveFacts, ReserveStore, SkipReason, CI_TRIGGER_SUBJECT_STRS,
};

pub use dispatch::{
    classify_trust, compile_trigger, git_trust_of, stamp_trust, trigger_matches, DedupLedger,
    OnTrigger, RunProvenance, TrustStamp, TrustTier, RUN_OBJECT_TYPE, TRIGGER_CONSUMER,
};

pub use resolve::{
    reserve_and_start, resolve_snapshot, resolve_versioned_snapshot, snapshot_ref, CheckContext,
    CiDefinition, CiPlanContract, CiRunWrite, JobDef, JobKind, ResolveError, ResolvedJob,
    ResolvedJobV1, ResolvedJobV2, ResolvedRunPlanV1, ResolvedRunPlanV2, ResolvedSnapshot,
    ResolvedSnapshotExt, RunFacts, StartHandoff, StartSpec, StructuredBuildToolV1,
    StructuredBuildV1, VersionedCiDefinition, VersionedResolvedSnapshot, CI_PIPELINE_WF_TYPE,
};

use myelin_substrate::{
    AppSpec, Config, ConsumerReg, CriticalDependencies, InternalRpc, OutboxSpec, PublicRoutes,
    ServeError,
};
use std::sync::Arc;
use std::sync::Mutex;

pub use migrations::{dispatch_migrations, CONSUMER_DEDUP_TABLE, CREATE_CONSUMER_DEDUP_DDL};

pub const SERVICE_NAME: &str = "ci-dispatch";
pub const EVENT_STREAM_NAME: &str = "MYELIN_EVENTS";
pub const EVENT_SUBJECT_ROOT: &str = "myelin.events";
pub const EVENT_DURABLE_CONSUMER: &str = "ci-dispatch-trigger";

pub trait BlobIntakeReadiness: Send + Sync {
    fn readiness(&self) -> Result<(), myelin_storage::blob::BlobDependencyError>;
}

impl BlobIntakeReadiness for myelin_storage::s3blob::S3BlobStore {
    fn readiness(&self) -> Result<(), myelin_storage::blob::BlobDependencyError> {
        myelin_storage::s3blob::S3BlobStore::readiness(self)
    }
}

trait IntakeFactory: Send + Sync {
    fn connect(
        &self,
    ) -> Result<Box<dyn myelin_events::EventConsumer>, myelin_events::TransportError>;
}

struct NatsIntakeFactory {
    config: myelin_events::nats::JetStreamConsumerConfig,
    rt: tokio::runtime::Handle,
}

impl IntakeFactory for NatsIntakeFactory {
    fn connect(
        &self,
    ) -> Result<Box<dyn myelin_events::EventConsumer>, myelin_events::TransportError> {
        myelin_events::nats::NatsJetStreamBus::connect_consumer(
            self.config.clone(),
            self.rt.clone(),
        )
        .map(|consumer| Box::new(consumer) as Box<dyn myelin_events::EventConsumer>)
    }
}

pub struct RecoveringIntake {
    durable_name: String,
    blobs: Arc<dyn BlobIntakeReadiness>,
    factory: Box<dyn IntakeFactory>,
    active: Mutex<Option<Box<dyn myelin_events::EventConsumer>>>,
}

impl RecoveringIntake {
    pub fn new(
        config: myelin_events::nats::JetStreamConsumerConfig,
        blobs: Arc<myelin_storage::s3blob::S3BlobStore>,
        rt: tokio::runtime::Handle,
    ) -> Self {
        let durable_name = config.consumer_name.clone();
        Self {
            durable_name,
            blobs,
            factory: Box::new(NatsIntakeFactory { config, rt }),
            active: Mutex::new(None),
        }
    }

    #[cfg(test)]
    fn with_factory(
        durable_name: impl Into<String>,
        blobs: Arc<dyn BlobIntakeReadiness>,
        factory: Box<dyn IntakeFactory>,
    ) -> Self {
        Self {
            durable_name: durable_name.into(),
            blobs,
            factory,
            active: Mutex::new(None),
        }
    }

    fn with_active<R>(
        &self,
        operation: impl FnOnce(
            &dyn myelin_events::EventConsumer,
        ) -> Result<R, myelin_events::TransportError>,
    ) -> Result<R, myelin_events::TransportError> {
        let guard = self
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let active = guard
            .as_deref()
            .ok_or_else(|| myelin_events::TransportError("durable intake is not active".into()))?;
        operation(active)
    }
}

impl myelin_events::EventConsumer for RecoveringIntake {
    fn durable_name(&self) -> &str {
        &self.durable_name
    }

    fn pre_intake_readiness(
        &self,
    ) -> Result<
        Option<myelin_events::relay::IntakeDependency>,
        myelin_events::relay::IntakeDependency,
    > {
        self.blobs
            .readiness()
            .map(|()| Some(myelin_events::relay::IntakeDependency::Blob))
            .map_err(|_| myelin_events::relay::IntakeDependency::Blob)
    }

    fn consume(
        &self,
        subject_prefix: &str,
    ) -> Result<Vec<myelin_events::BrokerDelivery>, myelin_events::TransportError> {
        self.blobs.readiness().map_err(|_| {
            myelin_events::TransportError("blob dependency unavailable before broker intake".into())
        })?;
        let mut guard = self
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if guard.is_none() {
            *guard = Some(self.factory.connect()?);
        }
        guard
            .as_deref()
            .expect("intake was connected")
            .consume(subject_prefix)
    }

    fn flush_settlements(&self) -> Result<(), myelin_events::TransportError> {
        let guard = self
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match guard.as_deref() {
            Some(active) => active.flush_settlements(),
            None => Ok(()),
        }
    }

    fn ack(
        &self,
        token: myelin_events::DeliveryToken,
    ) -> Result<(), myelin_events::TransportError> {
        self.with_active(|active| active.ack(token))
    }

    fn retry(
        &self,
        token: myelin_events::DeliveryToken,
        delay_secs: u64,
    ) -> Result<(), myelin_events::TransportError> {
        self.with_active(|active| active.retry(token, delay_secs))
    }

    fn terminate(
        &self,
        token: myelin_events::DeliveryToken,
    ) -> Result<(), myelin_events::TransportError> {
        self.with_active(|active| active.terminate(token))
    }
}

pub fn git_intake_filter() -> String {
    format!("{EVENT_SUBJECT_ROOT}.evt.*.git.>")
}

#[allow(clippy::too_many_arguments)]
pub fn build_dispatch_consumers(
    git_root: AuthoritativeGitRoot,
    blobs: Arc<dyn myelin_storage::BlobStore + Send + Sync>,
    ci_run: myelin_ci_controlplane::CiRunStore,
    dedup: myelin_events::DedupLedger,
    dead_letters: std::sync::Arc<dyn myelin_events::DurableDeadLetter>,
    expected_region: impl Into<String>,
    minter: std::sync::Arc<dyn myelin_events::IdMinter>,
    rt: tokio::runtime::Handle,
    admission: myelin_events::DurableWorkerAdmission,
) -> Result<Vec<ConsumerReg>, myelin_events::SubscribeError> {
    use std::sync::Arc;
    let reader: Arc<dyn consumer::GitConfigReader> =
        Arc::new(consumer::DurableGitConfigReader::new(
            myelin_git::durable::DurableGitStore::rooted(git_root.as_path()),
        ));
    let reserve: Arc<dyn consumer::ReserveStore> =
        Arc::new(consumer::CoCommitReserveStore::new(ci_run, minter, rt));
    Ok(vec![consumer::build_trigger_consumer(
        reader,
        blobs,
        reserve,
        dedup,
        expected_region,
        dead_letters,
        admission,
    )?])
}

fn dispatch_critical() -> CriticalDependencies {
    CriticalDependencies::new(["broker", "authz", "blob"])
}

fn dispatch_app_spec(
    config: Config,
    outbox: myelin_events::OutboxStore,
    consumers: Vec<ConsumerReg>,
) -> AppSpec {
    AppSpec {
        name: SERVICE_NAME,
        config,
        migrations: dispatch_migrations(),
        hot_tables: myelin_substrate::HotTables::none(),
        public: PublicRoutes::default(),
        internal: InternalRpc::default(),
        consumers,
        outbox: OutboxSpec::external_relay(outbox),
        critical: dispatch_critical(),
        intake_scope: None,
    }
}

pub fn dispatch_app_spec_with_intake(
    config: Config,
    outbox: myelin_events::OutboxStore,
    consumers: Vec<ConsumerReg>,
    intake: Box<dyn myelin_events::EventConsumer>,
    delivery_quarantine: Arc<dyn myelin_events::DurableDeliveryQuarantine>,
) -> AppSpec {
    let mut spec = dispatch_app_spec(config, outbox.clone(), consumers);
    spec.outbox = OutboxSpec::external_relay_with_consumer(outbox, intake, delivery_quarantine);
    spec
}

#[cfg(test)]
fn boot_dispatch(
    config: Config,
    outbox: myelin_events::OutboxStore,
    consumers: Vec<ConsumerReg>,
) -> Result<myelin_substrate::ServeHandle, ServeError> {
    myelin_substrate::boot(dispatch_app_spec(config, outbox, consumers))
}

#[cfg(test)]
fn run_dispatch(
    config: Config,
    outbox: myelin_events::OutboxStore,
    consumers: Vec<ConsumerReg>,
) -> Result<(), ServeError> {
    myelin_substrate::serve(dispatch_app_spec(config, outbox, consumers))
}

pub async fn run_dispatch_until_shutdown<F>(
    config: Config,
    outbox: myelin_events::OutboxStore,
    consumers: Vec<ConsumerReg>,
    intake: Box<dyn myelin_events::EventConsumer>,
    delivery_quarantine: Arc<dyn myelin_events::DurableDeliveryQuarantine>,
    shutdown: F,
) -> Result<(), ServeError>
where
    F: std::future::Future<Output = ()>,
{
    myelin_substrate::serve_until_shutdown(
        dispatch_app_spec_with_intake(config, outbox, consumers, intake, delivery_quarantine),
        shutdown,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_substrate::{boot, Liveness, Surface};

    #[test]
    fn bare_test_spec_boots_with_three_ports() {
        let handle = boot_dispatch(
            Config::default(),
            myelin_events::OutboxStore::new(),
            Vec::new(),
        )
        .expect("the CI dispatch test shell boots from serve(AppSpec)");
        assert_eq!(handle.name(), SERVICE_NAME, "the deployable service name");

        assert_eq!(
            handle.surfaces(),
            &[Surface::Public, Surface::Internal, Surface::MetricsHealth],
            "the three ports opened (contract 1.2)"
        );

        let mh = handle.metrics_health();
        assert_eq!(
            mh.liveness(),
            Liveness::Up,
            "liveness = not-wedged (never checks a dependency)"
        );
        assert!(
            mh.readiness().is_ready(),
            "readiness = can-serve-now (all critical deps healthy at boot) - distinct from liveness"
        );
    }

    #[test]
    fn dead_broker_flips_readiness_not_liveness() {
        let handle = boot_dispatch(
            Config::default(),
            myelin_events::OutboxStore::new(),
            Vec::new(),
        )
        .expect("boot");
        let mh = handle.metrics_health();
        assert!(
            mh.readiness().is_ready(),
            "ready while the broker is healthy"
        );

        handle.health_probe().mark_down("broker");

        assert!(
            !mh.readiness().is_ready(),
            "a dead broker → not-ready + shed (Trigger & Dispatch is close to the bus)"
        );
        assert_eq!(
            mh.liveness(),
            Liveness::Up,
            "liveness stays UP (not-ready is NOT not-alive - no restart storm)"
        );
    }

    #[test]
    fn run_dispatch_runs_lifecycle_and_returns_ok() {
        assert_eq!(
            run_dispatch(
                Config::default(),
                myelin_events::OutboxStore::new(),
                Vec::new()
            ),
            Ok(()),
            "the Trigger & Dispatch shell boots → … → drains cleanly"
        );
    }

    #[test]
    fn failed_boot_returns_non_zero() {
        let r = run_dispatch(
            Config("BAD_POOL".into()),
            myelin_events::OutboxStore::new(),
            Vec::new(),
        );
        assert!(r.is_err(), "a failed boot must return non-zero (Err)");
        assert!(
            r.unwrap_err().0.contains("fail-fast"),
            "the error names the §3.2 fail-fast validation"
        );
    }

    #[test]
    fn bare_spec_carries_the_dedup_ledger_and_explicit_consumer() {
        use std::sync::Arc;
        let handler = consumer::CiTriggerHandler::new(
            Arc::new(consumer::MapGitConfigReader::new()),
            Arc::new(myelin_storage::FsBlobStore::new()),
            Arc::new(consumer::RecordingReserveStore::new()),
        );
        let sub = myelin_events::consumer::Subscription::bind(
            myelin_events::ConsumerName(TRIGGER_CONSUMER.into()),
            CI_TRIGGER_SUBJECT_STRS,
            myelin_events::PrefetchBound::DEFAULT,
        )
        .expect("the CI trigger whitelist binds (never `*`)");
        let reg = ConsumerReg::new(myelin_events::Consumer::new(
            handler,
            sub,
            myelin_events::DedupLedger::new(),
        ));

        let spec = dispatch_app_spec(
            Config::default(),
            myelin_events::OutboxStore::new(),
            vec![reg],
        );
        assert_eq!(
            spec.migrations.0.len(),
            1,
            "Dispatch repeats the one foundation-owned dedup declaration"
        );
        assert_eq!(
            spec.migrations.0[0].table.as_deref(),
            Some(CONSUMER_DEDUP_TABLE),
            "the migration declares the shared consumer_dedup ledger"
        );
        assert_eq!(
            spec.consumers.len(),
            1,
            "the injected ci-dispatch.trigger consumer is registered (CT-004b - dispatch FIRES)"
        );
        let deps: Vec<&str> = spec.critical.deps().iter().map(|d| d.0.as_str()).collect();
        assert!(
            deps.contains(&"broker"),
            "broker is critical (close to the bus)"
        );
        assert!(
            deps.contains(&"authz"),
            "authz is critical (the trust-tier ABAC edge)"
        );

        let empty = dispatch_app_spec(
            Config::default(),
            myelin_events::OutboxStore::new(),
            Vec::new(),
        );
        assert!(empty.consumers.is_empty(), "the bare test spec stays empty");
    }

    #[test]
    fn the_injected_consumer_boots_and_drains() {
        use std::sync::Arc;
        let handler = consumer::CiTriggerHandler::new(
            Arc::new(consumer::MapGitConfigReader::new()),
            Arc::new(myelin_storage::FsBlobStore::new()),
            Arc::new(consumer::RecordingReserveStore::new()),
        );
        let sub = myelin_events::consumer::Subscription::bind(
            myelin_events::ConsumerName(TRIGGER_CONSUMER.into()),
            CI_TRIGGER_SUBJECT_STRS,
            myelin_events::PrefetchBound::DEFAULT,
        )
        .unwrap();
        let reg = ConsumerReg::new(myelin_events::Consumer::new(
            handler,
            sub,
            myelin_events::DedupLedger::new(),
        ));
        let outbox = myelin_events::OutboxStore::new();
        let mut spec = dispatch_app_spec(Config::default(), outbox.clone(), vec![reg]);
        spec.outbox = OutboxSpec::new(outbox, myelin_events::InProcessBus::new());
        let handle = boot(spec).expect("explicit embedded test intake boots");
        handle.tick();
        handle.signal_drain();
        let final_telemetry = handle.drain();
        assert_eq!(
            final_telemetry.outbox_depth(),
            0,
            "the live trigger consumer boots → … → drains cleanly"
        );
    }

    struct ToggleBlobReadiness(std::sync::atomic::AtomicBool);
    impl BlobIntakeReadiness for ToggleBlobReadiness {
        fn readiness(&self) -> Result<(), myelin_storage::blob::BlobDependencyError> {
            if self.0.load(std::sync::atomic::Ordering::SeqCst) {
                Ok(())
            } else {
                Err(myelin_storage::blob::BlobDependencyError::Transient)
            }
        }
    }

    #[derive(Default)]
    struct IntakeCounts {
        connects: std::sync::atomic::AtomicUsize,
        pulls: std::sync::atomic::AtomicUsize,
        flushes: std::sync::atomic::AtomicUsize,
    }
    struct ProbeIntakeFactory(Arc<IntakeCounts>);
    impl IntakeFactory for ProbeIntakeFactory {
        fn connect(
            &self,
        ) -> Result<Box<dyn myelin_events::EventConsumer>, myelin_events::TransportError> {
            self.0
                .connects
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(Box::new(ProbeIntake(self.0.clone())))
        }
    }
    struct ProbeIntake(Arc<IntakeCounts>);
    impl myelin_events::EventConsumer for ProbeIntake {
        fn durable_name(&self) -> &str {
            "probe-intake"
        }
        fn consume(
            &self,
            _: &str,
        ) -> Result<Vec<myelin_events::BrokerDelivery>, myelin_events::TransportError> {
            self.0
                .pulls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(Vec::new())
        }
        fn flush_settlements(&self) -> Result<(), myelin_events::TransportError> {
            self.0
                .flushes
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
        fn ack(
            &self,
            _: myelin_events::DeliveryToken,
        ) -> Result<(), myelin_events::TransportError> {
            Ok(())
        }
        fn retry(
            &self,
            _: myelin_events::DeliveryToken,
            _: u64,
        ) -> Result<(), myelin_events::TransportError> {
            Ok(())
        }
        fn terminate(
            &self,
            _: myelin_events::DeliveryToken,
        ) -> Result<(), myelin_events::TransportError> {
            Ok(())
        }
    }
    struct QuarantineNoop;
    impl myelin_events::DurableDeliveryQuarantine for QuarantineNoop {
        fn record(
            &self,
            _: &str,
            _: &myelin_events::BrokerDeliveryRef,
            _: myelin_events::DeliveryQuarantineReason,
            _: u64,
        ) -> Result<(), String> {
            Ok(())
        }
    }

    fn recovering_probe(
        blob: Arc<ToggleBlobReadiness>,
        counts: Arc<IntakeCounts>,
    ) -> RecoveringIntake {
        RecoveringIntake::with_factory("probe-intake", blob, Box::new(ProbeIntakeFactory(counts)))
    }

    #[test]
    fn transient_blob_boot_is_immediately_not_ready_then_recovers_with_one_connect() {
        let blob = Arc::new(ToggleBlobReadiness(std::sync::atomic::AtomicBool::new(
            false,
        )));
        let counts = Arc::new(IntakeCounts::default());
        let handle = boot(dispatch_app_spec_with_intake(
            Config::default(),
            myelin_events::OutboxStore::new(),
            Vec::new(),
            Box::new(recovering_probe(blob.clone(), counts.clone())),
            Arc::new(QuarantineNoop),
        ))
        .unwrap();
        assert_eq!(handle.metrics_health().liveness(), Liveness::Up);
        assert!(!handle.metrics_health().readiness().is_ready());
        assert_eq!(counts.connects.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(counts.pulls.load(std::sync::atomic::Ordering::SeqCst), 0);

        blob.0.store(true, std::sync::atomic::Ordering::SeqCst);
        handle.tick();
        handle.tick();
        assert!(handle.metrics_health().readiness().is_ready());
        assert_eq!(counts.connects.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(counts.pulls.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[test]
    fn initial_blob_outage_shutdown_does_not_connect_or_pull() {
        let blob = Arc::new(ToggleBlobReadiness(std::sync::atomic::AtomicBool::new(
            false,
        )));
        let counts = Arc::new(IntakeCounts::default());
        let handle = boot(dispatch_app_spec_with_intake(
            Config::default(),
            myelin_events::OutboxStore::new(),
            Vec::new(),
            Box::new(recovering_probe(blob, counts.clone())),
            Arc::new(QuarantineNoop),
        ))
        .unwrap();
        handle.drain_checked().unwrap();
        assert_eq!(counts.connects.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(counts.pulls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn active_blob_outage_shutdown_flushes_without_reconnect_or_pull() {
        let blob = Arc::new(ToggleBlobReadiness(std::sync::atomic::AtomicBool::new(
            true,
        )));
        let counts = Arc::new(IntakeCounts::default());
        let handle = boot(dispatch_app_spec_with_intake(
            Config::default(),
            myelin_events::OutboxStore::new(),
            Vec::new(),
            Box::new(recovering_probe(blob.clone(), counts.clone())),
            Arc::new(QuarantineNoop),
        ))
        .unwrap();
        handle.tick();
        assert_eq!(counts.connects.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(counts.pulls.load(std::sync::atomic::Ordering::SeqCst), 1);

        blob.0.store(false, std::sync::atomic::Ordering::SeqCst);
        handle.drain_checked().unwrap();

        assert_eq!(counts.flushes.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(counts.connects.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(counts.pulls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
