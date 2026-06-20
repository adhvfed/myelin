//! # `notif` — the Notifications service binary (NOTIF-P1 → P-127)
//!
//! The "every service `main.rs`" the contract-index row 1.1 names: it does NOTHING but hand the
//! Notif [`AppSpec`](myelin_notif::notif_app_spec) to the harness's one call,
//! [`run_notif`](myelin_notif::run_notif) (a thin wrapper over `serve`). The harness owns the
//! whole lifecycle (boot → migrate → outbox relay → consumers → three ports → graceful drain,
//! with liveness ≠ readiness); this `main` is deliberately a four-line shim — a service main is
//! NOT a place for hand-rolled lifecycle logic; the platform substrate owns it once, for everyone.
//!
//! A failed boot / incomplete drain returns non-zero (§3.1) — loud, never a silent success.
//!
//! **Floor:** the env-first `Config::from_env()` parse lands with the driver (P-S15); here the
//! service boots over the validated default config, so the bootable-shell + fail-fast-on-bad-boot
//! properties are exercised.

use myelin_notif::run_notif;
use myelin_substrate::Config;

fn main() {
    // The env-first config parse is P-S15; the shell boots over the validated default today.
    let config = Config::default();
    match run_notif(config) {
        Ok(()) => {}
        Err(e) => {
            // A failed boot / incomplete drain returns non-zero (§3.1) — loud, never swallowed.
            eprintln!("notif service failed: {e}");
            std::process::exit(1);
        }
    }
}
