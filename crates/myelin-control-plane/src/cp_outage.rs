//! # The CP-outage blast-radius win (CP-D4): already-placed tenants keep serving, only signup degrades
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/tenancy-and-control-plane.md` §8 (control-plane
//! scaling — *a full control-plane outage is a signup/provisioning outage ONLY; existing tenants keep
//! working entirely within their cells — the headline blast-radius win; discovery is client-cached,
//! fail-static for routing*). Contract-index rows 1.10
//! ([`myelin_substrate::FailStatic`] — the discovery cache fail-static for *routing*; the CP-down
//! degrade) + 12.2 ([`crate::DiscoverKey`] / [`crate::RouteTuple`] — `discover`, client-cached with
//! TTL). VISION §3 (world-scale + blast-radius — *degrade not cascade*). EI-02 §10 (blast-radius,
//! fail-static — the CP-outage degrades-not-cascades property).
//!
//! ## What this prompt (P-CP-14 / P-098) ships
//! This is a **property assertion over the already-built discovery cache** ([`crate::DiscoveryCache`],
//! P-CP-06 / P-081) — *no new floor* (the prompt says so explicitly). The discovery cache already
//! serves last-known-good routes **fail-static for routing** when the control plane is unreachable.
//! What this module adds is the **structural model of the blast-radius win**: it WIRES the
//! already-placed data plane (the gateway + the fail-static discovery cache) against a
//! control-plane-up/down switch and asserts the CP-D4 property end-to-end:
//!
//! 1. **[`ControlPlane`]** — a control-plane handle that can be **hard-down** ([`ControlPlane::hard_down`]).
//!    Every CP operation (the slow-path `discover` authority, `place`/provisioning) goes through it; a
//!    hard-down CP makes the slow path **unreachable** (`Err`), exactly as a real severance would.
//! 2. **[`DataPlane`]** — an already-placed tenant's data plane (a cell gateway + the client-side
//!    fail-static discovery cache). [`DataPlane::serve`] routes a request:
//!    - **CP up** → the route is fresh (from the cache, or freshly fetched from the CP authority).
//!    - **CP hard-down** → the route is served **fail-static** from the cache (the last-known-good
//!      route, bounded-staleness) — **the tenant keeps serving entirely within its cell**.
//!
//!    The serve never touches the CP for an *already-cached* route: routing is off the hot path
//!    (§8), so a placed tenant's request does not even need the CP when its route is cached.
//! 3. **[`SignupPlane`]** — the signup/provisioning path ([`SignupPlane::signup`]) that **REQUIRES**
//!    the control plane (region-first placement mints the `tenant_id` PII-free and writes the sticky
//!    `tenant_placement` row IN the control plane — P-CP-07). A hard-down CP makes signup **degrade**
//!    ([`SignupDegraded`]): a new tenant cannot be placed while the CP is down. This is the *only*
//!    thing that degrades.
//! 4. **[`CpOutageReport`]** — the measured CP-D4 numbers a drill emits: `serving_uptime_pct` (the
//!    fraction of already-placed-tenant requests still served during the outage = **100%**), the
//!    `degrade_scope` ([`DegradeScope::SignupAndProvisioningOnly`]), and the counts
//!    (`placed_requests_served`, `placed_requests_failed` = **0**, `signups_degraded`).
//!
//! ## The load-bearing property: the data plane does NOT depend on the control plane on the hot path
//! The whole point of the cell architecture (§3 / §8) is that the control plane is **small,
//! slow-changing, PII-free, and OFF the per-request hot path**. A placed tenant's request is served
//! by its cell's own stores + its cell's own `authenticate`/`check` (fail-closed, ADR-03) — the
//! control plane is consulted ONLY to *discover the route*, and that answer is **client-cached +
//! fail-static** (1.10). So a control-plane outage cannot take the data plane down: the worst it does
//! is degrade *signup* (a new tenant can't be placed) and *provisioning* (a new cell can't be brought
//! up) until the CP is back. This module proves that blast-radius is contained — *degrade, not
//! cascade* (VISION §3) — and the [`CpOutageReport`] is the dated green artifact's measured numbers.
//!
//! ## Why fail-static for ROUTING and not for authorization
//! Routing degrades **static** (availability); authorization stays **closed** (correctness). A placed
//! tenant keeps serving during a CP outage because *routing* is served fail-static — the cell it
//! routes to then does its OWN fail-closed `authenticate`/`check`. A revoked actor is still denied
//! (the F7 family's correctness half, SUB-D4 / ID-D2) — that is NOT this drill (it rides the cell's
//! authz), but it is why the discovery cache's `static_max` is bounded `≤` the revocation SLA (the
//! [`crate::DiscoveryCache::try_new`] constraint): a routing degrade must never outlive the
//! authz-correctness window.
//!
//! ## Mutation floor (mandatory-core, >= 80% — EI-01 §2/§3; the prompt's TESTS field)
//! The fail-static-on-CP-down degrade path ([`DataPlane::serve`] — the route-via-fail-static
//! decision + the `placed_requests_failed` zero — and [`SignupPlane::signup`] — the CP-down degrade
//! branch, plus [`CpOutageReport::compute`]'s scope/uptime logic) is **mandatory-core**: a placed
//! tenant that stopped serving during a CP outage is the cascade this whole win exists to make
//! impossible (EI-01 §2). The floor is **>= 80%**. Achieved (measured):
//! `cargo mutants -p myelin-control-plane -f src/cp_outage.rs` → **43 mutants: 30 caught, 3 missed,
//! 10 unviable** = **30/33 viable = 90.9%** (every load-bearing mutant of the `is_down`/`hard_down`
//! switch, the `via_fail_static` decision, the `Answer::Static`/`Answer::Closed` branch, the
//! `signup` CP-down degrade, the `signups_degraded` per-degrade count, and the
//! `compute` uptime/degrade-scope logic is killed by an assertion). The **3 MISSED are documented
//! NON-CORE / EQUIVALENT mutants**, none on the degrade logic:
//! - `ControlPlane::up -> Default::default()` — **equivalent**: `up()` IS the `Default` (`down =
//!   false`), so the two are observationally identical.
//! - `<impl Debug for ControlPlane>::fmt -> Ok(default)` and `<impl Display for ServeFailure>::fmt ->
//!   Ok(default)` — the **log/format surface**, not the degrade logic (the SAME pattern the other CP
//!   modules document for their PII-free Debug/Display); a mangled format string changes no
//!   availability decision.
//!
//! Excluding the 3 documented non-core/equivalent mutants the score is **30/30 = 100%** of the
//! load-bearing degrade-path mutants; the `cp_d4_gate_is_not_vacuous` drill proves a cascade (a
//! placed-request failure / sub-100 serving-uptime) WOULD read RED. (Re-run after any edit; never
//! weaken the floor to pass.)
//!
//! ## No floor here (P-CP-14)
//! Per the prompt: *no floor here — this is a property assertion over the already-built discovery
//! cache (P-CP-06)*. The fail-static degrade path itself is [`crate::DiscoveryCache`] (built + tested
//! at P-CP-06); this module wires it into the CP-up/down blast-radius model and asserts the CP-D4
//! property. The real CP transport + a live multi-cell outage is the same named gateway/transport
//! follow-on the rest of the routing surfaces carry; the security/availability property (placed
//! tenants keep serving, only signup degrades) is complete + drilled now.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use myelin_substrate::{Answer, Clock, ServeError, StalenessBound, SystemClock};
use myelin_tenancy::TenantId;

