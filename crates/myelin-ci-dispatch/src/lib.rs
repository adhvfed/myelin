//! # myelin-ci-dispatch — the CI Trigger & Dispatch service shell (CI-P6 → P-349, M4)
//!
//! **Owning architecture doc (byte-authoritative):**
//! `planning/04-subsystem-architectures/continuous-integration/architecture/00-overview.md` §4 (the
//! five logical services — this is service #1, Trigger & Dispatch: close to the bus; matches the
//! `EventMatcher` (= `QueryAst`), dedups on the triggering `event_id`, evaluates + stamps the trust
//! tier, resolves + content-addresses the definition, starts the workflow; **"stateless except the
//! dedup ledger"**) + §5 (cell topology); `01-tech-and-data-model.md` §3.8 (the `consumer_dedup`
//! ledger — the platform consumer template's exactly-once-effect anchor). **Contracts:**
//! `contract-index.md` rows 1.1 (`serve(AppSpec)` — the service shell), 1.2/1.3 (the three ports +
//! liveness ≠ readiness), 1.5 (the forward-only migration), 11.1 (OLTP), 12.1 (the `(tenant,
//! region)` partition key), 2.5 (the dedup ledger — one push = one run, exactly-once effect).
//!
//! ## What CI-P6 ships here — the bootable SHELL + the dedup-ledger schema, NOT the dispatch logic
//! [`dispatch_app_spec`] assembles the Trigger & Dispatch [`AppSpec`] the harness's ONE call drives
//! (boot → migrate → outbox relay → consumers → three ports → graceful drain, liveness ≠ readiness).
//! Trigger & Dispatch is an `AppSpec`, not a hand-rolled lifecycle — the EXACT analog of the Search
//! / Refs / Identity / CI-Control-Plane shells. The shell:
//!   - declares the **three ports** (public / internal / metrics-health) via the harness (1.2/1.3) —
//!     liveness must not check deps; readiness gates on the DB pool + the declared critical deps;
//!   - runs the **forward-only migration** for the ONE table this service owns: the `consumer_dedup`
//!     ledger ([`migrations::dispatch_migrations`]) — `(tenant_id, region)`-first + RLS-on, the
//!     exactly-once-effect anchor (contract 2.5);
//!   - declares its critical downstreams (`broker` — the bus it consumes triggering events from and
//!     starts the workflow on; `authz` — the ReBAC `read & !is_untrusted_fork` edge the trust-tier
//!     evaluation reads; the OLTP store is implicitly critical) for the readiness probe (§4.3).
//!
//! ## The dispatch BEHAVIOUR — CI-P10 (P-353) + the resolve/start (CI-P11, P-354)
//! CI-P6 (this shell) shipped the dedup-ledger SHAPE; **CI-P10 (P-353) lands the dispatch
//! behaviour** in the [`dispatch`] module ON TOP of that ledger: the [`compile_trigger`]
//! `EventMatcher` (= the frozen `QueryAst`, contract 3.4 — a `pull_request` trigger IS a `QueryAst`,
//! not CEL), the [`DedupLedger`] exactly-once effect (contract 2.5 — one push = one run), and the
//! [`classify_trust`] / [`stamp_trust`] trust-tier evaluation + the single consistent stamp onto
//! BOTH `JobSpec.trust_tier` AND `CheckStatus.trust_tier` (contract 4.9 / X-1). It REUSES the one
//! `myelin-query` matcher engine and the already-frozen `myelin-ci-sandbox` / `myelin-git` trust
//! enums — no second trigger language, no third trust enum.
//!
//! ## CI-P11 (P-354) — the definition resolution → CAS snapshot + the reserve/start handoff
//! **CI-P11 (P-354) lands the resolve/start** in the [`resolve`] module ON TOP of CI-P10's stamped
//! trigger: [`resolve::resolve_snapshot`] reads a parsed `.myelin/ci.*` [`resolve::CiDefinition`],
//! validates the DAG, **resolves every image to a digest FAIL-CLOSED** (a floating tag is rejected —
//! 0 un-digested references reach a snapshot), expands the matrix deterministically, and writes the
//! resolved DAG as a **T2 CAS blob** (`myelin_storage::BlobStore`, contract 11.2). Then
//! [`resolve::reserve_and_start`] builds the **atomic reserve+start handoff**
//! ([`resolve::StartHandoff`]): the `StartSpec` for the `ci.pipeline` workflow
//! (`myelin_flow::DurableExecutor::start`, contract 9.1) + the `ci_run` row + `ci.run.started` + the
//! first `ci.check.updated{state: queued}` per context (via the outbox, contract 2.2) — committed in
//! ONE tx (no partial run).
//!
//! ## Floors named (CI-P11 DoD)
//! - the **sandboxed dynamic-generation escape hatch** is HOOKED here
//!   ([`resolve::ResolvedSnapshot::has_dynamic_generation`] — a `Generate` job is a normal
//!   digest-pinned job on the CI-P3 runner, the SAME sandbox as any untrusted code, NO privileged
//!   config-eval path); the in-sandbox EXECUTION of the generation step lands with the runner + the
//!   `ci.pipeline` body (**CI-P15**);
//! - the **tag→digest registry resolution** (the real lookup + sigstore verify) is **CI-P23**
//!   (CI-D4); CI-P11 enforces the fail-closed PLAN-time half only.
//!
//! The shell below still registers no consumer — the LIVE bus-subscription consumer that drives
//! match→dedup→stamp→resolve→start end-to-end on each triggering event is the named follow-on; this
//! prompt ships the pure resolve/start core ([`resolve`]) the consumer will call, proven
//! deterministically (the CAS round-trip + the atomic-bundle invariant), DB-free by default.
//!
//! ## DB-free by default; the live-stack proof behind `integration`
//! `cargo build --workspace` / `cargo test --workspace` stay DB-free. The REAL forward-only apply
//! against the dev-stack Postgres (RLS isolation + the exactly-once PRIMARY KEY) is
//! `tests/integration_ci_p6_dispatch_schema.rs` behind the `integration` cargo feature.

