//! Coordinated lifecycle owner for the production CI starter, Flow recovery, and sandbox runner.
//!
//! One signal stops all three intake paths. The async starter and workflow lanes are joined with the
//! dedicated runner thread; an already-launched sandbox is allowed to finish under its persisted
//! per-job timeout. Any lane failure stops its peers and is surfaced to the service lifecycle.

use std::future::Future;
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::{
    CiProductionWorkflowPoller, CiRunStarterPollerError, CiRunnerLoop, CiRunnerLoopExit,
    PgCiRunStarterPoller, MAX_CI_RUN_START_BATCH, MAX_CI_WORKFLOW_DRIVES_PER_SCOPE,
    MAX_CI_WORKFLOW_SCOPES_PER_PASS, MAX_JOB_TIMEOUT_SECS,
};

/// Production starter/recovery cadence. Each pass is additionally bounded by the public batch
/// ceilings below.
pub const CI_RUNNER_HOST_POLL_INTERVAL: Duration = Duration::from_millis(250);
/// Process-fatal graceful-drain deadline: the maximum admitted sandbox wall clock plus one minute.
/// Crossing it fails the service immediately, but the supervisor retains and awaits every task/thread
/// owner until the process is terminated or the work actually returns.
pub const CI_RUNNER_HOST_DRAIN_TIMEOUT: Duration =
    Duration::from_secs(MAX_JOB_TIMEOUT_SECS as u64 + 60);

/// Validated lifecycle bounds for one runner host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CiRunnerHostConfig {
    starter_poll_interval: Duration,
    starter_batch: usize,
    workflow_poll_interval: Duration,
    workflow_scopes: usize,
    workflow_drives_per_scope: usize,
    drain_timeout: Duration,
}

impl CiRunnerHostConfig {
    /// Production bounds: one complete bounded page per 250 ms tick, with an explicit process-fatal
    /// maximum-job drain deadline.
    pub const fn production() -> Self {
        Self {
            starter_poll_interval: CI_RUNNER_HOST_POLL_INTERVAL,
            starter_batch: MAX_CI_RUN_START_BATCH,
            workflow_poll_interval: CI_RUNNER_HOST_POLL_INTERVAL,
            workflow_scopes: MAX_CI_WORKFLOW_SCOPES_PER_PASS,
            workflow_drives_per_scope: MAX_CI_WORKFLOW_DRIVES_PER_SCOPE,
            drain_timeout: CI_RUNNER_HOST_DRAIN_TIMEOUT,
        }
    }

    #[cfg(test)]
    fn for_test(drain_timeout: Duration) -> Self {
        Self {
            starter_poll_interval: Duration::from_millis(1),
            starter_batch: 1,
            workflow_poll_interval: Duration::from_millis(1),
            workflow_scopes: 1,
            workflow_drives_per_scope: 1,
            drain_timeout,
        }
    }

    fn validate(self) -> Result<Self, CiRunnerHostFailure> {
        if self.starter_poll_interval.is_zero()
            || !(1..=MAX_CI_RUN_START_BATCH).contains(&self.starter_batch)
            || self.workflow_poll_interval.is_zero()
            || !(1..=MAX_CI_WORKFLOW_SCOPES_PER_PASS).contains(&self.workflow_scopes)
            || !(1..=MAX_CI_WORKFLOW_DRIVES_PER_SCOPE).contains(&self.workflow_drives_per_scope)
            || self.drain_timeout.is_zero()
        {
            return Err(CiRunnerHostFailure::InvalidConfig);
        }
        Ok(self)
    }
}

impl Default for CiRunnerHostConfig {
    fn default() -> Self {
        Self::production()
    }
}

/// Credential-free runner-host failure. Values are safe to emit at the composition root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CiRunnerHostFailure {
    InvalidConfig,
    RunnerThreadSpawn,
    Starter,
    Workflow,
    SettlementOwnerMismatch,
    TerminalReportFailed,
    StarterTaskPanicked,
    WorkflowTaskPanicked,
    RunnerThreadPanicked,
    RunnerJoinTaskPanicked,
    StarterExitedEarly,
    WorkflowExitedEarly,
    RunnerExitedEarly,
    DrainTimedOut,
}