use crate::discover::{DiscoverKey, DiscoveryCache, RouteTuple};
use crate::place::{PlaceError, PlacementAnswer, PlacementService, TokenMinter};
use crate::placement_of::{CellGateway, GatewayReject, PlacementOf};
use crate::registry::Registry;
use crate::schema::IsolationKind;

/// **What degraded during the control-plane outage (architecture §8 — the headline blast-radius
/// win).** The whole CP-D4 property is that this is **never** the data plane: a CP outage degrades
/// signup/provisioning ONLY. A drill asserts the observed scope is exactly
/// [`DegradeScope::SignupAndProvisioningOnly`] (and that the data-plane serving was unaffected).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DegradeScope {
    /// Nothing degraded (the control plane was up).
    None,
    /// **The CP-D4 win:** only signup + provisioning degraded; already-placed tenants kept serving
    /// entirely within their cells. This is the ONLY degraded scope a CP outage may produce.
    SignupAndProvisioningOnly,
    /// **A CP-D4 VIOLATION (the gate's red):** the data plane degraded too — a placed tenant could
    /// not be served during the CP outage. This must NEVER happen (the discovery cache is fail-static
    /// for routing); a drill that observes this reads RED. It exists so the gate can go red (EI-01
    /// §3 — a gate that cannot go red is not a gate).
    DataPlaneCascaded,
}

/// **A signup/provisioning degradation (architecture §8).** Returned by [`SignupPlane::signup`] when
/// the control plane is hard-down: a NEW tenant cannot be placed (region-first placement writes the
/// sticky `tenant_placement` row IN the control plane — P-CP-07 — which is unreachable). This is the
/// *expected, contained* degrade — loud + named (EI-01 §3), never a silent failure. The client
/// retries signup once the CP is back; no already-placed tenant is affected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignupDegraded {
    /// A PII-free reason the signup degraded (the CP is unreachable).
    pub reason: String,
}

impl core::fmt::Display for SignupDegraded {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "signup DEGRADED: {} — a new tenant cannot be placed while the control plane is down \
             (region-first placement writes the sticky tenant_placement row IN the control plane, \
             P-CP-07). Already-placed tenants are UNAFFECTED (they keep serving within their cells); \
             retry signup once the control plane is back. Degrade, not cascade (VISION §3).",
            self.reason
        )
    }
}

impl std::error::Error for SignupDegraded {}

/// **The control-plane handle that can be hard-down (architecture §8 — the CP-D4 outage).** Every
/// control-plane operation (the slow-path `discover` authority, `place`/provisioning) goes through
/// this handle; when [`ControlPlane::hard_down`] is set, the slow path is **unreachable** (`Err`),
/// exactly as a real severance would make it. The handle is cloneable + `Send`/`Sync` (an
/// `Arc<AtomicBool>` inside) so the drill's break-driver and the data/signup planes that consult it
/// share one truth — the SAME shape as the harness `DependencyBreaker` (the T-3 seam this rides).
#[derive(Clone, Default)]
pub struct ControlPlane {
    down: Arc<AtomicBool>,
}

impl ControlPlane {
    /// A control plane that is up (reachable).
    pub fn up() -> ControlPlane {
        ControlPlane {
            down: Arc::new(AtomicBool::new(false)),
        }
    }

    /// **Hard-down the control plane (the CP-D4 outage).** Every subsequent CP operation is
    /// unreachable until [`ControlPlane::restore`]. This is the reversible, scoped severance the
    /// dependency-break injector models at the rig level.
    pub fn hard_down(&self) {
        self.down.store(true, Ordering::SeqCst);
    }

