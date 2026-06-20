//! # `search` — the Search & Indexing service binary (SRCH-P03 → P-166, M2)
//!
//! The "every service `main.rs`" the contract-index row 1.1 names: it does NOTHING but hand the
//! Search [`AppSpec`](myelin_search::search_app_spec) to the harness's one call,
//! [`run_search`](myelin_search::run_search) (a thin wrapper over `serve`). The harness owns the
//! whole lifecycle (boot → migrate → outbox relay → consumers → three ports → graceful drain, with
//! liveness ≠ readiness); this `main` is deliberately a four-line shim — a service main is NOT a
//! place for hand-rolled lifecycle logic; the platform substrate owns it once, for everyone.
//!
//! On boot the Search shell runs the forward-only migration that creates the per-tenant index
//! directory (encrypted-from-birth under the per-tenant index DEK) and auto-registers the per-tenant
//! search index as the H7 `PersonalDataHolder`. A failed boot / incomplete drain returns non-zero
//! (§3.1) — loud, never a silent success.
//!
//! **Floor:** the env-first `Config::from_env()` parse lands with the driver (P-S15); here the
//! service boots over the validated default config, so the bootable-shell + fail-fast-on-bad-boot
//! properties are exercised. The engine itself (the `IndexBackend` + the three index shapes + the
//! indexer + the query path) is SRCH-P04..P08 — this shell answers no query yet (named floor,
//! [`myelin_search::srch_p03_floors`]).

use myelin_search::run_search;
use myelin_substrate::Config;

fn main() {
    // The env-first config parse is P-S15; the shell boots over the validated default today.
    let config = Config::default();
    match run_search(config) {
        Ok(()) => {}
        Err(e) => {
            // A failed boot / incomplete drain returns non-zero (§3.1) — loud, never swallowed.
            eprintln!("search service failed: {e}");
            std::process::exit(1);
        }
    }
}
