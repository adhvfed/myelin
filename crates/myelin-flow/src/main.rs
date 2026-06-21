//! # `myelin-flow` — the durable-workflow service binary (P-FLOW-02 → P-198, M2)
//!
//! The "every service `main.rs`" the contract-index row 1.1 names: it does NOTHING but hand the
//! flow [`AppSpec`](myelin_flow::flow_app_spec) to the harness's one call,
//! [`run_flow`](myelin_flow::run_flow) (a thin wrapper over `serve`). The harness owns the whole
//! lifecycle (boot → migrate → outbox relay → consumers → three ports → graceful drain, with
//! liveness ≠ readiness); this `main` is deliberately a four-line shim — a service main is NOT a
//! place for hand-rolled lifecycle logic; the platform substrate owns it once, for everyone (§10:
//! the engine boots from `serve(AppSpec)`, there is no second emit/boot path).
//!
//! A failed boot / incomplete drain returns non-zero (§3.1) — loud, never a silent success.
//!
//! **Floor:** the env-first `Config::from_env()` parse lands with the driver (P-S15); here the
//! service boots over the validated default config, so the bootable-shell + fail-fast-on-bad-boot
//! properties are exercised. The `consumers` slot is empty (the replay engine + signal/timer
//! consumers are P-FLOW-04..05/09/13) — this shell boots + migrates + relays, it does not yet run
//! a workflow.

use myelin_flow::run_flow;
use myelin_substrate::Config;

fn main() {
    // The env-first config parse is P-S15; the shell boots over the validated default today.
    let config = Config::default();
    match run_flow(config) {
        Ok(()) => {}
        Err(e) => {
            // A failed boot / incomplete drain returns non-zero (§3.1) — loud, never swallowed.
            eprintln!("myelin-flow service failed: {e}");
            std::process::exit(1);
        }
    }
}
