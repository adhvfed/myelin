use myelin_events::Timestamp;
use myelin_identity::{
    Consistency, ConsistencyMode, DataRole, Decision, IdentityService, ObjectId, Permission,
    Principal, PrincipalId, PrincipalKind, PrincipalStatus, RelName, RelationTuple, TupleDelta,
    Zookie,
};
use myelin_identity_service::{StoreBackedCheck, TupleStore};
use myelin_issues::pg_issue_store::MAX_PAGE_SIZE;
use myelin_issues::{
    IssueAuthorizationBinding, IssueAuthorizationOutcome, IssueAuthorizer, IssuePermission,
    IssueStoreError, IssueTupleWriter, PgIssueStore, VisibleIssues,
};
use myelin_storage::TenantScope;
use myelin_tenancy::{ArtifactRef, Region, TenantId};
use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::JoinHandle;

pub const ISSUE_RECONCILE_TENANTS_ENV: &str = "MYELIN_ISSUES_RECONCILE_TENANTS";
const ISSUE_RECONCILE_BATCH_ENV: &str = "MYELIN_ISSUES_RECONCILE_BATCH";
const ISSUE_RECONCILE_INTERVAL_MS_ENV: &str = "MYELIN_ISSUES_RECONCILE_INTERVAL_MS";
const ISSUE_RECONCILE_MAX_BACKOFF_MS_ENV: &str = "MYELIN_ISSUES_RECONCILE_MAX_BACKOFF_MS";
const DEFAULT_INTERVAL_MS: u64 = 5_000;
const DEFAULT_MAX_BACKOFF_MS: u64 = 60_000;
const MAX_TENANTS_PER_SWEEP: usize = 32;

#[derive(Clone)]
pub struct StoreBackedIssueAuthorizer {
    identity: StoreBackedCheck,
}

impl StoreBackedIssueAuthorizer {
    pub fn new(identity: StoreBackedCheck) -> Self {
        Self { identity }
    }

    fn allows(&self, principal: &Principal, permission: &str, object: String) -> bool {
        matches!(
            self.identity.check(
                principal,
                &Permission(permission.into()),
                &ArtifactRef(object),
                &Consistency {
                    at_least: Zookie(String::new()),
                    mode: ConsistencyMode::Strong,
                },
                None,
            ),
            Ok(Decision::Allow)
        )
    }
}

impl IssueAuthorizer for StoreBackedIssueAuthorizer {
    fn may_create(&self, principal: &Principal, project_id: &str) -> bool {
        self.allows(principal, "view", format!("project:{project_id}"))
    }

    fn may_view_project(&self, principal: &Principal, project_id: &str) -> bool {
        self.allows(principal, "view", format!("project:{project_id}"))
    }

    fn may_access(
        &self,
        principal: &Principal,
        issue_id: &str,
        permission: IssuePermission,
    ) -> bool {
        let permission = match permission {
            IssuePermission::View => "view",
            IssuePermission::Close => "transition",
        };
        self.allows(principal, permission, format!("issue:{issue_id}"))
    }

