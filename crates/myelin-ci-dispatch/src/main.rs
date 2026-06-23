//! # `ci-dispatch` — the CI Trigger & Dispatch service binary (CI-P6 → P-349, M4)
//!
//! The "every service `main.rs`" the contract-index row 1.1 names: it does NOTHING but hand the CI
//! Trigger & Dispatch [`AppSpec`](myelin_ci_dispatch::dispatch_app_spec) to the harness's one call,
//! [`run_dispatch`](myelin_ci_dispatch::run_dispatch) (a thin wrapper over `serve`). The harness
//! owns the whole lifecycle (boot → migrate → outbox relay → consumers → three ports → graceful
//! drain, with liveness ≠ readiness); this `main` is a four-line shim.
//!
//! On boot the Trigger & Dispatch shell runs the forward-only migration that creates the
//! `consumer_dedup` ledger (the exactly-once-effect anchor) and auto-registers its OLTP store as a
//! `PersonalDataHolder`. A failed boot / incomplete drain returns non-zero (§3.1) — loud.
//!
//! **Floor:** the env-first `Config::from_env()` parse lands with the driver (P-S15); here the
//! service boots over the validated default. The dispatch behaviour (the `EventMatcher`, the
//! trust-tier stamp, the definition resolution → CAS snapshot, the reserve/start handoff) is
//! CI-P10/CI-P11 — this shell matches no event and starts no workflow yet.

use myelin_ci_dispatch::run_dispatch;
use myelin_substrate::Config;

fn main() {
    // The env-first config parse is P-S15; the shell boots over the validated default today.
    let config = Config::default();
    match run_dispatch(config) {
        Ok(()) => {}
        Err(e) => {
            // A failed boot / incomplete drain returns non-zero (§3.1) — loud, never swallowed.
            eprintln!("ci-dispatch service failed: {e}");
            std::process::exit(1);
        }
    }
}