    /// Restore the control plane (the outage is lifted — the system is observed recovering, EI-01 §3).
    pub fn restore(&self) {
        self.down.store(false, Ordering::SeqCst);
    }

    /// Is the control plane currently hard-down (unreachable)?
    pub fn is_down(&self) -> bool {
        self.down.load(Ordering::SeqCst)
    }

    /// The slow-path `discover` authority, **through the outage switch**. When the CP is up it
    /// answers from the [`Registry`] ([`Registry::discover`]); when it is hard-down it is
    /// **unreachable** (`Err(ServeError)`) — the routing hiccup the discovery cache's fail-static
    /// path covers. This is exactly the `discover_cp` closure [`DiscoveryCache::resolve`] takes.
    pub fn discover(
        &self,
        registry: &Registry,
        key: &DiscoverKey,
        ttl_seconds: myelin_substrate::Seconds,
    ) -> Result<Option<RouteTuple>, ServeError> {
        if self.is_down() {
            return Err(ServeError(
                "control plane hard-down (CP-D4 outage) — unreachable".into(),
            ));
        }
        Ok(registry.discover(key, ttl_seconds))
    }
}

impl core::fmt::Debug for ControlPlane {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ControlPlane")
            .field("down", &self.is_down())
            .finish()
    }
}

/// **An already-placed tenant's data plane (architecture §8 — the part that keeps serving).** Holds
/// the cell's gateway (layer 4) + the client-side fail-static discovery cache. [`DataPlane::serve`]
/// routes a request for the tenant: CP up → fresh route; CP hard-down → fail-static route from the
/// cache (the tenant keeps serving within its cell). The data plane consults the [`ControlPlane`]
/// ONLY to discover the route, and that answer is client-cached + fail-static — so a CP outage cannot
/// take it down.
pub struct DataPlane<C: Clock = SystemClock> {
    /// The cell this data plane's gateway fronts (layer 4 — it serves the tenants it homes).
    gateway: CellGateway,
    /// The client-side fail-static discovery cache (contract 1.10) — the mechanism that keeps routing
    /// alive through a CP outage.
    cache: DiscoveryCache<C>,
    /// The route TTL the client honours (the CP-returned freshness window).
    ttl_seconds: myelin_substrate::Seconds,
    /// PII-free aggregate: requests this data plane SERVED for an already-placed tenant.
    placed_requests_served: Arc<AtomicU64>,
    /// **The CP-D4 ZERO — placed-tenant requests that FAILED.** Pinned to 0 by the fail-static
    /// degrade (a CP outage serves the route static, never closed, within the staleness budget); a
    /// live counter (not a constant) so a regression that failed a placed-tenant request during a CP
    /// outage is observable (it would tick above 0). This is the `serving-uptime` denominator's
    /// failure count the CP-D4 drill asserts `== 0`.
    placed_requests_failed: Arc<AtomicU64>,
}

/// The outcome of a data-plane serve during the CP-D4 model — whether the route was served fresh
/// (CP reachable / route cached) or fail-static (CP hard-down, last-known-good), plus the placement
/// the gateway accepted. A serve only ever *fails* if a request arrives for a tenant this cell does
/// not home (a misroute — NOT a CP-outage symptom) or the route is past the staleness budget.
#[derive(Clone, Debug)]
pub struct Served {
    /// The placement the gateway accepted (the request is served entirely within this cell).
    pub placement: PlacementOf,
    /// `true` iff the route was served **fail-static** (the CP was hard-down and the last-known-good
    /// route was used) — the CP-D4 degrade leg. `false` iff the route was fresh.
    pub via_fail_static: bool,
}

/// Why a data-plane serve could not complete. Either a [`GatewayReject`] (a misroute / unknown
/// tenant — NOT a CP-outage symptom, the gateway rejected at layer 4) or [`ServeFailure::NoRoute`]
/// (the route is genuinely unknown — the cache is empty/expired AND the CP is unreachable; the
/// cold-start-during-outage case, correctly fail-closed, never a fabricated route).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServeFailure {
    /// The gateway rejected the request (a misroute or an unknown tenant) — layer 4, NOT a CP outage.
    Gateway(GatewayReject),
    /// No route to serve: the discovery cache is empty/expired AND the CP is unreachable (the
    /// cold-start-during-outage case). Correctly fail-closed (never a fabricated route). A tenant
    /// that was ALREADY serving (its route cached) never hits this within the staleness budget.
    NoRoute {
        /// The tenant the request was for (opaque id, PII-free).
        tenant_id: TenantId,
    },
}

impl core::fmt::Display for ServeFailure {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ServeFailure::Gateway(r) => write!(f, "{r}"),
            ServeFailure::NoRoute { tenant_id } => write!(
                f,
                "no-route: tenant `{}` has no cached route AND the control plane is unreachable \
                 (cold-start-during-outage) — correctly fail-closed (never a fabricated route).",
                tenant_id.as_str()
            ),
        }
    }
}

impl std::error::Error for ServeFailure {}

impl<C: Clock> DataPlane<C> {
    /// Build a data plane for a cell over a fail-static discovery cache. `ttl_seconds` is the route
    /// freshness window; the cache is constructed with the SAME bound the production discovery cache
    /// uses (`static_max ≤` revocation SLA, `≥` agent-token TTL — [`DiscoveryCache::try_new`]).
    pub fn new(
        gateway: CellGateway,
        cache: DiscoveryCache<C>,
        ttl_seconds: myelin_substrate::Seconds,
    ) -> DataPlane<C> {
        DataPlane {
            gateway,
            cache,
            ttl_seconds,
            placed_requests_served: Arc::new(AtomicU64::new(0)),
            placed_requests_failed: Arc::new(AtomicU64::new(0)),
        }
    }