impl std::fmt::Display for CiRunnerHostFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::InvalidConfig => "runner-host lifecycle bounds are invalid",
            Self::RunnerThreadSpawn => "sandbox runner thread could not be spawned",
            Self::Starter => "queued-run starter lane failed",
            Self::Workflow => "Flow recovery lane failed",
            Self::SettlementOwnerMismatch => {
                "sandbox runner refused inconsistent terminal-settlement ownership"
            }
            Self::TerminalReportFailed => {
                "sandbox runner could not commit terminal accounting/reporting"
            }
            Self::StarterTaskPanicked => "queued-run starter task panicked",
            Self::WorkflowTaskPanicked => "Flow recovery task panicked",
            Self::RunnerThreadPanicked => "sandbox runner thread panicked",
            Self::RunnerJoinTaskPanicked => "sandbox runner join task panicked",
            Self::StarterExitedEarly => "queued-run starter exited before shutdown",
            Self::WorkflowExitedEarly => "Flow recovery lane exited before shutdown",
            Self::RunnerExitedEarly => "sandbox runner exited before shutdown",
            Self::DrainTimedOut => "runner host exceeded its process-fatal graceful-drain deadline",
        };
        f.write_str(message)
    }
}

impl std::error::Error for CiRunnerHostFailure {}

/// Complete, not-yet-started production runner host.
pub struct CiRunnerHost {
    starter: PgCiRunStarterPoller,
    workflow: CiProductionWorkflowPoller,
    runner: CiRunnerLoop,
}

impl CiRunnerHost {
    pub fn new(
        starter: PgCiRunStarterPoller,
        workflow: CiProductionWorkflowPoller,
        runner: CiRunnerLoop,
    ) -> Self {
        Self {
            starter,
            workflow,
            runner,
        }
    }

    /// Start every lane behind one owned lifecycle signal. The runner thread is created before the
    /// Tokio tasks so thread-resource failure leaves no detached partial host.
    pub fn start(
        self,
        config: CiRunnerHostConfig,
    ) -> Result<CiRunnerHostHandle, CiRunnerHostFailure> {
        let (shutdown, _receiver) = watch::channel(false);
        self.start_with_shutdown(config, shutdown)
    }

    /// Start every lane behind a lifecycle signal that was armed before host construction.
    ///
    /// Production passes the process-wide shutdown sender installed before bootstrap. A signal
    /// received while migrations or composition are still running is therefore already visible to
    /// every lane at its first instruction; no queued work can be admitted in the gap between
    /// bootstrap and host supervision.
    pub fn start_with_shutdown(
        self,
        config: CiRunnerHostConfig,
        shutdown: watch::Sender<bool>,
    ) -> Result<CiRunnerHostHandle, CiRunnerHostFailure> {
        let config = config.validate()?;
        let Self {
            starter,
            workflow,
            runner,
        } = self;
        start_lanes_with_shutdown(
            config,
            shutdown,
            move |shutdown| async move {
                starter
                    .run_until_shutdown(
                        shutdown,
                        config.starter_poll_interval,
                        config.starter_batch,
                    )
                    .await
                    .map_err(map_starter_error)
            },
            move |shutdown| async move {
                workflow
                    .run_until_shutdown(
                        shutdown,
                        config.workflow_poll_interval,
                        config.workflow_scopes,
                        config.workflow_drives_per_scope,
                    )
                    .await
                    .map_err(|_| CiRunnerHostFailure::Workflow)
            },
            move |shutdown| runner.try_spawn_until_shutdown(shutdown),
        )
    }
}

fn map_starter_error(_error: CiRunStarterPollerError) -> CiRunnerHostFailure {
    CiRunnerHostFailure::Starter
}

/// Owned running host. A failure receiver lets the service stop intake immediately if any lane
/// fails; `shutdown` always joins the supervisor's bounded drain before returning.
pub struct CiRunnerHostHandle {
    shutdown: watch::Sender<bool>,
    failures: watch::Receiver<Option<CiRunnerHostFailure>>,
    supervisor: JoinHandle<Result<(), CiRunnerHostFailure>>,
}

impl CiRunnerHostHandle {
    pub fn shutdown_sender(&self) -> watch::Sender<bool> {
        self.shutdown.clone()
    }

    pub fn failure_receiver(&self) -> watch::Receiver<Option<CiRunnerHostFailure>> {
        self.failures.clone()
    }

    pub async fn shutdown(self) -> Result<(), CiRunnerHostFailure> {
        let _ = self.shutdown.send(true);
        match self.supervisor.await {
            Ok(result) => result,
            Err(_) => Err(CiRunnerHostFailure::RunnerJoinTaskPanicked),
        }
    }
}