    fn visible_issues(&self, _principal: &Principal) -> Result<VisibleIssues, String> {
        Ok(VisibleIssues::effective_issue_view_filter())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssueReconciliationConfig {
    tenants: Vec<TenantId>,
    region: Region,
    batch_limit: u32,
    interval: Duration,
    max_backoff: Duration,
}

impl IssueReconciliationConfig {
    pub fn new(
        tenants: Vec<TenantId>,
        region: Region,
        batch_limit: u32,
        interval: Duration,
        max_backoff: Duration,
    ) -> Result<Self, String> {
        if tenants.is_empty() {
            return Err("at least one Issues reconciliation tenant is required".into());
        }
        let mut unique = BTreeSet::new();
        for tenant in &tenants {
            let token = tenant.as_str();
            if token.is_empty()
                || token.len() > 128
                || !token
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
            {
                return Err(
                    "Issues reconciliation tenants must be opaque 1..=128 byte tokens".into(),
                );
            }
            if !unique.insert(token.to_string()) {
                return Err("Issues reconciliation tenant list contains a duplicate".into());
            }
        }
        if region.as_str().is_empty() {
            return Err("Issues reconciliation region must not be empty".into());
        }
        if batch_limit == 0 || batch_limit > MAX_PAGE_SIZE {
            return Err(format!(
                "Issues reconciliation batch must be between 1 and {MAX_PAGE_SIZE}"
            ));
        }
        if interval.is_zero() {
            return Err("Issues reconciliation interval must be non-zero".into());
        }
        if max_backoff < interval {
            return Err("Issues reconciliation max backoff must be at least the interval".into());
        }
        Ok(Self {
            tenants,
            region,
            batch_limit,
            interval,
            max_backoff,
        })
    }

    pub fn from_env(region: Region) -> Result<Self, String> {
        let raw = std::env::var(ISSUE_RECONCILE_TENANTS_ENV).map_err(|_| {
            format!(
                "{ISSUE_RECONCILE_TENANTS_ENV} is required (comma-separated opaque tenant tokens)"
            )
        })?;
        let tenants = raw
            .split(',')
            .map(str::trim)
            .map(|token| TenantId::from_token(token.to_string()))
            .collect();
        let batch_limit = parse_env_u32(ISSUE_RECONCILE_BATCH_ENV, MAX_PAGE_SIZE)?;
        let interval_ms = parse_env_u64(ISSUE_RECONCILE_INTERVAL_MS_ENV, DEFAULT_INTERVAL_MS)?;
        let max_backoff_ms =
            parse_env_u64(ISSUE_RECONCILE_MAX_BACKOFF_MS_ENV, DEFAULT_MAX_BACKOFF_MS)?;
        Self::new(
            tenants,
            region,
            batch_limit,
            Duration::from_millis(interval_ms),
            Duration::from_millis(max_backoff_ms),
        )
    }

    pub fn tenants(&self) -> &[TenantId] {
        &self.tenants
    }
}

fn parse_env_u32(name: &str, default: u32) -> Result<u32, String> {
    std::env::var(name)
        .ok()
        .map(|raw| {
            raw.parse::<u32>()
                .map_err(|_| format!("{name} must be an unsigned integer"))
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn parse_env_u64(name: &str, default: u64) -> Result<u64, String> {
    std::env::var(name)
        .ok()
        .map(|raw| {
            raw.parse::<u64>()
                .map_err(|_| format!("{name} must be an unsigned integer"))
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IssueReconciliationReport {
    pub pending_seen: u64,
    pub newly_activated: u64,
    pub already_active: u64,
    pub failures: u64,
    pub projections_rebuilt: u64,
    pub projected_grants: u64,
    pub projection_failures: u64,
    pub max_projection_lag: u64,
}

trait IssueAuthorizationSweeper: Send + Sync + 'static {
    fn sweep<'a>(&'a self) -> Pin<Box<dyn Future<Output = IssueReconciliationReport> + Send + 'a>>;
}

struct PgIssueAuthorizationSweeper {
    store: Arc<PgIssueStore<StoreBackedIssueAuthorizer>>,
    identity: StoreBackedCheck,
    workers: Vec<Principal>,
    batch_limit: u32,
    tenant_cursor: AtomicU64,
}

impl IssueAuthorizationSweeper for PgIssueAuthorizationSweeper {
    fn sweep<'a>(&'a self) -> Pin<Box<dyn Future<Output = IssueReconciliationReport> + Send + 'a>> {
        Box::pin(async move {
            let mut report = IssueReconciliationReport::default();
            let tenant_count = self.workers.len();
            let partitions = tenant_count.min(MAX_TENANTS_PER_SWEEP);
            let start = self
                .tenant_cursor
                .fetch_add(partitions as u64, Ordering::Relaxed) as usize
                % tenant_count;
            for offset in 0..partitions {
                let worker = &self.workers[(start + offset) % tenant_count];
                match reconcile_pending_issue_authorizations(
                    &self.store,
                    &self.identity,
                    worker,
                    self.batch_limit,
                )
                .await
                {
                    Ok(outcomes) => {
                        report.pending_seen += outcomes.len() as u64;
                        for (_, outcome) in outcomes {
                            match outcome {
                                Ok(outcome) if outcome.newly_activated => {
                                    report.newly_activated += 1;
                                }
                                Ok(_) => report.already_active += 1,
                                Err(_) => report.failures += 1,
                            }
                        }
                    }
                    Err(_) => report.failures += 1,
                }
                match self.store.effective_issue_view_lag(worker).await {
                    Ok(Some(0)) => {}
                    Ok(lag) => {
                        report.max_projection_lag = report
                            .max_projection_lag
                            .max(lag.unwrap_or(1).unsigned_abs());
                        match self.store.rebuild_effective_issue_view(worker).await {
                            Ok(rebuilt) => {
                                report.projections_rebuilt += 1;
                                report.projected_grants += rebuilt.effective_grants;
                            }
                            Err(_) => {
                                report.projection_failures += 1;
                                report.failures += 1;
                            }
                        }
                    }
                    Err(_) => {
                        report.projection_failures += 1;
                        report.failures += 1;
                    }
                }
            }
            report
        })
    }
}

#[derive(Default)]
pub struct IssueReconciliationMetrics {
    sweeps: AtomicU64,
    pending_seen: AtomicU64,
    newly_activated: AtomicU64,
    already_active: AtomicU64,
    failures: AtomicU64,
    projections_rebuilt: AtomicU64,
    projected_grants: AtomicU64,
    projection_failures: AtomicU64,
    max_projection_lag: AtomicU64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IssueReconciliationMetricsSnapshot {
    pub sweeps: u64,
    pub pending_seen: u64,
    pub newly_activated: u64,
    pub already_active: u64,
    pub failures: u64,
    pub projections_rebuilt: u64,
    pub projected_grants: u64,
    pub projection_failures: u64,
    pub max_projection_lag: u64,
}

impl IssueReconciliationMetrics {
    pub fn snapshot(&self) -> IssueReconciliationMetricsSnapshot {
        IssueReconciliationMetricsSnapshot {
            sweeps: self.sweeps.load(Ordering::Relaxed),
            pending_seen: self.pending_seen.load(Ordering::Relaxed),
            newly_activated: self.newly_activated.load(Ordering::Relaxed),
            already_active: self.already_active.load(Ordering::Relaxed),
            failures: self.failures.load(Ordering::Relaxed),
            projections_rebuilt: self.projections_rebuilt.load(Ordering::Relaxed),
            projected_grants: self.projected_grants.load(Ordering::Relaxed),
            projection_failures: self.projection_failures.load(Ordering::Relaxed),
            max_projection_lag: self.max_projection_lag.load(Ordering::Relaxed),
        }
    }

    fn record(&self, report: IssueReconciliationReport) {
        self.sweeps.fetch_add(1, Ordering::Relaxed);
        self.pending_seen
            .fetch_add(report.pending_seen, Ordering::Relaxed);
        self.newly_activated
            .fetch_add(report.newly_activated, Ordering::Relaxed);
        self.already_active
            .fetch_add(report.already_active, Ordering::Relaxed);
        self.failures.fetch_add(report.failures, Ordering::Relaxed);
        self.projections_rebuilt
            .fetch_add(report.projections_rebuilt, Ordering::Relaxed);
        self.projected_grants
            .fetch_add(report.projected_grants, Ordering::Relaxed);
        self.projection_failures
            .fetch_add(report.projection_failures, Ordering::Relaxed);
        self.max_projection_lag
            .fetch_max(report.max_projection_lag, Ordering::Relaxed);
    }
}

pub struct IssueReconciliationHandle {
    shutdown: watch::Sender<bool>,
    join: JoinHandle<()>,
    metrics: Arc<IssueReconciliationMetrics>,
}

impl IssueReconciliationHandle {
    pub fn metrics(&self) -> &Arc<IssueReconciliationMetrics> {
        &self.metrics
    }

    pub async fn shutdown(self) -> Result<(), String> {
        let _ = self.shutdown.send(true);
        match tokio::time::timeout(Duration::from_secs(10), self.join).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) => Err("Issues reconciliation task join failed".into()),
            Err(_) => Err("Issues reconciliation task did not stop within 10 seconds".into()),
        }
    }
}

pub fn spawn_issue_authorization_reconciler(
    store: Arc<PgIssueStore<StoreBackedIssueAuthorizer>>,
    identity: StoreBackedCheck,
    config: IssueReconciliationConfig,
) -> IssueReconciliationHandle {
    let workers = config
        .tenants
        .iter()
        .map(|tenant| {
            Principal::new(
                tenant.clone(),
                config.region.clone(),
                PrincipalId("service:issues-authz-reconciler".into()),
                PrincipalKind::Service,
                DataRole::Processor,
                PrincipalStatus::Active,
            )
        })
        .collect();
    let sweeper: Arc<dyn IssueAuthorizationSweeper> = Arc::new(PgIssueAuthorizationSweeper {
        store,
        identity,
        workers,
        batch_limit: config.batch_limit,
        tenant_cursor: AtomicU64::new(0),
    });
    spawn_reconciliation_loop(sweeper, config)
}

fn spawn_reconciliation_loop(
    sweeper: Arc<dyn IssueAuthorizationSweeper>,
    config: IssueReconciliationConfig,
) -> IssueReconciliationHandle {
    let (shutdown, receiver) = watch::channel(false);
    let metrics = Arc::new(IssueReconciliationMetrics::default());
    let metrics_for_task = metrics.clone();
    let join = tokio::spawn(run_reconciliation_loop(
        sweeper,
        config,
        receiver,
        metrics_for_task,
    ));
    IssueReconciliationHandle {
        shutdown,
        join,
        metrics,
    }
}

async fn run_reconciliation_loop(
    sweeper: Arc<dyn IssueAuthorizationSweeper>,
    config: IssueReconciliationConfig,
    mut shutdown: watch::Receiver<bool>,
    metrics: Arc<IssueReconciliationMetrics>,
) {
    let mut delay = config.interval;
    loop {
        if *shutdown.borrow() {
            break;
        }
        let report = sweeper.sweep().await;
        metrics.record(report);
        if report.pending_seen > 0 || report.failures > 0 {
            eprintln!(
                "issues authz reconciler: pending_seen={} newly_activated={} already_active={} failures={} projections_rebuilt={} projection_failures={} max_projection_lag={} region={}",
                report.pending_seen,
                report.newly_activated,
                report.already_active,
                report.failures,
                report.projections_rebuilt,
                report.projection_failures,
                report.max_projection_lag,
                config.region.as_str(),
            );
        }
        delay = next_reconciliation_delay(
            delay,
            config.interval,
            config.max_backoff,
            report.failures > 0,
        );

        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
        }
    }
}

fn next_reconciliation_delay(
    current: Duration,
    interval: Duration,
    max_backoff: Duration,
    failed: bool,
) -> Duration {
    if !failed {
        interval
    } else {
        current
            .checked_mul(2)
            .unwrap_or(max_backoff)
            .min(max_backoff)
    }
}

#[derive(Clone)]
pub struct IdentityIssueTupleWriter {
    tuples: TupleStore,
}

impl IdentityIssueTupleWriter {
    pub fn new(tuples: TupleStore) -> Self {
        Self { tuples }
    }

    pub fn from_identity(identity: &StoreBackedCheck) -> Self {
        Self::new(identity.tuples().clone())
    }
}

pub async fn reconcile_pending_issue_authorizations<A: IssueAuthorizer>(
    store: &PgIssueStore<A>,
    identity: &StoreBackedCheck,
    worker: &Principal,
    limit: u32,
) -> Result<Vec<(String, Result<IssueAuthorizationOutcome, IssueStoreError>)>, IssueStoreError> {
    let writer = IdentityIssueTupleWriter::from_identity(identity);
    let pending = store.pending_authorization_ids(worker, limit).await?;
    let mut outcomes = Vec::with_capacity(pending.len());
    for issue_id in pending {
        let outcome = store
            .reconcile_authorization(worker, &issue_id, &writer)
            .await;
        outcomes.push((issue_id, outcome));
    }
    Ok(outcomes)
}

impl IssueTupleWriter for IdentityIssueTupleWriter {
    fn ensure_parent_project<'a>(
        &'a self,
        scope: &'a TenantScope,
        actor: &'a Principal,
        binding: &'a IssueAuthorizationBinding,
    ) -> Pin<Box<dyn Future<Output = Result<Zookie, String>> + Send + 'a>> {
        let tuples = self.tuples.clone();
        let scope = scope.clone();
        let actor = actor.clone();
        let delta = parent_project_delta(binding);
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                tuples.write_tuples(&scope, &actor, &[delta], None, None, now_timestamp())
            })
            .await
            .map_err(|_| "identity_tuple_worker_join_failed".to_string())?
            .map_err(|_| "identity_tuple_write_failed".to_string())
        })
    }
}

