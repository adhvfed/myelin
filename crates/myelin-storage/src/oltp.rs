//! The OLTP tier client — the harness-wired bounded pool (Tier 1, Postgres-class).
//!
//! **Architecture:** storage.md §3.1 (Tier 1 OLTP: Postgres-class, ONE database per
//! service, system of record; **bounded pools + statement timeouts**), §1.1 (the
//! cache/coordination store is never a source of truth; T1 is). Contract 11.1 (the OLTP
//! tier client — pool half) + 1.1 (the harness opens it through `serve(AppSpec)`).
//!
//! ## What this is on the M0 floor (and the deferred concrete driver)
//! The substrate's `serve(AppSpec)` DB-pool body is itself a `todo!()` floor
//! (P-S12/P-S15). So [`OltpPool`] is a **backend-agnostic pool MODEL** over the same
//! `AppSpec` config the harness validates: bounded concurrent permits (max pool size),
//! a statement-timeout (milliseconds, the §3.1 bound), and a per-tenant in-flight cap so a
//! single tenant cannot starve the shared pool. The concrete `tokio-postgres`/`sqlx`
//! connection lands when `serve`'s pool body does (P-S12); the bounded-pool semantics +
//! the fast-fail + the saturation signal are complete and testable now and do not change
//! shape when the driver lands.
//!
//! ## Fast-fail on saturation (never block unboundedly) — the §3.1 bound + the drill
//! [`OltpPool::acquire`] returns immediately with [`OltpError::PoolSaturated`] /
//! [`OltpError::TenantInFlightCapExceeded`] when no permit is available — it does **not**
//! block unboundedly (an unbounded wait turns one slow query into a whole-pool stall, the
//! cascade §1.1 forbids). The rejection is the `PoolSaturation` survival signal the
//! bounded-pool drill reads.
//!
//! ## `residency-pin` lint — NAMED M0 FLOOR (`@residency-cell-pinned:file`)
//! The `residency-pin` architecture lint (P-S11 → P-018, SHARPENED to the real OLTP constructor
//! in P-ST-04 → P-020) requires every store/pool construction to pin a `Region`. This file is the
//! **M0 region-less pool MODEL**: on this floor the pool layer is region-agnostic (the cell's
//! region pins data OUT-OF-BAND — the per-query `(tenant, region)` `TenantScope` in [`crate::rls`]
//! carries the region). **The per-POOL runtime region-pin is now SHIPPED in [`crate::residency`]
//! (P-ST-15 / P-102, the STOR-D5 gate):** [`crate::residency::RegionPinnedStore`] pins each store to
//! its cell's `Region`, its [`crate::residency::RegionPinnedStore::admit_write`] is the in-process
//! residency WRITE boundary (an out-of-region write is rejected — no store writes outside its
//! region), and [`crate::residency::StoreSet::residency_verify`] is the
//! `myelin storage residency verify <tenant>` admin path that proves region pinning (0 cross-region
//! egress). This `OltpPool` MODEL stays region-agnostic at the permit-accounting layer (the region
//! pin lives on the store/store-set seam, not the bounded-pool counter); when the concrete sqlx
//! driver lands (P-S12) the pool is constructed inside a [`crate::residency::RegionPinnedStore`].
//! The file-level lint waiver marker `@residency-cell-pinned:file` records this LOUDLY (EI-01 §4 —
//! named, never a silent skip); the lint stays fully live on every caller/application file and fires
//! on any UNMARKED region-less store open.

/// The validated OLTP pool config (storage §3.1; contract 1.1 config). Read from the
/// service's `AppSpec` config and **validated at boot** (fast-fail on a nonsensical
/// config, never start with an unbounded pool).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OltpConfig {
    /// The bounded pool size — the max number of concurrent in-flight statements across all
    /// tenants. Must be ≥ 1 (an unbounded / zero pool is a config error).
    pub max_pool_size: u32,
    /// The statement timeout, in **milliseconds** (the frozen unit for resilient-client/DB
    /// timeouts, architecture §2.10). Must be ≥ 1 (a zero timeout is a config error). Every
    /// statement runs under this bound so a runaway query cannot hold a permit forever.
    pub statement_timeout_ms: u64,
    /// The per-tenant in-flight cap — the max concurrent statements a SINGLE tenant may hold
    /// at once, so one tenant cannot starve the shared pool (the noisy-neighbour bound).
    /// Must be ≥ 1 and ≤ `max_pool_size`.
    pub per_tenant_in_flight_cap: u32,
}