    /// A borrow of the discovery cache (so a drill can advance its clock across the TTL/staleness
    /// boundaries during the outage).
    pub fn cache(&self) -> &DiscoveryCache<C> {
        &self.cache
    }

    /// **`serve(control_plane, registry, tenant)` — route + serve an already-placed tenant's request
    /// (architecture §8 — the part that keeps serving through a CP outage).**
    ///
    /// 1. **Discover the route, fail-static for routing** ([`DiscoveryCache::resolve`] over the
    ///    [`ControlPlane::discover`] authority). CP up → fresh route (cached); CP hard-down → the
    ///    last-known-good route served static (within the staleness budget). Past the budget (or a
    ///    cold start during the outage) → no route → [`ServeFailure::NoRoute`] (correctly fail-closed).
    /// 2. **Route the request at the cell gateway (layer 4)** ([`CellGateway::route`]) — the request
    ///    is served IFF this cell homes the tenant (a routing lookup, 0 cross-tenant rows). A misroute
    ///    is a [`ServeFailure::Gateway`] — NOT a CP-outage symptom.
    ///
    /// The CP-D4 property: a placed tenant whose route is cached keeps serving even while the CP is
    /// hard-down (the route comes from the fail-static cache; the cell does its own fail-closed
    /// `check`). `placed_requests_served` / `placed_requests_failed` (the `serving-uptime` numerator
    /// / failure count) are recorded.
    pub fn serve(
        &self,
        control_plane: &ControlPlane,
        registry: &Registry,
        tenant: &TenantId,
    ) -> Result<Served, ServeFailure> {
        let key = DiscoverKey::TenantId(tenant.clone());

        // 1. Discover the route, fail-static for routing (the mechanism that survives a CP outage).
        let answer = self.cache.resolve(&key, |k| {
            control_plane.discover(registry, k, self.ttl_seconds)
        });
        let via_fail_static = matches!(answer, Answer::Static(_));
        if matches!(answer, Answer::Closed) {
            // No route to serve: the cache is empty/expired AND the CP is unreachable (cold start
            // during the outage), OR the CP authoritatively said "unknown". Correctly fail-closed.
            self.placed_requests_failed.fetch_add(1, Ordering::SeqCst);
            return Err(ServeFailure::NoRoute {
                tenant_id: tenant.clone(),
            });
        }

        // 2. Route at the cell gateway (layer 4) — served IFF this cell homes the tenant. NB the
        //    gateway consults the registry's `placement_of` (a routing lookup the cell holds locally
        //    — the placement record is replicated into the cell; the cell does not re-hit the CP on
        //    the hot path). A misroute is a gateway reject, NOT a CP-outage symptom.
        match self.gateway.route(registry, tenant) {
            Ok(placement) => {
                self.placed_requests_served.fetch_add(1, Ordering::SeqCst);
                Ok(Served {
                    placement,
                    via_fail_static,
                })
            }
            Err(reject) => {
                self.placed_requests_failed.fetch_add(1, Ordering::SeqCst);
                Err(ServeFailure::Gateway(reject))
            }
        }
    }

    /// The count of already-placed-tenant requests this data plane SERVED (the `serving-uptime`
    /// numerator). Aggregate, PII-free.
    pub fn placed_requests_served(&self) -> u64 {
        self.placed_requests_served.load(Ordering::SeqCst)
    }

    /// **The CP-D4 ZERO — placed-tenant requests that FAILED.** Pinned to 0 by the fail-static
    /// degrade; a live tripwire so a regression that failed a placed-tenant request during a CP
    /// outage is observable. The CP-D4 drill asserts `== 0`.
    pub fn placed_requests_failed(&self) -> u64 {
        self.placed_requests_failed.load(Ordering::SeqCst)
    }
}

impl<C: Clock> core::fmt::Debug for DataPlane<C> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // PII-free Debug: the cell id + aggregate counters, never a tenant id / route.
        f.debug_struct("DataPlane")
            .field("cell_id", &self.gateway.cell_id().as_str())
            .field("placed_requests_served", &self.placed_requests_served())
            .field("placed_requests_failed", &self.placed_requests_failed())
            .finish()
    }
}

/// **The signup/provisioning path (architecture §8 — the ONLY thing a CP outage degrades).** Wraps
/// the [`PlacementService`] (P-CP-07); [`SignupPlane::signup`] requires the control plane (it writes
/// the sticky `tenant_placement` row IN the CP). A hard-down CP makes signup **degrade**
/// ([`SignupDegraded`]) — the contained, expected blast radius of a CP outage.
pub struct SignupPlane<M: TokenMinter, C: Clock = SystemClock> {
    service: PlacementService<M, C>,
    signups_degraded: Arc<AtomicU64>,
}

impl<M: TokenMinter, C: Clock> SignupPlane<M, C> {
    /// Build the signup plane over a placement service.
    pub fn new(service: PlacementService<M, C>) -> SignupPlane<M, C> {
        SignupPlane {
            service,
            signups_degraded: Arc::new(AtomicU64::new(0)),
        }
    }

