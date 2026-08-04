use serde::{Deserialize, Serialize};

pub mod agent_load;
pub mod crate_graph;
pub mod fail_static;
pub mod fail_static_authz;
pub mod firehose;
pub mod firehose_selector;
pub mod holder_catalog;
pub mod holder_registered;
pub mod holders;
pub mod metrics_health;
pub mod migrations;
pub mod overlay;
pub mod serve;
pub mod shed;
pub mod thresholds;
pub mod topology;

pub use agent_load::{
    count_by_root, AgentLoadGuard, AgentLoadSignals, BudgetBreach, DepthCeiling, DepthVerdict,
    DispatchAdmission, DispatchPool, GuardOutcome, PredicateGuard, PredicateVerdict,
    SharedRootTripwire, TripwireVerdict,
};
pub use fail_static::{
    Answer, Clock, FailStatic, FailStaticError, FailStaticSignals, StalenessBound, SystemClock,
    TestClock,
};
pub use fail_static_authz::{
    encode_authz_key, AuthzDecision, AuthzServed, CoarseAuthz, FailStaticAuthz, AUTHZ_FRESH_TTL_SECS,
};
pub use firehose::{
    FirehoseScope, FirehoseSignals, Frame, FrameBuffer, FrameClass, FrameLagSample, PushOutcome,
};
pub use firehose_selector::{
    BoundedSelector, FrameBudgetVerdict, FrameOutcome, FrameSelector, FrameShedBudget, ScopeWindow,
    SelectorError, SelectorKind, WindowVerdict,
};
pub use holder_catalog::{
    assert_holder_completeness, classify_store, holder_completeness, Holder, OrphanStore,
    StoreClassifier, StoreHolder,
};
pub use holder_registered::{
    assert_all_holders_registered, holder_registered, DeclaredStore, HolderViolation, StoreManifest,
};
pub use holders::{HolderRegistration, HolderRegistry, StoreKind};
pub use metrics_health::{
    CriticalDependencies, CriticalDependency, DependencyHealth, HealthTable, Liveness,
    LivenessState, MetricsHealthSurface, Readiness, ReadinessReport, Startup,
};
pub use migrations::{
    is_blocking_alter, is_destructive, HotTables, Migration, MigrationPhase, MigrationRunner,
    Migrations,
};
pub use overlay::{
    center_dialog, place_overlay, reachable_within, FocusId, FocusMove, FocusTrap, Placement, Px,
    Rect, Side,
};
pub use serve::{
    boot, serve, serve_until_shutdown, AppSpec, ConsumerReg, HoldersSpec, InternalRpc, OutboxSpec,
    PortOpener, PublicRoutes, ServeHandle, Surface, Telemetry,
};
pub use shed::{
    BoundedQueue, RunClass, RunClassHeader, ShedBudgetError, ShedBudgetTable, ShedDecision,
    ShedLane, Surface as ShedSurface, SurfaceBudget,
};
pub use thresholds::{
    CellSizing, ClaimedNotProven, DepthCeilings, DsrDeadline, FailStaticThreshold, FlexDb,
    Revocation, RpoRto, ShedBudgetRow, Surge, ThresholdError, Thresholds, THRESHOLDS_FILENAME,
};
pub use topology::{
    AllowPrincipal, AuditSink, Authorizer, DenyAll, IdorAuditRecord, InjectedIdentity,
    InternalReject, InternalSurface, PublicReject, PublicSurface,
};

pub type Seconds = u64;

pub const fn perf_budget_enforced() -> bool {
    cfg!(feature = "host-perf-tests")
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config(pub String);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServeError(pub String);

impl core::fmt::Display for ServeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ServeError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serve_and_appspec_shape_is_frozen() {
        let spec = AppSpec {
            name: "hello",
            config: Config::default(),
            migrations: Migrations::default(),
            hot_tables: HotTables::none(),
            public: PublicRoutes::default(),
            internal: InternalRpc::default(),
            consumers: vec![],
            holders: HoldersSpec::Auto,
            stores: StoreManifest::new(),
            outbox: OutboxSpec::default(),
            critical: CriticalDependencies::default(),
        };
        assert_eq!(spec.name, "hello");
        assert_eq!(spec.holders, HoldersSpec::Auto);
        let _f: fn(AppSpec) -> Result<(), ServeError> = serve;
    }

    #[test]
    fn fail_static_shape_and_units_are_frozen() {
        let bound = StalenessBound {
            revocation_sla_secs: 300,
            agent_token_ttl_secs: 60,
        };
        let fs: FailStatic<&str, u8> = FailStatic::try_new(30, 300, bound).expect("valid bound");
        assert_eq!(fs.fresh_ttl(), 30u64);
        assert_eq!(fs.static_max(), 300u64);
        assert!(fs.static_max() >= fs.fresh_ttl());
        let a: Answer<u8> = Answer::Static(1);
        assert!(a.is_static());
        let _closed: Answer<u8> = Answer::Closed;
    }
}