impl OltpConfig {
    /// Validate the config at boot (storage §3.1 — bounded-everything, validated up front).
    /// Returns the first violation as a loud [`OltpError::InvalidConfig`]; a service that
    /// boots with a bad pool config fails fast rather than starting with an unbounded pool.
    pub fn validate(&self) -> Result<(), OltpError> {
        if self.max_pool_size == 0 {
            return Err(OltpError::InvalidConfig(
                "max_pool_size must be >= 1 (no unbounded pool)",
            ));
        }
        if self.statement_timeout_ms == 0 {
            return Err(OltpError::InvalidConfig(
                "statement_timeout_ms must be >= 1",
            ));
        }
        if self.per_tenant_in_flight_cap == 0 {
            return Err(OltpError::InvalidConfig(
                "per_tenant_in_flight_cap must be >= 1",
            ));
        }
        if self.per_tenant_in_flight_cap > self.max_pool_size {
            return Err(OltpError::InvalidConfig(
                "per_tenant_in_flight_cap must be <= max_pool_size",
            ));
        }
        Ok(())
    }
}

/// An OLTP pool error. Every variant is a loud, typed value — a saturated pool is a
/// rejection, never an unbounded block (the cascade §1.1 forbids).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OltpError {
    /// The pool config failed boot-time validation (carries the precise reason).
    InvalidConfig(&'static str),
    /// No global permit was available — the bounded pool is saturated. The caller is
    /// rejected immediately (fast-fail) and the `PoolSaturation` signal is raised; it does
    /// NOT block unboundedly.
    PoolSaturated,
    /// This tenant already holds its per-tenant in-flight cap. Rejected so one tenant cannot
    /// starve the shared pool (the noisy-neighbour bound) even when global permits remain.
    TenantInFlightCapExceeded,
}

impl core::fmt::Display for OltpError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            OltpError::InvalidConfig(why) => write!(f, "invalid OLTP pool config: {why}"),
            OltpError::PoolSaturated => write!(
                f,
                "OLTP pool saturated — request rejected (fast-fail, not blocked; storage §1.1)"
            ),
            OltpError::TenantInFlightCapExceeded => write!(
                f,
                "per-tenant in-flight cap exceeded — request rejected (noisy-neighbour bound)"
            ),
        }
    }
}

impl std::error::Error for OltpError {}

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use myelin_tenancy::TenantId;

/// The harness-wired bounded OLTP pool (storage §3.1). Hands out [`PermitGuard`]s up to
/// `max_pool_size` globally and `per_tenant_in_flight_cap` per tenant; releases on drop.
/// **Acquisition never blocks** — it returns a rejection the instant a bound is hit.
///
/// Backend-agnostic on this floor (the in-memory permit accounting IS the bounded-pool
/// semantics); the concrete connection lands with the driver (P-S12). `Clone` shares the
/// same underlying counters (an `Arc`) so the pool is a single shared resource a service's
/// handlers acquire against.
#[derive(Clone, Debug)]
pub struct OltpPool {
    config: OltpConfig,
    state: Arc<Mutex<PoolState>>,
}

#[derive(Debug, Default)]
struct PoolState {
    /// Total permits currently held across all tenants (≤ `max_pool_size`).
    global_in_flight: u32,
    /// Per-tenant permits currently held (each ≤ `per_tenant_in_flight_cap`).
    per_tenant_in_flight: HashMap<TenantId, u32>,
    /// Cumulative count of rejections — the `PoolSaturation` survival signal a drill reads
    /// (it is non-zero iff the bounded pool fast-failed at least once).
    saturation_rejections: u64,
}