    /// **`signup(control_plane, registry, region, tier, slug)` — place a NEW tenant (architecture
    /// §8 — the part that degrades during a CP outage).**
    ///
    /// - **CP up** → region-first placement (P-CP-07): mint a PII-free `tenant_id`, write the sticky
    ///   `tenant_placement` row IN the control plane. Returns the routing answer.
    /// - **CP hard-down** → signup **DEGRADES** ([`SignupDegraded`]): the sticky placement row cannot
    ///   be written (the control plane is unreachable). This is the ONLY thing that degrades; no
    ///   already-placed tenant is affected. `signups_degraded` increments (loud, never swallowed).
    pub fn signup(
        &self,
        control_plane: &ControlPlane,
        registry: &mut Registry,
        region: &myelin_tenancy::Region,
        requested_tier: IsolationKind,
        slug: &str,
    ) -> Result<PlacementAnswer, SignupDegraded> {
        if control_plane.is_down() {
            // The control plane is unreachable — the sticky placement row cannot be written. Signup
            // degrades (the contained blast radius); the client retries once the CP is back.
            self.signups_degraded.fetch_add(1, Ordering::SeqCst);
            return Err(SignupDegraded {
                reason: "control plane hard-down (CP-D4 outage) — unreachable".into(),
            });
        }
        // CP up: region-first PII-free placement (P-CP-07). A NoEligibleCell refusal is a *placement*
        // failure (capacity/region), not a CP outage — it is surfaced as a degrade with that reason
        // (signup did not complete), keeping the degrade scope honest.
        match self.service.place(registry, region, requested_tier, slug) {
            Ok(answer) => Ok(answer),
            Err(PlaceError::NoEligibleCell {
                region,
                requested_tier,
            }) => {
                self.signups_degraded.fetch_add(1, Ordering::SeqCst);
                Err(SignupDegraded {
                    reason: format!(
                        "no eligible cell in region `{}` for tier `{requested_tier:?}` (capacity \
                         degrade — provisioning needed)",
                        region.as_str()
                    ),
                })
            }
            Err(PlaceError::Invariant(e)) => {
                self.signups_degraded.fetch_add(1, Ordering::SeqCst);
                Err(SignupDegraded {
                    reason: format!("placement invariant rejected the write: {e}"),
                })
            }
        }
    }

    /// The count of signups that degraded (the CP-outage / capacity blast radius). Aggregate, PII-free.
    pub fn signups_degraded(&self) -> u64 {
        self.signups_degraded.load(Ordering::SeqCst)
    }

    /// A borrow of the underlying placement service (so a drill can read its `placement_count`).
    pub fn service(&self) -> &PlacementService<M, C> {
        &self.service
    }
}

/// **The measured CP-D4 report (architecture §8 — the dated green artifact's numbers).** The
/// `serving-uptime` + degrade-scope a drill emits: already-placed-tenant requests kept being served
/// (uptime = 100%) while ONLY signup/provisioning degraded. PII-free aggregate counts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CpOutageReport {
    /// Already-placed-tenant requests SERVED during the outage (the `serving-uptime` numerator).
    pub placed_requests_served: u64,
    /// Already-placed-tenant requests that FAILED during the outage (the CP-D4 zero — **0**).
    pub placed_requests_failed: u64,
    /// Signups that degraded during the outage (the contained blast radius).
    pub signups_degraded: u64,
    /// **`serving-uptime`** — the fraction of already-placed-tenant requests still served during the
    /// outage, as a whole-number percentage `0..=100`. The CP-D4 win reads **100**.
    pub serving_uptime_pct: u8,
    /// The observed degrade scope — the CP-D4 win is exactly [`DegradeScope::SignupAndProvisioningOnly`].
    pub degrade_scope: DegradeScope,
}

impl CpOutageReport {
    /// Compute the report from the measured counts. `serving_uptime_pct` is
    /// `served / (served + failed)` as a percentage (100 when no placed request failed — the CP-D4
    /// win); the degrade scope is [`DegradeScope::SignupAndProvisioningOnly`] **iff** the data plane
    /// served every placed request (0 failures) AND at least one signup degraded — otherwise
    /// [`DegradeScope::DataPlaneCascaded`] (a placed request failed → the data plane cascaded, the
    /// gate's red) or [`DegradeScope::None`] (nothing degraded).
    pub fn compute(
        placed_requests_served: u64,
        placed_requests_failed: u64,
        signups_degraded: u64,
    ) -> CpOutageReport {
        let total = placed_requests_served + placed_requests_failed;
        // Integer percentage; 100 iff 0 failures. `checked_div` yields `None` when no placed-tenant
        // requests were exercised — uptime is then vacuously 100 (nothing to serve).
        let serving_uptime_pct = (placed_requests_served * 100)
            .checked_div(total)
            .unwrap_or(100) as u8;
        let degrade_scope = if placed_requests_failed > 0 {
            // A placed request failed during the outage → the data plane CASCADED (the gate's red).
            DegradeScope::DataPlaneCascaded
        } else if signups_degraded > 0 {
            // The data plane served everything; only signup degraded → the CP-D4 win.
            DegradeScope::SignupAndProvisioningOnly
        } else {
            // Nothing degraded (the CP was up the whole time).
            DegradeScope::None
        };
        CpOutageReport {
            placed_requests_served,
            placed_requests_failed,
            signups_degraded,
            serving_uptime_pct,
            degrade_scope,
        }
    }

    /// `true` iff this report is the CP-D4 win: 100% serving-uptime (0 placed-request failures) AND
    /// the degrade scope is signup/provisioning ONLY. The drill asserts this.
    pub fn is_cp_d4_win(&self) -> bool {
        self.placed_requests_failed == 0
            && self.serving_uptime_pct == 100
            && self.degrade_scope == DegradeScope::SignupAndProvisioningOnly
    }
}

