//! # `app` — the Issue Tracker service shell (`serve(AppSpec)`) — ISS-P05 / P-371, M4
//!
//! **Owning architecture docs (byte-authoritative):**
//! `planning/04-subsystem-architectures/issue-tracker/architecture/00-overview.md` §2.2 (the Issues
//! services are thin shells over identical plumbing — `serve(AppSpec)`, not a hand-rolled `main`) +
//! `01-tech-and-data-model.md` §2–§8 (the complete spine this shell migrates). **Contracts:**
//! `contract-index.md` rows 1.1 (`serve(AppSpec)` — the service shell), 1.2/1.3 (the three ports +
//! liveness ≠ readiness), 1.5 (the forward-only migrations + hot-table flags), 11.1 (OLTP + RLS),
//! 12.1 (the `(tenant, region)` partition key), 10.1 (the auto-registered H3 holder), 2.3/2.5 (the
//! outbox + consumer_dedup tables).
//!
//! ## What ISS-P05 ships here — the bootable SHELL + the COMPLETE spine data model, NOT the behaviour
//! [`issues_app_spec`] assembles the Issue Tracker [`AppSpec`] the harness's ONE call drives (boot →
//! migrate → outbox relay → consumers → three ports → graceful drain, liveness ≠ readiness). The
//! Issues service is an `AppSpec`, not a hand-rolled lifecycle — the EXACT analog of the CI Control
//! Plane / Search / Refs / Identity service shells. The shell:
//!   - declares the **three ports** (public / internal / metrics-health) via the harness (1.2/1.3) —
//!     liveness must not check deps; readiness gates on the DB pool + the declared critical deps;
//!   - runs the **complete forward-only issue-spine migrations**
//!     ([`crate::migrations::issues_migrations`]): all eleven spine tables (`issue` + its six
//!     hot-path indexes, `issue_relation` + its two traversal indexes, `issue_change_log`, `scheme`,
//!     `scheme_assignment`, `cycle`, `cycle_membership`, `milestone`, `prefix_counter`,
//!     `consumer_dedup`, `outbox`), each domain table `(tenant_id, region)`-first + RLS-on
//!     (11.1/12.1/1.5);
//!   - declares the **three hot tables** ([`crate::migrations::issues_hot_tables`]) — `issue`,
//!     `issue_relation`, `issue_change_log` (arch 01 §8.1 "Hot tables flagged");
//!   - declares `identity` (the authz/ReBAC check the write path + the surfacing push-down depend on)
//!     critical for the readiness probe (the OLTP store is implicitly critical);
//!   - auto-registers the Issues OLTP spine store as the **H3 `PersonalDataHolder`** through the one
//!     door (`holders: AppSpec::auto()`, contract 1.4 — [`crate::holder`]).
//!
//! ## Floors named (the per-table-behaviour follow-ons — see [`crate::migrations`])
//! The ISS-P05 shell ships the table SHAPES + the bootable shell; the per-table behaviour lands in
//! its own prompt: the silent-data-loss-safe write path (ISS-P06), the per-subject DEK + the full
//! holder ops (ISS-P07), the Hi/Lo key allocation (ISS-P08), the scheme algebra (ISS-P11), the time
//! axis (ISS-P18+). The PG-sharded storage floor (distributed-SQL = the measured R-6 follow-on,
//! ISS-P32) is named in [`crate::migrations`]. NO consumers are registered at the shell — the rollup
//! / SLA / trigger / feeder bus consumers land with their behaviour bands; the shell's outbox is a
//! PRODUCER seam the write path (ISS-P06) emits into.
//!
//! ## DB-free by default; the live-stack proof behind `integration`
//! `cargo build --workspace` / `cargo test --workspace` stay DB-free (the shell boots over the
//! substrate's in-process floor pool; the migrations are `&str` DDL the runner admits without a DB).
//! The REAL forward-only apply against the dev-stack Postgres (RLS isolation + the FK + the hot-path
//! indexes) is `tests/integration_iss_p05_spine_schema.rs` behind the `integration` cargo feature.