impl OltpPool {
    /// Open the pool through the harness with a validated config (contract 1.1 — a service
    /// opens its pool through `serve(AppSpec)`, never a hand-rolled connection). Validates
    /// the config at boot and fails fast on a bad one.
    pub fn open(config: OltpConfig) -> Result<OltpPool, OltpError> {
        config.validate()?;
        Ok(OltpPool {
            config,
            state: Arc::new(Mutex::new(PoolState::default())),
        })
    }

    /// The validated config this pool was opened with.
    pub fn config(&self) -> OltpConfig {
        self.config
    }

    /// Acquire one in-flight permit for `tenant`, scoped to the bounded pool. Returns a
    /// [`PermitGuard`] (released on drop) on success, or **immediately** rejects with
    /// [`OltpError::PoolSaturated`] / [`OltpError::TenantInFlightCapExceeded`] — it never
    /// blocks. A rejection bumps the saturation counter (the `PoolSaturation` signal).
    ///
    /// The per-tenant cap is checked FIRST so a noisy tenant is rejected with the precise
    /// reason even when global permits remain; then the global bound.
    pub fn acquire(&self, tenant: &TenantId) -> Result<PermitGuard, OltpError> {
        let mut state = self.state.lock().expect("OLTP pool mutex poisoned");
        let tenant_in_flight = state.per_tenant_in_flight.get(tenant).copied().unwrap_or(0);
        if tenant_in_flight >= self.config.per_tenant_in_flight_cap {
            state.saturation_rejections += 1;
            return Err(OltpError::TenantInFlightCapExceeded);
        }
        if state.global_in_flight >= self.config.max_pool_size {
            state.saturation_rejections += 1;
            return Err(OltpError::PoolSaturated);
        }
        state.global_in_flight += 1;
        *state
            .per_tenant_in_flight
            .entry(tenant.clone())
            .or_insert(0) += 1;
        Ok(PermitGuard {
            state: Arc::clone(&self.state),
            tenant: tenant.clone(),
            released: false,
        })
    }

    /// The current count of in-flight permits (across all tenants) — the `PoolSaturation`
    /// USE-utilisation signal value.
    pub fn in_flight(&self) -> u32 {
        self.state
            .lock()
            .expect("OLTP pool mutex poisoned")
            .global_in_flight
    }

    /// The cumulative count of saturation/cap rejections — the `PoolSaturation` survival
    /// signal a bounded-pool drill asserts is non-zero after the pool was driven to its
    /// bound (proving it fast-failed rather than blocking).
    pub fn saturation_rejections(&self) -> u64 {
        self.state
            .lock()
            .expect("OLTP pool mutex poisoned")
            .saturation_rejections
    }
}

/// An in-flight permit, released on drop (RAII). Holding one means the statement counts
/// against both the global pool bound and the tenant's in-flight cap; dropping it frees
/// both. There is no way to leak a permit (the guard cannot be cloned).
#[derive(Debug)]
pub struct PermitGuard {
    state: Arc<Mutex<PoolState>>,
    tenant: TenantId,
    released: bool,
}

impl Drop for PermitGuard {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        let mut state = self.state.lock().expect("OLTP pool mutex poisoned");
        state.global_in_flight = state.global_in_flight.saturating_sub(1);
        if let Some(n) = state.per_tenant_in_flight.get_mut(&self.tenant) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                state.per_tenant_in_flight.remove(&self.tenant);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> OltpConfig {
        OltpConfig {
            max_pool_size: 4,
            statement_timeout_ms: 5_000,
            per_tenant_in_flight_cap: 2,
        }
    }

    fn t(name: &str) -> TenantId {
        TenantId(name.into())
    }

    /// A valid config validates green and opens a pool.
    #[test]
    fn valid_config_opens() {
        assert_eq!(cfg().validate(), Ok(()));
        assert!(OltpPool::open(cfg()).is_ok());
    }

    /// Boot-time validation fast-fails an unbounded (zero) pool — never start unbounded.
    #[test]
    fn zero_pool_size_is_rejected_at_boot() {
        let c = OltpConfig {
            max_pool_size: 0,
            ..cfg()
        };
        assert!(matches!(c.validate(), Err(OltpError::InvalidConfig(_))));
        assert!(OltpPool::open(c).is_err());
    }

