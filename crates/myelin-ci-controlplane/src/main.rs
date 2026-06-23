//! # `ci-controlplane` — the CI Control Plane service binary (CI-P6 → P-349, M4)
//!
//! The "every service `main.rs`" the contract-index row 1.1 names: it does NOTHING but hand the CI
//! Control Plane [`AppSpec`](myelin_ci_controlplane::controlplane_app_spec) to the harness's one
//! call, [`run_controlplane`](myelin_ci_controlplane::run_controlplane) (a thin wrapper over
//! `serve`). The harness owns the whole lifecycle (boot → migrate → outbox relay → consumers →
//! three ports → graceful drain, with liveness ≠ readiness); this `main` is a four-line shim — a
//! service main is NOT a place for hand-rolled lifecycle logic; the substrate owns it once.
//!
//! On boot the CI Control Plane shell runs the complete forward-only data-model migrations (every CI
//! Control-Plane table — `ci_run` … `cost_event` — `(tenant, region)`-first + RLS-on) and
//! auto-registers its OLTP store as a `PersonalDataHolder`. A failed boot / incomplete drain returns
//! non-zero (§3.1) — loud, never a silent success.
//!
//! **Floor:** the env-first `Config::from_env()` parse lands with the driver (P-S15); here the
//! service boots over the validated default config, so the bootable-shell + fail-fast-on-bad-boot
//! properties are exercised. The per-table behaviour (the scheduler claim, the check emitter, the
//! log index, the metering) lands in the CI-P12..CI-P24 follow-ons — this shell runs no job yet.

use myelin_ci_controlplane::run_controlplane;
use myelin_substrate::Config;

fn main() {
    // The env-first config parse is P-S15; the shell boots over the validated default today.
    let config = Config::default();
    match run_controlplane(config) {
        Ok(()) => {}
        Err(e) => {
            // A failed boot / incomplete drain returns non-zero (§3.1) — loud, never swallowed.
            eprintln!("ci-controlplane service failed: {e}");
            std::process::exit(1);
        }
    }
}