use crate::migrations::{issues_hot_tables, issues_migrations};
use myelin_events::OutboxStore;
use myelin_substrate::{
    boot, serve, AppSpec, Config, CriticalDependencies, InternalRpc, OutboxSpec, PublicRoutes,
    ServeError, ServeHandle, StoreManifest,
};

/// The deployable service name (the `AppSpec::name` + the telemetry/trace service identifier). The
/// `issues` binary (`src/main.rs`) and the `AppSpec` both read this.
pub const SERVICE_NAME: &str = "issues";

/// The critical-dependency set the metrics-health readiness probe reads (§4.3, SUB-D9). The OLTP
/// store is implicitly critical (the harness adds it). The Issue Tracker declares `identity` — the
/// authz/ReBAC `check`/`list_objects` the write path's permission check + the surfacing push-down
/// depend on (a dead authz means Issues cannot make a correct visibility/transition decision, so it
/// reports not-ready + sheds rather than serving a wrong answer). A dead critical dependency reports
/// not-ready + sheds while liveness stays Up (no restart storm).
fn issues_critical() -> CriticalDependencies {
    CriticalDependencies::new(["identity"])
}

/// **Assemble the Issue Tracker service [`AppSpec`] (contract 1.1; the service shell).** The harness
/// owns the lifecycle around it (boot → migrate → relay → consumers → three ports → graceful drain,
/// liveness ≠ readiness). The Issue Tracker is an `AppSpec` + handlers, NOT a hand-rolled `main`.
///
/// `config` is the validated, env-first config (§3.2; `Config::from_env()` lands with the driver —
/// the shell boots over the validated default today). The complete forward-only issue-spine
/// migrations create all eleven spine tables `(tenant_id, region)`-first + RLS-on; the three hot
/// tables are declared; `identity` is declared critical; the Issues OLTP store auto-registers as the
/// H3 holder. No consumers are registered here (the rollup/SLA/trigger/feeder consumers are the
/// per-band follow-ons; the write path is ISS-P06).
/// **The outbox is INJECTED (MR-009b W3b.4 — the composition root owns durability):** the
/// production `main.rs` constructs `OutboxStore::durable(PgOutboxBacking)` over the MR-022
/// `SubstrateProvider` pool (foundation migrations applied, fail-loud on missing durable config);
/// a test/drill passes the in-memory `OutboxStore::new()` double. This builder constructs NO
/// store of its own — the W3 dedup-injection precedent applied to the outbox.
pub fn issues_app_spec(config: Config, outbox: OutboxStore) -> AppSpec {
    AppSpec {
        name: SERVICE_NAME,
        config,
        migrations: issues_migrations(),
        hot_tables: issues_hot_tables(),
        public: PublicRoutes::default(),
        internal: InternalRpc::default(),
        // No consumers at the shell — the rollup/SLA/trigger/feeder bus consumers land with their
        // behaviour bands; the outbox is a PRODUCER seam the write path (ISS-P06) emits into.
        consumers: Vec::new(),
        holders: AppSpec::auto(),
        // The implicit OLTP store (the harness adds it) is the spine store the Issues service owns at
        // the shell — every spine table lives in the one Postgres; it auto-registers as H3 (the only
        // store the shell owns; the blob attachments tier is declared by ISS-P19's behaviour band).
        stores: StoreManifest::new(),
        // Producer-only shared durable outbox. The elected cell relay is the only publisher.
        outbox: OutboxSpec::external_relay(outbox),
        critical: issues_critical(),
    }
}

/// **Boot the Issue Tracker service to the pre-serve [`ServeHandle`]** (the harness's [`boot`] of
/// [`issues_app_spec`]). Separated from [`run_issues`] so a test/drill can boot, assert the three
/// ports opened + the migrations ran + the holder registered, drive ticks, and drive the drain
/// deterministically.
pub fn boot_issues(config: Config, outbox: OutboxStore) -> Result<ServeHandle, ServeError> {
    boot(issues_app_spec(config, outbox))
}