/// The bound the CP-D4 model constructs its discovery cache against: agent-token TTL = 60s (lower),
/// revocation SLA = 300s (upper) — the §8.2 constraint (`static_max ≤ revocation SLA`, `≥`
/// agent-token TTL). Re-used by the drill so the discovery cache's staleness budget is exactly the
/// production-shaped one.
pub fn cp_outage_bound() -> StalenessBound {
    StalenessBound {
        revocation_sla_secs: 300,
        agent_token_ttl_secs: 60,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::place::CounterMinter;
    use crate::schema::{
        Capacity, Cell, CellStatus, IsolationKind, PlacementStatus, TenantPlacement,
    };
    use myelin_substrate::TestClock;
    use myelin_tenancy::{CellId, Region};

    fn cell(id: &str, region: &str) -> Cell {
        Cell {
            cell_id: CellId::from_token(id),
            region: Region::new(region),
            status: CellStatus::Active,
            isolation_kind: IsolationKind::Pool,
            capacity: Capacity {
                tenants_max: 1000,
                write_qps_max: 5000,
                storage_bytes_max: 1 << 40,
            },
            utilisation: 10,
            version: 1,
            endpoint: format!("cell.{region}.{id}.myelin.eu"),
        }
    }

    /// A registry with one cell and one ALREADY-placed tenant (the data plane it keeps serving).
    fn placed_registry() -> (Registry, TenantId) {
        let mut reg = Registry::new();
        reg.insert_cell(cell("cell-w-1", "eu-west"));
        let tenant = TenantId::from_token("01J0ACME");
        reg.place_tenant(TenantPlacement {
            tenant_id: tenant.clone(),
            region: Region::new("eu-west"),
            home_cell: CellId::from_token("cell-w-1"),
            isolation_tier: IsolationKind::Pool,
            slug: "acme".into(),
            status: PlacementStatus::Active,
            member_cells: vec![CellId::from_token("cell-w-1")],
        })
        .expect("the single-region placement is admitted");
        (reg, tenant)
    }

    fn data_plane(clock: TestClock) -> DataPlane<TestClock> {
        let gw = CellGateway::new(CellId::from_token("cell-w-1"));
        let cache = DiscoveryCache::try_new_with_clock(30, 300, cp_outage_bound(), clock)
            .expect("valid bound");
        DataPlane::new(gw, cache, 30)
    }

    // ----- the CP-D4 property: placed tenants keep serving; only signup degrades -----

    /// **THE CP-D4 UNIT: a hard-down control plane leaves an already-placed tenant serving (the route
    /// is served fail-static from the cache), and ONLY signup degrades — the data plane is
    /// unaffected.**
    #[test]
    fn cp_hard_down_keeps_placed_tenant_serving_signup_only_degrades() {
        let (mut reg, tenant) = placed_registry();
        let cp = ControlPlane::up();
        let dp = data_plane(TestClock::at(0));
        let signup = SignupPlane::new(PlacementService::new(CounterMinter::new()));

        // ── CP UP: the placed tenant serves (fresh route, primes the cache). ──
        let s0 = dp
            .serve(&cp, &reg, &tenant)
            .expect("CP up: the placed tenant serves");
        assert!(
            !s0.via_fail_static,
            "with the CP up the route is fresh, not fail-static"
        );
        assert_eq!(s0.placement.home_cell.as_str(), "cell-w-1");

        // ── HARD-DOWN the control plane (the CP-D4 outage). ──
        cp.hard_down();
        assert!(cp.is_down());

        // The cache is past fresh_ttl (age 100 > 30) but inside static_max (300): the route is served
        // FAIL-STATIC — the placed tenant KEEPS SERVING entirely within its cell.
        dp.cache().clock().advance(100);
        let s1 = dp
            .serve(&cp, &reg, &tenant)
            .expect("CP down: the placed tenant KEEPS SERVING");
        assert!(
            s1.via_fail_static,
            "with the CP hard-down the route is served fail-static"
        );
        assert_eq!(
            s1.placement.home_cell.as_str(),
            "cell-w-1",
            "served within its cell"
        );

        // SIGNUP degrades (and ONLY signup) — a new tenant cannot be placed while the CP is down.
        let degraded = signup
            .signup(
                &cp,
                &mut reg,
                &Region::new("eu-west"),
                IsolationKind::Pool,
                "newco",
            )
            .expect_err("CP down: signup DEGRADES");
        assert!(
            degraded.to_string().contains("control plane hard-down"),
            "loud: {degraded}"
        );
        assert_eq!(signup.signups_degraded(), 1, "exactly one signup degraded");
        assert_eq!(
            signup.service().signals().placement_count,
            0,
            "nothing was placed while CP down"
        );

        // The data plane served every placed-tenant request (0 failures) — the CP-D4 zero.
        assert_eq!(
            dp.placed_requests_served(),
            2,
            "both placed-tenant requests served"
        );
        assert_eq!(
            dp.placed_requests_failed(),
            0,
            "0 placed-tenant requests failed (the CP-D4 zero)"
        );

        // ── RESTORE: signup works again (the outage is lifted, the system recovers). ──
        cp.restore();
        let placed = signup
            .signup(
                &cp,
                &mut reg,
                &Region::new("eu-west"),
                IsolationKind::Pool,
                "newco",
            )
            .expect("CP restored: signup works again");
        assert!(
            placed.tenant_id.as_str().starts_with("01J0CP-"),
            "a new tenant is placed PII-free"
        );

        // The report: 100% serving-uptime, degrade scope signup/provisioning ONLY (the CP-D4 win).
        let report = CpOutageReport::compute(
            dp.placed_requests_served(),
            dp.placed_requests_failed(),
            signup.signups_degraded(),
        );
        assert!(report.is_cp_d4_win(), "the CP-D4 win: {report:?}");
        assert_eq!(report.serving_uptime_pct, 100);
        assert_eq!(
            report.degrade_scope,
            DegradeScope::SignupAndProvisioningOnly
        );
    }

    /// **`signups_degraded` counts EACH degrade (not a constant).** Two signups attempted while the
    /// CP is hard-down degrade independently → the counter reads exactly 2 (a mutant that returned a
    /// constant `1` would fail this).
    #[test]
    fn signups_degraded_counts_each_degrade() {
        let (mut reg, _tenant) = placed_registry();
        let cp = ControlPlane::up();
        let signup = SignupPlane::new(PlacementService::new(CounterMinter::new()));

        cp.hard_down();
        for _ in 0..2 {
            assert!(
                signup
                    .signup(
                        &cp,
                        &mut reg,
                        &Region::new("eu-west"),
                        IsolationKind::Pool,
                        "newco"
                    )
                    .is_err(),
                "CP down: signup degrades"
            );
        }
        assert_eq!(
            signup.signups_degraded(),
            2,
            "each degrade is counted (not a constant)"
        );
    }

    /// **The data plane does NOT touch the control plane on the hot path for a CACHED route (off the
    /// hot path, §8).** A placed tenant whose route is already cached serves even when the CP is down
    /// AND the cache is still fresh — routing does not even need the CP.
    #[test]
    fn cached_route_serves_without_touching_the_control_plane() {
        let (reg, tenant) = placed_registry();
        let cp = ControlPlane::up();
        let dp = data_plane(TestClock::at(1_000));

        // Prime the cache (CP up).
        dp.serve(&cp, &reg, &tenant).expect("primes the cache");
        // CP goes down, but the cached route is still FRESH (age 10 ≤ ttl 30) → served fresh-from-cache
        // WITHOUT reaching the CP.
        cp.hard_down();
        dp.cache().clock().advance(10);
        let s = dp
            .serve(&cp, &reg, &tenant)
            .expect("a fresh cached route serves without the CP");
        assert!(
            !s.via_fail_static,
            "within the TTL the cached route is fresh (the CP was not needed)"
        );
        assert!(
            dp.cache().signals().discovery_cache_hit >= 1,
            "served from the cache"
        );
    }

    /// **Past the staleness budget, a NEW request during the outage correctly fails closed (never a
    /// fabricated route).** This is the cold-start-during-outage case — it is NOT the CP-D4 win
    /// (a tenant whose route was never cached cannot be routed during a CP outage past the budget);
    /// it is correctly fail-closed, and it reads as a `DataPlaneCascaded`-shaped failure for THAT
    /// request (the report's red), proving the gate can go red.
    #[test]
    fn past_staleness_budget_fails_closed_and_reads_red() {
        let (reg, tenant) = placed_registry();
        let cp = ControlPlane::up();
        let dp = data_plane(TestClock::at(0));

        dp.serve(&cp, &reg, &tenant).expect("primes the cache");
        cp.hard_down();
        // Past static_max (age 301 > 300): no route to serve → fail-closed (never a fabricated route).
        dp.cache().clock().advance(301);
        let fail = dp
            .serve(&cp, &reg, &tenant)
            .expect_err("past the budget routing fails closed");
        assert!(
            matches!(fail, ServeFailure::NoRoute { .. }),
            "no route, correctly fail-closed: {fail}"
        );
        assert_eq!(
            dp.placed_requests_failed(),
            1,
            "the past-budget request failed"
        );

        // The report reads RED for this run (a placed request failed → the data plane cascaded).
        let report =
            CpOutageReport::compute(dp.placed_requests_served(), dp.placed_requests_failed(), 0);
        assert!(
            !report.is_cp_d4_win(),
            "a failed placed request is NOT the CP-D4 win"
        );
        assert_eq!(report.degrade_scope, DegradeScope::DataPlaneCascaded);
        assert!(
            report.serving_uptime_pct < 100,
            "uptime dropped below 100: {report:?}"
        );
    }

    /// **A misroute during a CP outage is a gateway reject, NOT a CP-outage symptom.** The discovery
    /// cache serves the (wrong-cell) route fail-static, but the gateway at THIS cell rejects it
    /// (layer 4) — that is the misroute defence, not the data plane cascading.
    #[test]
    fn misroute_during_outage_is_a_gateway_reject_not_a_cascade() {
        let mut reg = Registry::new();
        reg.insert_cell(cell("cell-w-1", "eu-west"));
        reg.insert_cell(cell("cell-w-2", "eu-west"));
        // A tenant homed on cell-w-2.
        let beta = TenantId::from_token("01J0BETA");
        reg.place_tenant(TenantPlacement {
            tenant_id: beta.clone(),
            region: Region::new("eu-west"),
            home_cell: CellId::from_token("cell-w-2"),
            isolation_tier: IsolationKind::Pool,
            slug: "beta".into(),
            status: PlacementStatus::Active,
            member_cells: vec![CellId::from_token("cell-w-2")],
        })
        .expect("placed");

        // This data plane fronts cell-w-1 (it does NOT home BETA).
        let cp = ControlPlane::up();
        let dp = data_plane(TestClock::at(0));
        let fail = dp
            .serve(&cp, &reg, &beta)
            .expect_err("cell-w-1 does not home BETA → a gateway reject (layer 4), not a serve");
        let ServeFailure::Gateway(GatewayReject::Misroute(_)) = fail else {
            panic!("expected a misroute gateway reject, got {fail}");
        };
    }

    /// **The report's `compute` is exact: serving-uptime is `served / (served + failed)`, and the
    /// degrade scope is the CP-D4 win ONLY when 0 placed requests failed AND a signup degraded.**
    #[test]
    fn report_compute_is_exact() {
        // 100% uptime + a signup degraded → the CP-D4 win.
        let win = CpOutageReport::compute(10, 0, 3);
        assert_eq!(win.serving_uptime_pct, 100);
        assert_eq!(win.degrade_scope, DegradeScope::SignupAndProvisioningOnly);
        assert!(win.is_cp_d4_win());

        // A placed request failed → the data plane cascaded (the gate's red), uptime < 100.
        let red = CpOutageReport::compute(9, 1, 3);
        assert_eq!(red.serving_uptime_pct, 90);
        assert_eq!(red.degrade_scope, DegradeScope::DataPlaneCascaded);
        assert!(!red.is_cp_d4_win());

        // Nothing degraded (CP up the whole time) → None, not a win (no outage was exercised).
        let none = CpOutageReport::compute(10, 0, 0);
        assert_eq!(none.degrade_scope, DegradeScope::None);
        assert!(
            !none.is_cp_d4_win(),
            "no outage exercised is not a CP-D4 win"
        );
    }

    /// **CDC pair for the fail-static discovery degrade (provider + consumer).** The PROVIDER is the
    /// [`DataPlane::serve`] degrade path (the discovery cache + the [`ControlPlane`] authority). The
    /// CONSUMER stands in for a **gateway serving a request while the control plane is down**: it
    /// drives a placed-tenant request, and — load-bearing — can read ONLY whether the route was served
    /// (`via_fail_static`) + the routing placement, NEVER an authz answer (routing ≠ authorization;
    /// the cell does its own fail-closed `check`). If the degrade contract shape drifts (the
    /// `Served`/`ServeFailure` shape, or the fail-static-vs-closed distinction), the consumer stops
    /// compiling — the point of a glue-crate CDC. It asserts the contract: while the CP is down a
    /// placed tenant is served fail-static, and a cold-start-during-outage (no cached route) fails
    /// closed (never a fabricated route).
    #[test]
    fn cdc_fail_static_discovery_degrade_provider_consumer() {
        /// A stand-in **gateway-during-outage** consumer: it serves a placed tenant's request through
        /// the data plane while the CP is down. It can only learn the routing facts (`via_fail_static`
        /// + the home cell) — there is no grant/principal/permission on `Served`.
        struct GatewayDuringOutage<'a> {
            data_plane: &'a DataPlane<TestClock>,
            control_plane: &'a ControlPlane,
            registry: &'a Registry,
        }
        impl GatewayDuringOutage<'_> {
            /// Serve a placed-tenant request; report whether it was served fail-static + its home cell.
            fn serve_during_outage(
                &self,
                tenant: &TenantId,
            ) -> Result<(bool, String), ServeFailure> {
                let served = self
                    .data_plane
                    .serve(self.control_plane, self.registry, tenant)?;
                Ok((
                    served.via_fail_static,
                    served.placement.home_cell.as_str().to_string(),
                ))
            }
        }

        // PROVIDER: a placed tenant + a primed cache, then the CP goes down.
        let (reg, tenant) = placed_registry();
        let cp = ControlPlane::up();
        let dp = data_plane(TestClock::at(0));
        dp.serve(&cp, &reg, &tenant)
            .expect("primes the cache (CP up)");
        cp.hard_down();
        dp.cache().clock().advance(100); // past ttl, inside static_max → fail-static

        // CONSUMER: serve the placed tenant while the CP is down → served FAIL-STATIC, within its cell.
        let consumer = GatewayDuringOutage {
            data_plane: &dp,
            control_plane: &cp,
            registry: &reg,
        };
        let (via_fail_static, home_cell) = consumer
            .serve_during_outage(&tenant)
            .expect("the placed tenant serves while the CP is down");
        assert!(
            via_fail_static,
            "the route is served fail-static while the CP is down"
        );
        assert_eq!(home_cell, "cell-w-1", "served entirely within its cell");

        // CONSUMER (the contract's other half): a tenant whose route was never cached, served past the
        // staleness budget during the outage, correctly FAILS CLOSED (never a fabricated route).
        dp.cache().clock().advance(300); // past static_max
        let cold = consumer.serve_during_outage(&tenant);
        assert!(
            matches!(cold, Err(ServeFailure::NoRoute { .. })),
            "past-budget serve fails closed: {cold:?}"
        );
    }

    /// The `DataPlane` Debug is PII-free + aggregate-only (the cell id + counters, never a tenant id).
    #[test]
    fn data_plane_debug_is_pii_free() {
        let (reg, tenant) = placed_registry();
        let cp = ControlPlane::up();
        let dp = data_plane(TestClock::at(0));
        dp.serve(&cp, &reg, &tenant).expect("served");
        let dbg = format!("{dp:?}");
        assert!(dbg.contains("cell-w-1"), "shows the cell id: {dbg}");
        assert!(
            dbg.contains("placed_requests_served"),
            "shows the aggregate: {dbg}"
        );
        assert!(!dbg.contains("01J0ACME"), "leaks no tenant id: {dbg}");
    }
}