/// Wait for a host failure. Closing the supervisor without publishing one is itself a failure:
/// production must never silently continue serving after losing its runner host.
pub async fn wait_for_ci_runner_host_failure(
    mut failures: watch::Receiver<Option<CiRunnerHostFailure>>,
) -> CiRunnerHostFailure {
    loop {
        if let Some(failure) = *failures.borrow_and_update() {
            return failure;
        }
        if failures.changed().await.is_err() {
            return CiRunnerHostFailure::RunnerJoinTaskPanicked;
        }
    }
}

/// Retained production watchdog for the process-fatal drain deadline. Earlier lane failures are
/// deliberately ignored here because the service lifecycle observes them; this receiver stays live
/// after that first observer resolves so a later stuck-drain deadline cannot become an infinite hang.
/// `true` means the deadline fired; `false` means the host supervisor ended without one.
pub async fn wait_for_ci_runner_host_drain_timeout(
    mut failures: watch::Receiver<Option<CiRunnerHostFailure>>,
) -> bool {
    loop {
        if *failures.borrow_and_update() == Some(CiRunnerHostFailure::DrainTimedOut) {
            return true;
        }
        if failures.changed().await.is_err() {
            return false;
        }
    }
}

#[cfg(test)]
fn start_lanes<S, SF, W, WF, R>(
    config: CiRunnerHostConfig,
    starter: S,
    workflow: W,
    runner: R,
) -> Result<CiRunnerHostHandle, CiRunnerHostFailure>
where
    S: FnOnce(watch::Receiver<bool>) -> SF + Send + 'static,
    SF: Future<Output = Result<(), CiRunnerHostFailure>> + Send + 'static,
    W: FnOnce(watch::Receiver<bool>) -> WF + Send + 'static,
    WF: Future<Output = Result<(), CiRunnerHostFailure>> + Send + 'static,
    R: FnOnce(watch::Receiver<bool>) -> std::io::Result<std::thread::JoinHandle<CiRunnerLoopExit>>
        + Send
        + 'static,
{
    let (shutdown, _receiver) = watch::channel(false);
    start_lanes_with_shutdown(config, shutdown, starter, workflow, runner)
}

fn start_lanes_with_shutdown<S, SF, W, WF, R>(
    config: CiRunnerHostConfig,
    shutdown: watch::Sender<bool>,
    starter: S,
    workflow: W,
    runner: R,
) -> Result<CiRunnerHostHandle, CiRunnerHostFailure>
where
    S: FnOnce(watch::Receiver<bool>) -> SF + Send + 'static,
    SF: Future<Output = Result<(), CiRunnerHostFailure>> + Send + 'static,
    W: FnOnce(watch::Receiver<bool>) -> WF + Send + 'static,
    WF: Future<Output = Result<(), CiRunnerHostFailure>> + Send + 'static,
    R: FnOnce(watch::Receiver<bool>) -> std::io::Result<std::thread::JoinHandle<CiRunnerLoopExit>>
        + Send
        + 'static,
{
    let config = config.validate()?;
    let receiver = shutdown.subscribe();
    let runner_thread =
        runner(receiver.clone()).map_err(|_| CiRunnerHostFailure::RunnerThreadSpawn)?;
    let starter_task = tokio::spawn(starter(receiver.clone()));
    let workflow_task = tokio::spawn(workflow(receiver.clone()));
    let runner_task = tokio::task::spawn_blocking(move || runner_thread.join());
    let (failure_tx, failures) = watch::channel(None);
    let supervisor_shutdown = shutdown.clone();
    let supervisor = tokio::spawn(supervise_lanes(
        config,
        supervisor_shutdown,
        receiver,
        failure_tx,
        starter_task,
        workflow_task,
        runner_task,
    ));
    Ok(CiRunnerHostHandle {
        shutdown,
        failures,
        supervisor,
    })
}

type AsyncLaneResult = Result<Result<(), CiRunnerHostFailure>, tokio::task::JoinError>;
type RunnerLaneResult = Result<std::thread::Result<CiRunnerLoopExit>, tokio::task::JoinError>;

