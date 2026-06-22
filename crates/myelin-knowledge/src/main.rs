//! # `knowledge` — the Knowledge service binary (KN-P04 → P-294, M3)
//!
//! The "every service `main.rs`" the contract-index row 1.1 names: it does NOTHING but hand the
//! Knowledge [`AppSpec`](myelin_knowledge::knowledge_app_spec) to the harness's one call,
//! [`serve`](myelin_substrate::serve). The harness owns the whole lifecycle (boot → migrate →
//! relay → consumers → three ports → graceful drain, with liveness ≠ readiness); this `main` is
//! deliberately a four-line shim (architecture 00 §3.1 / 03 §4 — a service main is NOT a place for
//! hand-rolled lifecycle logic; the platform substrate owns it once, for everyone).
//!
//! A failed boot returns non-zero (§3.1) — `serve` returns an `Err`, which this `main` converts to
//! a non-zero process exit (never a silent success).
//!
//! **Floor:** the env-first `Config::from_env()` parse (the real `DATABASE_URL`/broker/KMS/region
//! knobs) lands with the driver (P-S15); here the service boots over the validated default config.
//! The OLTP store + the (tenant, region) partition land in KN-P05; the outbox emit bodies +
//! consumers in KN-P06. The OS-signal (`SIGTERM`) → graceful-drain wiring lands with the real
//! ports (P-S13/P-S14); `serve` drives the deterministic drain today.

use myelin_knowledge::knowledge_app_spec;
use myelin_substrate::{serve, Config};

fn main() {
    // The env-first config parse is P-S15; the shell boots over the validated default today.
    let config = Config::default();
    match serve(knowledge_app_spec(config)) {
        Ok(()) => {}
        Err(e) => {
            // A failed boot / incomplete drain returns non-zero (§3.1) — loud, never swallowed.
            eprintln!("knowledge service failed: {e}");
            std::process::exit(1);
        }
    }
}