/// **The Issue Tracker service entry — the one `serve(AppSpec)` call (contract 1.1).** The `issues`
/// binary (`src/main.rs`) does nothing but hand [`issues_app_spec`] to this. A failed boot /
/// incomplete drain returns non-zero (§3.1) — loud, never a silent success.
pub fn run_issues(config: Config, outbox: OutboxStore) -> Result<(), ServeError> {
    serve(issues_app_spec(config, outbox))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrations::{ISSUE_CHANGE_LOG_TABLE, ISSUE_RELATION_TABLE, ISSUE_TABLE};
    use myelin_substrate::{Liveness, Surface};

    /// **THE Issue Tracker shell boot test (contract 1.1/1.2/1.3): boots from `serve(AppSpec)` with
    /// three ports + liveness ≠ readiness; the complete forward-only issue-spine data model applies.**
    /// This is the prompt's GATE: the shell compiles + boots from `serve(AppSpec)` with the
    /// three-surface split and liveness ≠ readiness, and the forward-only migrations create every
    /// spine table.
    #[test]
    fn issues_boots_from_serve_appspec_with_three_ports() {
        let handle = boot_issues(Config::default(), OutboxStore::new())
            .expect("the Issue Tracker shell boots from serve(AppSpec)");
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

    /// **A dead critical dependency (`identity`) flips readiness to not-ready WITHOUT flipping
    /// liveness (liveness ≠ readiness, contract 1.3 / SUB-D9).** Issues cannot make a correct
    /// visibility/transition decision with a dead authz, so it reports not-ready + sheds — but stays
    /// live (no restart storm).
    #[test]
    fn dead_identity_flips_readiness_not_liveness() {
        let handle = boot_issues(Config::default(), OutboxStore::new()).expect("boot");
        let mh = handle.metrics_health();
        assert!(mh.readiness().is_ready(), "ready while identity is healthy");

        handle.health_probe().mark_down("identity");

        assert!(
            !mh.readiness().is_ready(),
            "a dead authz/identity → not-ready + shed"
        );
        assert_eq!(
            mh.liveness(),
            Liveness::Up,
            "liveness stays UP (not-ready is NOT not-alive — no restart storm)"
        );
    }

    /// **The Issue Tracker shell runs the whole lifecycle end-to-end and drains cleanly (contract
    /// 1.1).** `run_issues` boots → migrates (creates every spine table) → … → graceful-drains →
    /// returns Ok. The CDC consumer side of 1.1 (a service `main` that just calls the one entry).
    #[test]
    fn run_issues_runs_lifecycle_and_returns_ok() {
        assert_eq!(
            run_issues(Config::default(), OutboxStore::new()),
            Ok(()),
            "the Issue Tracker shell boots → … → drains cleanly"
        );
    }

    /// **A failed boot returns non-zero (§3.1).** A config that fails boot-time validation aborts
    /// boot loudly — the shell never starts half-booted.
    #[test]
    fn failed_boot_returns_non_zero() {
        let r = run_issues(Config("BAD_POOL".into()), OutboxStore::new());
        assert!(r.is_err(), "a failed boot must return non-zero (Err)");
        assert!(
            r.unwrap_err().0.contains("fail-fast"),
            "the error names the §3.2 fail-fast validation"
        );
    }

    /// **The shell's AppSpec carries the complete spine data model + the three hot tables + the
    /// critical dep, and NO consumers (the behaviour floor).** Pins the shell's surface so a later
    /// edit that smuggles in a consumer without reconciliation, or drops a table / a hot-table flag,
    /// is loud.
    #[test]
    fn the_shell_carries_the_complete_spine_and_no_consumers() {
        let spec = issues_app_spec(Config::default(), OutboxStore::new());
        assert_eq!(
            spec.migrations.0.len(),
            11,
            "all 11 spine tables are in the forward-only migration set"
        );
        assert!(
            spec.consumers.is_empty(),
            "no consumers at the shell (the rollup/SLA/trigger/feeder consumers are the per-band follow-ons)"
        );
        for t in [ISSUE_TABLE, ISSUE_RELATION_TABLE, ISSUE_CHANGE_LOG_TABLE] {
            assert!(spec.hot_tables.is_hot(t), "`{t}` is declared hot");
        }
        let deps: Vec<&str> = spec.critical.deps().iter().map(|d| d.0.as_str()).collect();
        assert!(
            deps.contains(&"identity"),
            "identity is critical (the authz dependency)"
        );
    }
}