async fn supervise_lanes(
    config: CiRunnerHostConfig,
    shutdown_tx: watch::Sender<bool>,
    mut shutdown_rx: watch::Receiver<bool>,
    failure_tx: watch::Sender<Option<CiRunnerHostFailure>>,
    mut starter: JoinHandle<Result<(), CiRunnerHostFailure>>,
    mut workflow: JoinHandle<Result<(), CiRunnerHostFailure>>,
    mut runner: JoinHandle<std::thread::Result<CiRunnerLoopExit>>,
) -> Result<(), CiRunnerHostFailure> {
    enum First {
        Shutdown,
        Starter(AsyncLaneResult),
        Workflow(AsyncLaneResult),
        Runner(RunnerLaneResult),
    }

    let first = tokio::select! {
        biased;
        changed = shutdown_rx.changed() => {
            let _ = changed;
            First::Shutdown
        }
        result = &mut starter => First::Starter(result),
        result = &mut workflow => First::Workflow(result),
        result = &mut runner => First::Runner(result),
    };
    let shutdown_was_requested = *shutdown_rx.borrow();
    let (starter_done, workflow_done, runner_done, mut failure) = match first {
        First::Shutdown => (false, false, false, None),
        First::Starter(result) => (
            true,
            false,
            false,
            classify_async_lane(
                result,
                shutdown_was_requested,
                CiRunnerHostFailure::StarterTaskPanicked,
                CiRunnerHostFailure::StarterExitedEarly,
            ),
        ),
        First::Workflow(result) => (
            false,
            true,
            false,
            classify_async_lane(
                result,
                shutdown_was_requested,
                CiRunnerHostFailure::WorkflowTaskPanicked,
                CiRunnerHostFailure::WorkflowExitedEarly,
            ),
        ),
        First::Runner(result) => (
            false,
            false,
            true,
            classify_runner_lane(result, shutdown_was_requested),
        ),
    };
    if let Some(reason) = failure {
        let _ = failure_tx.send(Some(reason));
    }
    let _ = shutdown_tx.send(true);

    let drain = async {
        let starter_result = async {
            if starter_done {
                None
            } else {
                Some(starter.await)
            }
        };
        let workflow_result = async {
            if workflow_done {
                None
            } else {
                Some(workflow.await)
            }
        };
        let runner_result = async {
            if runner_done {
                None
            } else {
                Some(runner.await)
            }
        };
        tokio::join!(starter_result, workflow_result, runner_result)
    };
    tokio::pin!(drain);
    let mut drain_timed_out = false;
    let (starter_result, workflow_result, runner_result) = tokio::select! {
        results = &mut drain => results,
        _ = tokio::time::sleep(config.drain_timeout) => {
            drain_timed_out = true;
            let _ = failure_tx.send(Some(CiRunnerHostFailure::DrainTimedOut));
            drain.await
        }
    };

    if failure.is_none() {
        failure = starter_result.and_then(|result| {
            classify_async_lane(
                result,
                true,
                CiRunnerHostFailure::StarterTaskPanicked,
                CiRunnerHostFailure::StarterExitedEarly,
            )
        });
    }
    if failure.is_none() {
        failure = workflow_result.and_then(|result| {
            classify_async_lane(
                result,
                true,
                CiRunnerHostFailure::WorkflowTaskPanicked,
                CiRunnerHostFailure::WorkflowExitedEarly,
            )
        });
    }
    if failure.is_none() {
        failure = runner_result.and_then(|result| classify_runner_lane(result, true));
    }
    if drain_timed_out {
        Err(CiRunnerHostFailure::DrainTimedOut)
    } else if let Some(reason) = failure {
        let _ = failure_tx.send(Some(reason));
        Err(reason)
    } else {
        Ok(())
    }
}

fn classify_async_lane(
    result: AsyncLaneResult,
    shutdown_requested: bool,
    panicked: CiRunnerHostFailure,
    exited_early: CiRunnerHostFailure,
) -> Option<CiRunnerHostFailure> {
    match result {
        Ok(Ok(())) if shutdown_requested => None,
        Ok(Ok(())) => Some(exited_early),
        Ok(Err(failure)) => Some(failure),
        Err(_) => Some(panicked),
    }
}