    /// A zero statement timeout is a config error (every statement must be time-bounded).
    #[test]
    fn zero_statement_timeout_is_rejected() {
        let c = OltpConfig {
            statement_timeout_ms: 0,
            ..cfg()
        };
        assert!(matches!(c.validate(), Err(OltpError::InvalidConfig(_))));
    }

    /// A per-tenant cap of zero, or one exceeding the pool size, is a config error.
    #[test]
    fn nonsensical_per_tenant_cap_is_rejected() {
        let zero = OltpConfig {
            per_tenant_in_flight_cap: 0,
            ..cfg()
        };
        assert!(matches!(zero.validate(), Err(OltpError::InvalidConfig(_))));
        let too_big = OltpConfig {
            per_tenant_in_flight_cap: 99,
            ..cfg()
        };
        assert!(matches!(
            too_big.validate(),
            Err(OltpError::InvalidConfig(_))
        ));
    }

    /// Acquire/drop accounts permits correctly: in-flight rises on acquire, falls on drop.
    #[test]
    fn acquire_and_release_accounts_permits() {
        let pool = OltpPool::open(cfg()).unwrap();
        assert_eq!(pool.in_flight(), 0);
        {
            let _g = pool.acquire(&t("acme")).unwrap();
            assert_eq!(pool.in_flight(), 1);
        }
        assert_eq!(
            pool.in_flight(),
            0,
            "dropping the guard releases the permit"
        );
    }

    /// **The bounded-pool fast-fail (the drill's mechanism).** Driving the pool to its
    /// GLOBAL bound (across tenants) rejects the next acquire IMMEDIATELY with
    /// `PoolSaturated` and bumps the saturation signal — it does not block.
    #[test]
    fn global_saturation_fast_fails_and_signals() {
        // pool size 4, per-tenant cap 2 → fill with two tenants holding 2 each = 4 global.
        let pool = OltpPool::open(cfg()).unwrap();
        let _a1 = pool.acquire(&t("acme")).unwrap();
        let _a2 = pool.acquire(&t("acme")).unwrap();
        let _b1 = pool.acquire(&t("beta")).unwrap();
        let _b2 = pool.acquire(&t("beta")).unwrap();
        assert_eq!(pool.in_flight(), 4);
        // a third tenant hits the GLOBAL bound — rejected, not blocked.
        let rejected = pool.acquire(&t("gamma"));
        assert!(matches!(rejected, Err(OltpError::PoolSaturated)));
        assert_eq!(
            pool.saturation_rejections(),
            1,
            "the PoolSaturation signal fired"
        );
    }

    /// **The per-tenant noisy-neighbour bound.** A single tenant cannot exceed its
    /// in-flight cap even when global permits remain — rejected with the precise reason.
    #[test]
    fn per_tenant_cap_fast_fails_before_global_bound() {
        let pool = OltpPool::open(cfg()).unwrap();
        let _a1 = pool.acquire(&t("acme")).unwrap();
        let _a2 = pool.acquire(&t("acme")).unwrap();
        // acme is at its cap (2) though only 2/4 global permits are used.
        let rejected = pool.acquire(&t("acme"));
        assert!(matches!(
            rejected,
            Err(OltpError::TenantInFlightCapExceeded)
        ));
        // another tenant still gets a permit (global headroom remains) — isolation holds.
        let _b1 = pool.acquire(&t("beta")).unwrap();
        assert_eq!(pool.in_flight(), 3);
    }

    /// Releasing a permit frees capacity for a subsequently-rejected tenant (no leak).
    #[test]
    fn releasing_frees_capacity() {
        let pool = OltpPool::open(cfg()).unwrap();
        let a1 = pool.acquire(&t("acme")).unwrap();
        let _a2 = pool.acquire(&t("acme")).unwrap();
        assert!(pool.acquire(&t("acme")).is_err()); // at cap
        drop(a1);
        let _a3 = pool.acquire(&t("acme")).unwrap(); // freed slot reusable
        assert_eq!(pool.in_flight(), 2);
    }
}