pub mod dispatch;
pub mod migrations;
pub mod resolve;

pub use dispatch::{
    classify_trust, compile_trigger, git_trust_of, stamp_trust, trigger_matches, DedupLedger,
    OnTrigger, RunProvenance, TrustStamp, TrustTier, RUN_OBJECT_TYPE, TRIGGER_CONSUMER,
};

pub use resolve::{
    reserve_and_start, resolve_snapshot, snapshot_ref, CheckContext, CiDefinition, CiRunWrite,
    JobDef, JobKind, ResolveError, ResolvedJob, ResolvedSnapshot, RunFacts, StartHandoff,
    StartSpec, CI_PIPELINE_WF_TYPE,
};

use myelin_substrate::{
    boot, serve, AppSpec, Config, CriticalDependencies, InternalRpc, OutboxSpec, PublicRoutes,
    ServeError, ServeHandle, StoreManifest,
};

pub use migrations::{dispatch_migrations, CONSUMER_DEDUP_TABLE, CREATE_CONSUMER_DEDUP_DDL};

/// The deployable service name (the `AppSpec::name` + the telemetry/trace service identifier). The
/// `ci-dispatch` binary (`src/main.rs`) and the `AppSpec` both read this.
pub const SERVICE_NAME: &str = "ci-dispatch";

/// The critical-dependency set the metrics-health readiness probe reads (§4.3, SUB-D9). The OLTP
/// store is implicitly critical (the harness adds it). Trigger & Dispatch declares:
/// - `broker` — Trigger & Dispatch is "close to the bus" (arch 00 §4): it consumes the triggering
///   `git.*`/`issue.*`/… events and starts the `ci.pipeline` workflow on the durable bus. A dead
///   broker means it can neither receive a trigger nor dispatch a run — not-ready + shed.
/// - `authz` — the trust-tier evaluation reads the ReBAC `read & !is_untrusted_fork` ABAC edge
///   (contract 4.9) through Identity; a dead authz means CI cannot make the correct trust decision,
///   so it sheds rather than mis-stamping a trust tier (the poisoned-pipeline-execution defence).
fn dispatch_critical() -> CriticalDependencies {
    CriticalDependencies::new(["broker", "authz"])
}

/// **Assemble the CI Trigger & Dispatch service [`AppSpec`] (contract 1.1; the service shell).** The
/// harness owns the lifecycle around it (boot → migrate → relay → consumers → three ports →
/// graceful drain, liveness ≠ readiness). Trigger & Dispatch is an `AppSpec` + handlers, NOT a
/// hand-rolled `main`.
///
/// `config` is the validated, env-first config (§3.2; `Config::from_env()` lands with the driver,
/// P-S15). The forward-only migration creates the `consumer_dedup` ledger `(tenant, region)`-first
/// and RLS-on; `broker` / `authz` are declared critical. No consumers are registered at the SHELL —
/// the dispatch BEHAVIOUR (the [`dispatch`] module's `EventMatcher` + dedup + trust stamp) is
/// CI-P10's pure core; the bus-subscription consumer that drives it on each triggering event is
/// wired in with CI-P11's reserve/start. This shell carries the ledger SHAPE the idempotency rides.
pub fn dispatch_app_spec(config: Config) -> AppSpec {
    AppSpec {
        name: SERVICE_NAME,
        config,
        migrations: dispatch_migrations(),
        // No declared-hot table at the shell: the dedup ledger is an insert-on-trigger table, not a
        // claim-churn hot path; if its write rate warrants it, CI-P10 declares it (measured, §9.4).
        hot_tables: myelin_substrate::HotTables::none(),
        public: PublicRoutes::default(),
        internal: InternalRpc::default(),
        // No consumers at the shell — the dispatch behaviour is CI-P10's `dispatch` core; the
        // bus-subscription that wires it in lands with CI-P11's reserve/start.
        consumers: Vec::new(),
        holders: AppSpec::auto(),
        // Trigger & Dispatch owns only its OLTP store (the dedup ledger lives in it); the harness
        // adds the implicit OLTP store + auto-registers it as a holder.
        stores: StoreManifest::new(),
        outbox: OutboxSpec::default(),
        critical: dispatch_critical(),
    }
}