fn classify_runner_lane(
    result: RunnerLaneResult,
    shutdown_requested: bool,
) -> Option<CiRunnerHostFailure> {
    match result {
        Ok(Ok(CiRunnerLoopExit::Shutdown)) if shutdown_requested => None,
        Ok(Ok(CiRunnerLoopExit::Shutdown)) => Some(CiRunnerHostFailure::RunnerExitedEarly),
        Ok(Ok(CiRunnerLoopExit::SettlementOwnerMismatch)) => {
            Some(CiRunnerHostFailure::SettlementOwnerMismatch)
        }
        Ok(Ok(CiRunnerLoopExit::TerminalReportFailed)) => {
            Some(CiRunnerHostFailure::TerminalReportFailed)
        }
        Ok(Err(_)) => Some(CiRunnerHostFailure::RunnerThreadPanicked),
        Err(_) => Some(CiRunnerHostFailure::RunnerJoinTaskPanicked),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

    use super::*;

    async fn wait_for_shutdown(mut shutdown: watch::Receiver<bool>) {
        while !*shutdown.borrow() {
            if shutdown.changed().await.is_err() {
                return;
            }
        }
    }

    fn runner_waiting_for_shutdown(
        mut shutdown: watch::Receiver<bool>,
    ) -> std::io::Result<std::thread::JoinHandle<CiRunnerLoopExit>> {
        std::thread::Builder::new()
            .name("ci-runner-host-test".into())
            .spawn(move || {
                loop {
                    if *shutdown.borrow_and_update() {
                        break;
                    }
                    if shutdown.has_changed().is_err() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                CiRunnerLoopExit::Shutdown
            })
    }

    #[tokio::test]
    async fn one_shutdown_signal_joins_every_lane() {
        let exits = Arc::new(AtomicUsize::new(0));
        let starter_exits = exits.clone();
        let workflow_exits = exits.clone();
        let handle = start_lanes(
            CiRunnerHostConfig::for_test(Duration::from_secs(1)),
            move |shutdown| async move {
                wait_for_shutdown(shutdown).await;
                starter_exits.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            move |shutdown| async move {
                wait_for_shutdown(shutdown).await;
                workflow_exits.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            runner_waiting_for_shutdown,
        )
        .unwrap();

        handle.shutdown().await.unwrap();
        assert_eq!(exits.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn shutdown_latched_before_host_start_admits_no_lane_work() {
        let intake = Arc::new(AtomicUsize::new(0));
        let starter_intake = intake.clone();
        let workflow_intake = intake.clone();
        let runner_intake = intake.clone();
        let (shutdown, _receiver) = watch::channel(true);
        let handle = start_lanes_with_shutdown(
            CiRunnerHostConfig::for_test(Duration::from_secs(1)),
            shutdown,
            move |shutdown| async move {
                if !*shutdown.borrow() {
                    starter_intake.fetch_add(1, Ordering::SeqCst);
                }
                Ok(())
            },
            move |shutdown| async move {
                if !*shutdown.borrow() {
                    workflow_intake.fetch_add(1, Ordering::SeqCst);
                }
                Ok(())
            },
            move |mut shutdown| {
                std::thread::Builder::new()
                    .name("ci-runner-host-test".into())
                    .spawn(move || {
                        if !*shutdown.borrow_and_update() {
                            runner_intake.fetch_add(1, Ordering::SeqCst);
                        }
                        CiRunnerLoopExit::Shutdown
                    })
            },
        )
        .unwrap();

        handle.shutdown().await.unwrap();
        assert_eq!(intake.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_lane_failure_stops_its_peers_and_surfaces_to_the_service() {
        let runner_stopped = Arc::new(AtomicBool::new(false));
        let runner_stopped_in_thread = runner_stopped.clone();
        let handle = start_lanes(
            CiRunnerHostConfig::for_test(Duration::from_secs(1)),
            |_shutdown| async { Err(CiRunnerHostFailure::Starter) },
            |shutdown| async move {
                wait_for_shutdown(shutdown).await;
                Ok(())
            },
            move |mut shutdown| {
                std::thread::Builder::new()
                    .name("ci-runner-host-test".into())
                    .spawn(move || {
                        while !*shutdown.borrow_and_update() {
                            std::thread::sleep(Duration::from_millis(1));
                        }
                        runner_stopped_in_thread.store(true, Ordering::SeqCst);
                        CiRunnerLoopExit::Shutdown
                    })
            },
        )
        .unwrap();
        let failure = wait_for_ci_runner_host_failure(handle.failure_receiver()).await;
        assert_eq!(failure, CiRunnerHostFailure::Starter);
        assert_eq!(handle.shutdown().await, Err(CiRunnerHostFailure::Starter));
        assert!(runner_stopped.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn static_runner_refusal_stops_async_intake_and_surfaces() {
        let async_lanes_stopped = Arc::new(AtomicUsize::new(0));
        let starter_stopped = async_lanes_stopped.clone();
        let workflow_stopped = async_lanes_stopped.clone();
        let handle = start_lanes(
            CiRunnerHostConfig::for_test(Duration::from_secs(1)),
            move |shutdown| async move {
                wait_for_shutdown(shutdown).await;
                starter_stopped.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            move |shutdown| async move {
                wait_for_shutdown(shutdown).await;
                workflow_stopped.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            |_shutdown| {
                std::thread::Builder::new()
                    .name("ci-runner-host-test".into())
                    .spawn(|| CiRunnerLoopExit::SettlementOwnerMismatch)
            },
        )
        .unwrap();
        let failure = wait_for_ci_runner_host_failure(handle.failure_receiver()).await;
        assert_eq!(failure, CiRunnerHostFailure::SettlementOwnerMismatch);
        assert_eq!(
            handle.shutdown().await,
            Err(CiRunnerHostFailure::SettlementOwnerMismatch)
        );
        assert_eq!(async_lanes_stopped.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn terminal_report_failure_is_a_host_failure_not_a_retrying_lane() {
        assert_eq!(
            classify_runner_lane(Ok(Ok(CiRunnerLoopExit::TerminalReportFailed)), false),
            Some(CiRunnerHostFailure::TerminalReportFailed)
        );
    }

    #[tokio::test]
    async fn shutdown_waits_for_an_inflight_runner_to_drain() {
        let launched = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));
        let launched_in_thread = launched.clone();
        let release_in_thread = release.clone();
        let handle = start_lanes(
            CiRunnerHostConfig::for_test(Duration::from_secs(1)),
            |shutdown| async move {
                wait_for_shutdown(shutdown).await;
                Ok(())
            },
            |shutdown| async move {
                wait_for_shutdown(shutdown).await;
                Ok(())
            },
            move |_shutdown| {
                std::thread::Builder::new()
                    .name("ci-runner-host-test".into())
                    .spawn(move || {
                        launched_in_thread.store(true, Ordering::SeqCst);
                        while !release_in_thread.load(Ordering::SeqCst) {
                            std::thread::sleep(Duration::from_millis(1));
                        }
                        CiRunnerLoopExit::Shutdown
                    })
            },
        )
        .unwrap();
        while !launched.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
        let shutdown = tokio::spawn(handle.shutdown());
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(!shutdown.is_finished());
        release.store(true, Ordering::SeqCst);
        shutdown.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn invalid_config_refuses_before_any_lane_starts() {
        let started = Arc::new(AtomicBool::new(false));
        let starter_started = started.clone();
        let workflow_started = started.clone();
        let runner_started = started.clone();
        let result = start_lanes(
            CiRunnerHostConfig {
                drain_timeout: Duration::ZERO,
                ..CiRunnerHostConfig::for_test(Duration::from_secs(1))
            },
            move |_shutdown| async move {
                starter_started.store(true, Ordering::SeqCst);
                Ok(())
            },
            move |_shutdown| async move {
                workflow_started.store(true, Ordering::SeqCst);
                Ok(())
            },
            move |_shutdown| {
                runner_started.store(true, Ordering::SeqCst);
                unreachable!()
            },
        );
        assert!(matches!(result, Err(CiRunnerHostFailure::InvalidConfig)));
        assert!(!started.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn a_stuck_runner_fails_the_bounded_drain() {
        let handle = start_lanes(
            CiRunnerHostConfig::for_test(Duration::from_millis(5)),
            |shutdown| async move {
                wait_for_shutdown(shutdown).await;
                Ok(())
            },
            |shutdown| async move {
                wait_for_shutdown(shutdown).await;
                Ok(())
            },
            |_shutdown| {
                std::thread::Builder::new()
                    .name("ci-runner-host-test".into())
                    .spawn(|| {
                        std::thread::sleep(Duration::from_millis(50));
                        CiRunnerLoopExit::Shutdown
                    })
            },
        )
        .unwrap();
        let service_failures = handle.failure_receiver();
        let deadline_failures = handle.failure_receiver();
        drop(service_failures);
        let shutdown = tokio::spawn(handle.shutdown());
        assert!(wait_for_ci_runner_host_drain_timeout(deadline_failures).await);
        assert!(
            !shutdown.is_finished(),
            "the retained watchdog must fire while the supervisor still owns the runner join"
        );
        assert_eq!(
            shutdown.await.unwrap(),
            Err(CiRunnerHostFailure::DrainTimedOut)
        );
    }
}
