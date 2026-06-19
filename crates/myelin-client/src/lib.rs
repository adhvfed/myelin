//! # `myelin-client` — the shared resilient inter-service client
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/00-platform-substrate.md`
//! §2.5 (`myelin-client` — the substrate-relevant seam) and §6 (the shared resilient
//! inter-service client).
//!
//! **Contract-index cluster:** 1 — Bootstrap & service shell
//! (`planning/05-refined-shared-systems-architecture/contract-index.md` row 1.9
//! `ResilientClient::call`).
//!
//! ## What crosses the crate boundary here (the frozen surface)
//! `ResilientClient::call(target, req, idem)` — the ONE client every outbound
//! inter-service call goes through, so timeout/breaker/bulkhead/retry is correct in
//! exactly one place. The four primitives (all mandatory, all on by default): per-call
//! **timeout** (deadlines propagate), circuit **breaker** (never retry through a tripped
//! breaker — the retry-storm amplifier), bounded-concurrency **bulkhead** (saturation
//! fast-fails, never queues unboundedly), and jittered **retry** — **idempotent calls
//! only** (full jitter; a `NonIdempotent` call is never retried). Our clients **MUST
//! honour `Retry-After`** (§6.2) so shedding cannot become a retry storm.
//!
//! ## Frozen units (architecture §6.3, §2.10)
//! Resilient-client timeouts = **milliseconds**; breaker thresholds = failure ratio over
//! a rolling window + a minimum request count; bulkhead = integer concurrency cap;
//! backoff base in ms with full jitter.
//!
//! ## Floors named (stubbed bodies → filling prompt)
//! `call`'s body is `todo!()`. The four primitives + `Retry-After` honouring land in the
//! substrate roadmap:
//! - the four resilient-client primitives → **P-S16**;
//! - `Retry-After` honouring (SUB-D5, the retry-storm drill) → **P-S17**.
//!
//! P-001 ships only the frozen `call` signature (1.9).

use serde::{Deserialize, Serialize};

/// The target of an inter-service call (architecture §6; contract 1.9). The per-target
/// breaker/bulkhead are keyed on this; the concrete addressing lands with P-S16.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Target(pub String);

/// An outbound request (architecture §6; contract 1.9). Opaque in the skeleton; the typed
/// request/response lands with P-S16.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Req(pub String);

/// Whether a call is safe to retry (architecture §6; contract 1.9). A `NonIdempotent`
/// call is NEVER retried (full-jitter retry is idempotent-only).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Idempotency {
    Idempotent,
    NonIdempotent,
}

/// Placeholder error for the skeleton (timeout / breaker-open / bulkhead-rejected). Real
/// taxonomy lands with P-S16.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallError(pub String);

/// `Result` alias for the client surface.
pub type Result<T> = core::result::Result<T, CallError>;

/// The shared resilient inter-service client (architecture §6; contract 1.9; ADR-16).
/// One place where timeout + breaker + bulkhead + jittered-retry-idempotent-only is
/// correct for every caller (services, CLI, agent runtime). Honours `Retry-After`.
#[derive(Clone, Debug, Default)]
pub struct ResilientClient {
    // The breaker/bulkhead/timeout config is private state filled by P-S16; the skeleton
    // is a unit struct so `call`'s frozen signature compiles.
    _private: (),
}

impl ResilientClient {
    /// Every call: per-call TIMEOUT, BULKHEAD (bounded concurrency), through the BREAKER.
    /// Retry ONLY if idempotent, with full jitter, NEVER through a tripped breaker
    /// (architecture §6; contract 1.9).
    ///
    /// **Floor:** body is `todo!()`; the four primitives land in **P-S16**, `Retry-After`
    /// honouring in **P-S17** (SUB-D5).
    pub fn call<R>(&self, _target: Target, _req: Req, _idem: Idempotency) -> Result<R> {
        todo!("the four resilient-client primitives land in P-S16; Retry-After in P-S17")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile-asserting test: the `ResilientClient::call(target, req, idem)` signature is
    /// frozen (contract 1.9), generic in the response `R`, with the `Idempotency` enum
    /// gating retry. The struct is constructible (`Default`); `call`'s body is the P-S16
    /// floor.
    #[test]
    fn resilient_client_call_signature_is_frozen() {
        let client = ResilientClient::default();
        // We do not invoke `call` (its body is `todo!()`); we assert it is nameable with
        // the frozen parameter types by taking a function pointer to the monomorphised
        // form. This is the compile-time shape assertion.
        let _f: fn(&ResilientClient, Target, Req, Idempotency) -> Result<()> =
            ResilientClient::call::<()>;
        let _ = &client;
        assert_eq!(Idempotency::Idempotent, Idempotency::Idempotent);
    }
}