/// **Boot the CI Trigger & Dispatch service to the pre-serve [`ServeHandle`]** (the harness's
/// [`boot`] of [`dispatch_app_spec`]). Separated from [`run_dispatch`] so a test/drill can boot,
/// assert the three ports opened + the migration ran, drive ticks, and drive the drain.
pub fn boot_dispatch(config: Config) -> Result<ServeHandle, ServeError> {
    boot(dispatch_app_spec(config))
}

/// **The CI Trigger & Dispatch service entry — the one `serve(AppSpec)` call (contract 1.1).** The
/// `ci-dispatch` binary (`src/main.rs`) does nothing but hand [`dispatch_app_spec`] to this. A
/// failed boot / incomplete drain returns non-zero (§3.1) — loud, never a silent success.
pub fn run_dispatch(config: Config) -> Result<(), ServeError> {
    serve(dispatch_app_spec(config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_substrate::{Liveness, Surface};

    /// **THE Trigger & Dispatch shell boot test (contract 1.1/1.2/1.3): boots from `serve(AppSpec)`
    /// with three ports + liveness ≠ readiness; the forward-only dedup-ledger migration applies.**
    /// This is the prompt's GATE: the shell compiles + boots from `serve(AppSpec)` with the
    /// three-surface split and liveness ≠ readiness, and a forward-only migration creates the ledger.
    #[test]
    fn dispatch_boots_from_serve_appspec_with_three_ports() {
        let handle = boot_dispatch(Config::default())
            .expect("the Trigger & Dispatch shell boots from serve(AppSpec)");
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
            "readiness = can-serve-now (all critical deps healthy at boot) — distinct from liveness"
        );
    }

    /// **A dead critical dependency (`broker`) flips readiness to not-ready WITHOUT flipping liveness
    /// (liveness ≠ readiness, contract 1.3 / SUB-D9).** Trigger & Dispatch is "close to the bus": it
    /// cannot receive a trigger or dispatch a run with a dead broker, so it reports not-ready + sheds
    /// — but stays live (no restart storm).
    #[test]
    fn dead_broker_flips_readiness_not_liveness() {
        let handle = boot_dispatch(Config::default()).expect("boot");
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
            "liveness stays UP (not-ready is NOT not-alive — no restart storm)"
        );
    }

    /// **The Trigger & Dispatch shell runs the whole lifecycle end-to-end and drains cleanly
    /// (contract 1.1).** `run_dispatch` boots → migrates (creates the dedup ledger) → … →
    /// graceful-drains → returns Ok.
    #[test]
    fn run_dispatch_runs_lifecycle_and_returns_ok() {
        assert_eq!(
            run_dispatch(Config::default()),
            Ok(()),
            "the Trigger & Dispatch shell boots → … → drains cleanly"
        );
    }

    /// **A failed boot returns non-zero (§3.1).** A config that fails boot-time validation aborts
    /// boot loudly — the shell never starts half-booted.
    #[test]
    fn failed_boot_returns_non_zero() {
        let r = run_dispatch(Config("BAD_POOL".into()));
        assert!(r.is_err(), "a failed boot must return non-zero (Err)");
        assert!(
            r.unwrap_err().0.contains("fail-fast"),
            "the error names the §3.2 fail-fast validation"
        );
    }

    /// **The shell carries the dedup-ledger migration + the critical deps, and NO consumers (the
    /// dispatch-behaviour floor).** Pins the shell's surface so a later edit that smuggles in a
    /// consumer/handler without reconciliation, or drops the dedup ledger, is loud.
    #[test]
    fn the_shell_carries_the_dedup_ledger_and_no_consumers() {
        let spec = dispatch_app_spec(Config::default());
        assert_eq!(
            spec.migrations.0.len(),
            1,
            "the dedup ledger is the one table Trigger & Dispatch owns"
        );
        assert_eq!(
            spec.migrations.0[0].table,
            Some(CONSUMER_DEDUP_TABLE),
            "the migration creates the consumer_dedup ledger"
        );
        assert!(
            spec.consumers.is_empty(),
            "no consumers at the shell yet (the dispatch BEHAVIOUR is CI-P10's `dispatch` module; \
             the bus-subscription that wires it in lands with CI-P11's reserve/start)"
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
    }
}