fn parent_project_delta(binding: &IssueAuthorizationBinding) -> TupleDelta {
    TupleDelta::Add(RelationTuple {
        object: ObjectId(binding.issue_object.clone()),
        relation: RelName(binding.relation.clone()),
        subject: PrincipalId(binding.project_userset.clone()),
        caveat: None,
    })
}

fn now_timestamp() -> Timestamp {
    let now = chrono::DateTime::<chrono::Utc>::from(std::time::SystemTime::now());
    Timestamp(now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_issues::IssueAuthorizationState;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    #[test]
    fn adapter_writes_the_exact_staged_parent_project_userset() {
        let binding = IssueAuthorizationBinding {
            issue_id: "33333333-3333-3333-3333-333333333333".into(),
            project_id: "11111111-1111-1111-1111-111111111111".into(),
            issue_object: "issue:33333333-3333-3333-3333-333333333333".into(),
            project_userset: "project:11111111-1111-1111-1111-111111111111#view".into(),
            relation: "parent_project".into(),
            request_event_id: "01J00000000000000000000000".into(),
            created_event_id: "01J00000000000000000000001".into(),
            state: IssueAuthorizationState::Pending,
            zookie: None,
            attempts: 0,
        };
        assert_eq!(
            parent_project_delta(&binding),
            TupleDelta::Add(RelationTuple {
                object: ObjectId(binding.issue_object),
                relation: RelName("parent_project".into()),
                subject: PrincipalId(binding.project_userset),
                caveat: None,
            })
        );
    }

    #[test]
    fn reconciliation_config_is_explicit_bounded_and_partitioned() {
        let config = IssueReconciliationConfig::new(
            vec![TenantId::from_token("01J0TENANT_A")],
            Region::new("fr-par"),
            100,
            Duration::from_secs(5),
            Duration::from_secs(60),
        )
        .expect("valid worker config");
        assert_eq!(config.tenants()[0].as_str(), "01J0TENANT_A");
        assert!(IssueReconciliationConfig::new(
            Vec::new(),
            Region::new("fr-par"),
            100,
            Duration::from_secs(5),
            Duration::from_secs(60),
        )
        .is_err());
        assert!(IssueReconciliationConfig::new(
            vec![
                TenantId::from_token("duplicate"),
                TenantId::from_token("duplicate")
            ],
            Region::new("fr-par"),
            100,
            Duration::from_secs(5),
            Duration::from_secs(60),
        )
        .is_err());
        assert!(IssueReconciliationConfig::new(
            vec![TenantId::from_token("tenant@example.test")],
            Region::new("fr-par"),
            100,
            Duration::from_secs(5),
            Duration::from_secs(60),
        )
        .is_err());
        assert!(IssueReconciliationConfig::new(
            vec![TenantId::from_token("01J0TENANT_A")],
            Region::new("fr-par"),
            MAX_PAGE_SIZE + 1,
            Duration::from_secs(5),
            Duration::from_secs(60),
        )
        .is_err());
    }

    #[test]
    fn reconciliation_backoff_is_exponential_bounded_and_resets() {
        let normal = Duration::from_secs(5);
        let max = Duration::from_secs(60);
        let first = next_reconciliation_delay(normal, normal, max, true);
        let second = next_reconciliation_delay(first, normal, max, true);
        let capped = next_reconciliation_delay(Duration::from_secs(40), normal, max, true);
        assert_eq!(first, Duration::from_secs(10));
        assert_eq!(second, Duration::from_secs(20));
        assert_eq!(capped, max);
        assert_eq!(
            next_reconciliation_delay(capped, normal, max, false),
            normal
        );
    }

    struct FakeSweeper {
        reports: Mutex<VecDeque<IssueReconciliationReport>>,
        calls: AtomicU64,
    }

    impl FakeSweeper {
        fn new(reports: impl IntoIterator<Item = IssueReconciliationReport>) -> Self {
            Self {
                reports: Mutex::new(reports.into_iter().collect()),
                calls: AtomicU64::new(0),
            }
        }
    }

    impl IssueAuthorizationSweeper for FakeSweeper {
        fn sweep<'a>(
            &'a self,
        ) -> Pin<Box<dyn Future<Output = IssueReconciliationReport> + Send + 'a>> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                self.reports
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .pop_front()
                    .unwrap_or_default()
            })
        }
    }

    fn test_config() -> IssueReconciliationConfig {
        IssueReconciliationConfig::new(
            vec![TenantId::from_token("01J0TENANT_A")],
            Region::new("fr-par"),
            10,
            Duration::from_secs(60),
            Duration::from_secs(120),
        )
        .unwrap()
    }

    async fn wait_for_sweeps(metrics: &IssueReconciliationMetrics, expected: u64) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while metrics.snapshot().sweeps < expected {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("immediate boot sweep completes");
    }

    #[tokio::test]
    async fn worker_sweeps_immediately_and_shuts_down_gracefully() {
        let sweeper = Arc::new(FakeSweeper::new([IssueReconciliationReport {
            pending_seen: 2,
            newly_activated: 1,
            already_active: 1,
            failures: 0,
            ..IssueReconciliationReport::default()
        }]));
        let handle = spawn_reconciliation_loop(sweeper.clone(), test_config());
        wait_for_sweeps(handle.metrics(), 1).await;
        assert_eq!(handle.metrics().snapshot().pending_seen, 2);
        handle.shutdown().await.expect("graceful worker shutdown");
        assert_eq!(sweeper.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn restart_replays_pending_work_without_a_second_activation() {
        let sweeper = Arc::new(FakeSweeper::new([
            IssueReconciliationReport {
                pending_seen: 1,
                newly_activated: 1,
                already_active: 0,
                failures: 0,
                ..IssueReconciliationReport::default()
            },
            IssueReconciliationReport::default(),
        ]));

        let first = spawn_reconciliation_loop(sweeper.clone(), test_config());
        wait_for_sweeps(first.metrics(), 1).await;
        assert_eq!(first.metrics().snapshot().newly_activated, 1);
        first.shutdown().await.unwrap();

        let restarted = spawn_reconciliation_loop(sweeper.clone(), test_config());
        wait_for_sweeps(restarted.metrics(), 1).await;
        assert_eq!(restarted.metrics().snapshot().newly_activated, 0);
        restarted.shutdown().await.unwrap();
        assert_eq!(sweeper.calls.load(Ordering::SeqCst), 2);
    }
}
