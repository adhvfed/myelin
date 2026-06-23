//! # `issues` — the Issue Tracker service binary (ISS-P05 → P-371, M4)
//!
//! The "every service `main.rs`" the contract-index row 1.1 names: it does NOTHING but hand the
//! Issue Tracker [`AppSpec`](myelin_issues::issues_app_spec) to the harness's one call,
//! [`run_issues`](myelin_issues::run_issues) (a thin wrapper over `serve`). The harness owns the
//! whole lifecycle (boot → migrate → outbox relay → consumers → three ports → graceful drain, with
//! liveness ≠ readiness); this `main` is a four-line shim — a service main is NOT a place for
//! hand-rolled lifecycle logic; the substrate owns it once.
//!
//! On boot the Issue Tracker shell runs the complete forward-only issue-spine migrations (every spine
//! table — `issue` … `outbox` — each domain table `(tenant_id, region)`-first + RLS-on) and
//! auto-registers its OLTP spine store as the H3 `PersonalDataHolder`. A failed boot / incomplete
//! drain returns non-zero (§3.1) — loud, never a silent success.
//!
//! **Floor:** the env-first `Config::from_env()` parse lands with the driver; here the service boots
//! over the validated default config, so the bootable-shell + fail-fast-on-bad-boot properties are
//! exercised. The per-table behaviour (the write path ISS-P06, the key allocation ISS-P08, the scheme
//! algebra ISS-P11, the time axis ISS-P18+) lands in the follow-ons — this shell writes no row yet.

use myelin_issues::run_issues;
use myelin_substrate::Config;

fn main() {
    // The env-first config parse lands with the driver; the shell boots over the validated default.
    let config = Config::default();
    match run_issues(config) {
        Ok(()) => {}
        Err(e) => {
            // A failed boot / incomplete drain returns non-zero (§3.1) — loud, never swallowed.
            eprintln!("issues service failed: {e}");
            std::process::exit(1);
        }
    }
}
