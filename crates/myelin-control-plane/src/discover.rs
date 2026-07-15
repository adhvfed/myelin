//! # `discover(slug | tenant_id)` — PII-free routing, off the hot path, client-cacheable fail-static
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/tenancy-and-control-plane.md`
//! §7.3 (cell discovery — `discover` returns `{cell_id, region, cell_endpoint, ttl_seconds}` keyed by
//! the opaque `tenant_id` or the non-personal `slug`; clients cache with the TTL; **bounded-staleness
//! fail-static for *routing***; a misroute redirect is the correction signal), §4.1 (the `discover`
//! signature, frozen — PII-free routing only, NO authz answer), §8 (the control plane is small,
//! slow-changing, PII-free, and **off the per-request hot path** — discovery is cacheable).
//! Contract-index rows 12.2 (`discover` — tenant-grain here; repo-grain is P-CP-15) + 1.10
//! ([`myelin_substrate::FailStatic`] — the discovery-cache fail-static for *routing*).
//!
//! ## What this prompt (P-CP-06 / P-081) ships
//! 1. **`discover(slug | tenant_id) → RouteTuple`** ([`Registry::discover`]) — the PII-free routing
//!    answer keyed by the opaque [`DiscoverKey::TenantId`] or the non-personal [`DiscoverKey::Slug`].
//!    It reads `tenant_placement` (the home cell + region) JOINed to `cell` (the routing endpoint),
//!    and returns **only** a [`RouteTuple`] `{cell_id, region, cell_endpoint, ttl_seconds}` — never an
//!    authz answer (there is no principal, no permission check, no grant here; routing ≠ authorization).
//! 2. **The client-cacheable, fail-static [`DiscoveryCache`]** — wraps
//!    [`myelin_substrate::FailStatic`] so a client (gateway / git-wire / SDK) caches the route with the
//!    returned TTL and, when the control plane is **unreachable**, serves the last-known-good route
//!    **fail-static for routing** (bounded-staleness, contract 1.10) rather than failing the request
//!    closed. This is the blast-radius win re-confirmed in CP-D4 (P-CP-14): a CP outage leaves
//!    already-placed tenants serving entirely within their cells; only signup/provisioning degrades.
//! 3. **The `discovery_cache_hit` + `misroute_count` telemetry signals**
//!    ([`DiscoverySignals`]) — `discovery_cache_hit` increments on a cache serve (fresh-from-cache or
//!    stale-fail-static); `misroute_count` increments when `discover` resolves a key to **no** route
//!    (an unknown tenant/slug — the gateway's misroute correction signal, architecture §7.3). Both are
//!    PII-free aggregate counters. Observability is part of the pass (EI-01 §3).
//!
//! ## `discover` returns ROUTING, never AUTHORIZATION (the load-bearing distinction)
//! `discover` answers *"which cell hosts this tenant, and at what endpoint?"* — a routing fact. It is
//! NOT `check`/`list_objects` (Identity): it takes no principal, evaluates no relation, and returns no
//! grant. A caller routes the request to the returned `cell_endpoint`; the cell then does its own
//! `authenticate` + `check` against ITS tuples (fail-closed, ADR-03). Conflating routing with
//! authorization would make a routing-cache staleness an authorization staleness — which is exactly
//! why discovery is fail-**static** for routing (availability) while authz stays fail-**closed**
//! (correctness). The [`RouteTuple`] type carries no grant/principal/permission field by construction.
//!
//! ## Off the hot path (architecture §8)
//! Discovery QPS is signup/ops, orders of magnitude below a cell's request rate, because clients cache
//! the route for `ttl_seconds` and re-`discover` only on a TTL expiry or a misroute redirect. The
//! [`DiscoveryCache`] IS that client cache; the registry `discover` is the slow-path authority behind
//! it.
//!
//! ## Floor named (deferred body → filling prompt) — VISION §3 name-your-floors
//! - **The GeoDNS/anycast discovery edge is `[OPEN → P4 (infra)]`** (architecture §7.3) — a latency
//!   optimisation that fronts the PII-free discovery contract with a geo-routed edge.
//!   **v1 is CP-lookup + client cache** (this prompt). The edge is an infra follow-on, NOT a
//!   band-gated engineering unit; the discovery *contract* (the [`RouteTuple`] shape + the client
//!   cache + fail-static) is fully built here and does not change shape when the edge lands.
//!   Recorded in writing (here + the report).
//! - **Repo-grain `discover`/`placement_of(repo)`** is the M3 follow-on **P-CP-15** (C-1); this prompt
//!   is tenant-grain only.
//! - **The full CP-D2 misroute-REJECTION drill** (the gateway rejects — does not proxy — a request for
//!   a tenant it doesn't host) rides `placement_of` + the gateway, **P-CP-08 / P-CP-12**. Here
//!   `discover` ships the `misroute_count` *signal* (the correction-signal counter); the gateway's
//!   reject path is the named follow-on.

use std::sync::atomic::{AtomicU64, Ordering};

use myelin_substrate::{
    Answer, Clock, FailStatic, FailStaticError, Seconds, ServeError, StalenessBound, SystemClock,
};
use myelin_tenancy::{CellId, Region, TenantId};

use crate::registry::Registry;

/// **The `discover` key (architecture §7.3 / §4.1).** A route is looked up by EITHER the opaque
/// `tenant_id` OR the non-personal routing `slug` — both PII-free. There is deliberately no
/// `name`/`email` variant: discovery never takes personal data (the control plane holds none).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum DiscoverKey {
    /// Resolve by the opaque tenant id (the canonical key — the PK of `tenant_placement`).
    TenantId(TenantId),
    /// Resolve by the non-personal routing slug (e.g. `acme`) — a changeable, PII-free label, NOT a
    /// person's name (the slug-PII screening is the `[OPEN — LEGAL]` residual named in P-CP-12).
    Slug(String),
}

impl DiscoverKey {
    /// A short PII-free label for telemetry/trace (the key KIND, never the key value — a slug could
    /// in principle carry a typo'd PII string before screening, so we log only the kind).
    pub fn kind(&self) -> &'static str {
        match self {
            DiscoverKey::TenantId(_) => "tenant_id",
            DiscoverKey::Slug(_) => "slug",
        }
    }
}

/// **The `discover` answer (architecture §7.3 / §4.1; contract 12.2).** A PII-free ROUTING tuple —
/// `{cell_id, region, cell_endpoint, ttl_seconds}`. It carries **no** authz answer: no principal, no
/// permission, no grant. A client routes to `cell_endpoint` and caches the route for `ttl_seconds`
/// (the cell then does its own fail-closed `authenticate`/`check`). Every field is an opaque id /
/// region code / routing host / TTL count — PII-free by construction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteTuple {
    /// The cell that hosts the tenant (opaque id).
    pub cell_id: CellId,
    /// The cell's residency region (the immutable region the tenant is pinned to).
    pub region: Region,
    /// The PII-free routing endpoint (`cell.<region>.myelin.eu`) — a host the client connects to,
    /// never personal data.
    pub cell_endpoint: String,
    /// The client cache TTL in **seconds** (the bounded-staleness freshness window the client honours;
    /// a re-`discover` happens on expiry or a misroute redirect).
    pub ttl_seconds: Seconds,
}

/// **PII-free discovery telemetry (architecture §4.1 / §14; contract 1.8).** Aggregate counters only
/// — `discovery_cache_hit` (a route served from the client cache) and `misroute_count` (a `discover`
/// resolved to NO route — the gateway's misroute correction signal). Observability is part of the
/// pass (EI-01 §3). Never per-subject data.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct DiscoverySignals {
    /// Count of routes served from the client cache (fresh-from-cache or stale-fail-static).
    pub discovery_cache_hit: u64,
    /// Count of `discover` calls that resolved to NO route (an unknown tenant/slug — the misroute
    /// correction signal, architecture §7.3). The full gateway reject path is P-CP-08/P-CP-12.
    pub misroute_count: u64,
}

impl Registry {
    /// **`discover(slug | tenant_id) → RouteTuple` (architecture §7.3 / §4.1; contract 12.2).** The
    /// slow-path routing authority: resolve a [`DiscoverKey`] to the [`RouteTuple`] of the cell that
    /// hosts the tenant, by reading `tenant_placement` (home cell + region) JOINed to `cell` (the
    /// routing endpoint). Returns `None` when the key resolves to no placed tenant (an unknown
    /// tenant/slug — the caller increments `misroute_count`).
    ///
    /// This returns **routing only** — never an authz answer. There is no principal, no permission
    /// evaluation, no grant: a routing fact, by construction (the [`RouteTuple`] type carries no such
    /// field). The `ttl_seconds` the client caches the route for is passed in (the control plane's
    /// configured discovery TTL — the bounded-staleness freshness window).
    ///
    /// `member_cells` is single-element in v1 (the home cell is the route target). The multi-cell
    /// route fan-out is the M5 floor (P-CP-19/P-CP-20).
    pub fn discover(&self, key: &DiscoverKey, ttl_seconds: Seconds) -> Option<RouteTuple> {
        // Resolve the placement row by the key (opaque id or non-personal slug). A slug lookup is a
        // scan in this in-process floor; the live registry indexes `slug` (the driver floor).
        let placement = match key {
            DiscoverKey::TenantId(tenant_id) => self.placement(tenant_id)?,
            DiscoverKey::Slug(slug) => self.placement_by_slug(slug)?,
        };
        // JOIN to the cell inventory for the routing endpoint. A placement can only reference a
        // registered home cell (the placement invariant verified it at write time), so this is
        // expected to hit; if the cell is somehow gone, there is no route to serve (fail to None,
        // never fabricate an endpoint).
        let cell = self.cell(&placement.home_cell)?;
        Some(RouteTuple {
            cell_id: cell.cell_id.clone(),
            region: cell.region.clone(),
            cell_endpoint: cell.endpoint.clone(),
            ttl_seconds,
        })
    }
}

/// **The client-side discovery cache (architecture §7.3; contract 1.10).** Wraps
/// [`FailStatic`] so a client (gateway / git-wire / SDK) caches the [`RouteTuple`] with the route's
/// own `ttl_seconds` as the freshness window and, when the control plane is **unreachable**, serves
/// the last-known-good route **fail-static for routing** (bounded-staleness) rather than failing the
/// request closed.
///
/// Routing degrades, it does not fail closed: a CP outage leaves already-placed tenants serving
/// entirely within their cells (the blast-radius win, CP-D4 / P-CP-14). Past the staleness budget the
/// underlying [`FailStatic`] correctly returns [`Answer::Closed`] — at which point the route is
/// genuinely unknown and the client must re-discover (we never fabricate a route).
///
/// The cache key is the [`DiscoverKey`]; the value is the [`RouteTuple`]. The `discovery_cache_hit`
/// signal increments on every served route (fresh-from-cache or stale-fail-static).
pub struct DiscoveryCache<C: Clock = SystemClock> {
    // Keyed by the REAL [`DiscoverKey`] (compared by `Eq`, never a 64-bit digest) — two distinct
    // keys that collide in a hash land in distinct entries, so a route cached for one tenant/slug
    // can never be served for another (R2.3; the same full-key-comparison invariant the authz path
    // relies on, here for routing).
    inner: FailStatic<DiscoverKey, RouteTuple, C>,
    discovery_cache_hit: AtomicU64,
    misroute_count: AtomicU64,
}

impl<C: Clock> std::fmt::Debug for DiscoveryCache<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Mirrors `FailStatic`'s Debug discipline: print the signal counters + the inner window, but
        // NEVER the cached routes (they are routing facts, kept off the log surface).
        f.debug_struct("DiscoveryCache")
            .field(
                "discovery_cache_hit",
                &self.discovery_cache_hit.load(Ordering::SeqCst),
            )
            .field(
                "misroute_count",
                &self.misroute_count.load(Ordering::SeqCst),
            )
            .field("inner", &self.inner)
            .finish()
    }
}

impl DiscoveryCache<SystemClock> {
    /// Build the discovery cache against the wall clock. `fresh_ttl` is the route's `ttl_seconds`
    /// (the client honours the CP-returned TTL); `static_max` is the bounded-staleness routing budget
    /// (≤ the revocation SLA, ≥ the agent-token TTL — the same §8.2 constraint
    /// [`FailStatic::try_new`] enforces). A violating bound does not construct.
    pub fn try_new(
        fresh_ttl: Seconds,
        static_max: Seconds,
        bound: StalenessBound,
    ) -> Result<Self, FailStaticError> {
        Ok(DiscoveryCache {
            inner: FailStatic::try_new(fresh_ttl, static_max, bound)?,
            discovery_cache_hit: AtomicU64::new(0),
            misroute_count: AtomicU64::new(0),
        })
    }
}

impl<C: Clock> DiscoveryCache<C> {
    /// Build the discovery cache against an injected clock (the boundary drills use a `TestClock`).
    /// Same §8.2 constraint as [`DiscoveryCache::try_new`].
    pub fn try_new_with_clock(
        fresh_ttl: Seconds,
        static_max: Seconds,
        bound: StalenessBound,
        clock: C,
    ) -> Result<Self, FailStaticError> {
        Ok(DiscoveryCache {
            inner: FailStatic::try_new_with_clock(fresh_ttl, static_max, bound, clock)?,
            discovery_cache_hit: AtomicU64::new(0),
            misroute_count: AtomicU64::new(0),
        })
    }

    /// A borrow of the injected clock — the drills advance a `TestClock` across the TTL / staleness
    /// boundaries through this (the production `SystemClock` exposes no mutators, so this leaks no
    /// control over wall time).
    pub fn clock(&self) -> &C {
        self.inner.clock()
    }

    /// **Resolve a route, fail-static for routing (architecture §7.3; contract 1.10).** Calls the
    /// control plane via `discover_cp` (the slow-path authority — typically
    /// [`Registry::discover`] behind a transport); on success caches + returns the fresh route, on a
    /// control-plane hiccup serves the last-known-good route within the bounded-staleness budget, and
    /// past the budget (or with no cached route) returns [`Answer::Closed`] — never a fabricated route,
    /// never fail-open.
    ///
    /// `discover_cp` returns:
    /// - `Ok(Some(route))` — the CP answered with a route (reachable + the tenant is placed).
    /// - `Ok(None)` — the CP answered, but the tenant/slug is **unknown** (a misroute: increment
    ///   `misroute_count`, return [`Answer::Closed`] — there is no route to serve, fail-static is not
    ///   applicable because the CP authoritatively said "no such route").
    /// - `Err(..)` — the CP is **unreachable** (the routing hiccup fail-static covers).
    ///
    /// On any SERVED route (fresh upstream, fresh-from-cache, or stale-fail-static) the
    /// `discovery_cache_hit` signal increments when the route came from the cache (a hiccup served
    /// from cache, or a fresh value served within the TTL from a prior cache stamp).
    pub fn resolve(
        &self,
        key: &DiscoverKey,
        discover_cp: impl Fn(&DiscoverKey) -> Result<Option<RouteTuple>, ServeError>,
    ) -> Answer<RouteTuple> {
        // We track whether the upstream was actually reached this call, so a cache serve (the
        // fail-static path) increments `discovery_cache_hit` while a fresh upstream read does not.
        let reached_upstream = std::cell::Cell::new(false);
        let answer = self.inner.get(key.clone(), || match discover_cp(key) {
            // The CP answered with a route → fresh upstream read (cache it).
            Ok(Some(route)) => {
                reached_upstream.set(true);
                Ok(route)
            }
            // The CP answered "unknown tenant/slug" → a MISROUTE. We must NOT cache a route (there is
            // none) and must NOT fail-static (the CP authoritatively said "no such route"). Surface it
            // as an upstream-reached error so `FailStatic` does not fabricate from a stale cache for a
            // DIFFERENT key — and record the misroute below.
            Ok(None) => {
                reached_upstream.set(true);
                Err(ServeError(
                    "misroute: unknown tenant/slug (no route)".into(),
                ))
            }
            // The CP is unreachable → a routing hiccup (fail-static covers it).
            Err(e) => Err(e),
        });
        match &answer {
            // A fresh route that came from the cache (upstream was NOT reached this call but a cached
            // value within the TTL was served) → a cache hit. A fresh route from a reached upstream is
            // NOT a cache hit (it is an authoritative read).
            Answer::Fresh(_) if !reached_upstream.get() => {
                self.discovery_cache_hit.fetch_add(1, Ordering::SeqCst);
            }
            // A stale-fail-static route is always a cache serve (the CP was unreachable).
            Answer::Static(_) => {
                self.discovery_cache_hit.fetch_add(1, Ordering::SeqCst);
            }
            // Closed with the upstream reached means the CP said "unknown tenant/slug" → a misroute.
            // Closed without reaching the upstream means the staleness budget is spent (no route to
            // serve) — not a misroute, just an expired cache + an unreachable CP.
            Answer::Closed if reached_upstream.get() => {
                self.misroute_count.fetch_add(1, Ordering::SeqCst);
            }
            _ => {}
        }
        answer
    }

    /// A snapshot of the PII-free discovery telemetry (architecture §4.1) — the aggregate
    /// `discovery_cache_hit` and `misroute_count` counters. The smoke leg asserts `discovery_cache_hit`
    /// increments on a cache serve.
    pub fn signals(&self) -> DiscoverySignals {
        DiscoverySignals {
            discovery_cache_hit: self.discovery_cache_hit.load(Ordering::SeqCst),
            misroute_count: self.misroute_count.load(Ordering::SeqCst),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{
        Capacity, Cell, CellStatus, IsolationKind, PlacementStatus, TenantPlacement,
    };
    use myelin_substrate::TestClock;

    /// The bound the discovery-cache drills construct against: agent-token TTL = 60s (lower),
    /// revocation SLA = 300s (upper). The discovery `fresh_ttl` (the route TTL) and `static_max` (the
    /// routing staleness budget) sit inside it.
    fn drill_bound() -> StalenessBound {
        StalenessBound {
            revocation_sla_secs: 300,
            agent_token_ttl_secs: 60,
        }
    }

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
            endpoint: format!("cell.{region}.myelin.eu"),
        }
    }

    fn placed_registry() -> Registry {
        let mut reg = Registry::new();
        reg.insert_cell(cell("cell-w-1", "eu-west"));
        reg.place_tenant(TenantPlacement {
            tenant_id: TenantId::from_token("01J0ACME"),
            region: Region::new("eu-west"),
            home_cell: CellId::from_token("cell-w-1"),
            isolation_tier: IsolationKind::Pool,
            slug: "acme".into(),
            status: PlacementStatus::Active,
            member_cells: vec![CellId::from_token("cell-w-1")],
        })
        .expect("the single-region placement is admitted");
        reg
    }

    // ----- the `discover` smoke leg (architecture §7.3 / §4.1) -----

    /// **`discover(tenant_id)` returns the routing tuple keyed by the opaque id, never an authz
    /// answer.** The tuple is `{cell_id, region, cell_endpoint, ttl_seconds}` — a routing fact.
    #[test]
    fn discover_by_tenant_id_returns_the_routing_tuple() {
        let reg = placed_registry();
        let route = reg
            .discover(&DiscoverKey::TenantId(TenantId::from_token("01J0ACME")), 30)
            .expect("a placed tenant resolves to a route");
        assert_eq!(route.cell_id.as_str(), "cell-w-1");
        assert_eq!(route.region.as_str(), "eu-west");
        assert_eq!(route.cell_endpoint, "cell.eu-west.myelin.eu");
        assert_eq!(route.ttl_seconds, 30);
        // The route carries NO authz field — it is routing only (a type-level proof: there is no
        // `.grant`/`.principal`/`.permission` to read on RouteTuple).
    }

    /// **`discover(slug)` returns the routing tuple keyed by the non-personal slug** — the same
    /// route as the tenant-id lookup (both PII-free keys resolve the same placement).
    #[test]
    fn discover_by_slug_returns_the_same_route() {
        let reg = placed_registry();
        let by_slug = reg
            .discover(&DiscoverKey::Slug("acme".into()), 30)
            .expect("the non-personal slug resolves to a route");
        let by_id = reg
            .discover(&DiscoverKey::TenantId(TenantId::from_token("01J0ACME")), 30)
            .expect("the opaque id resolves to a route");
        assert_eq!(
            by_slug, by_id,
            "the slug and the tenant id resolve the SAME route"
        );
    }

    /// An unknown tenant/slug resolves to NO route (the caller treats this as a misroute).
    #[test]
    fn discover_unknown_key_returns_none() {
        let reg = placed_registry();
        assert!(reg
            .discover(
                &DiscoverKey::TenantId(TenantId::from_token("01J0GHOST")),
                30
            )
            .is_none());
        assert!(reg
            .discover(&DiscoverKey::Slug("ghost".into()), 30)
            .is_none());
    }

    /// **The key KIND is reported PII-free** (the telemetry logs the kind, never the value).
    #[test]
    fn discover_key_kind_is_pii_free() {
        assert_eq!(
            DiscoverKey::TenantId(TenantId::from_token("01J0ACME")).kind(),
            "tenant_id"
        );
        assert_eq!(DiscoverKey::Slug("acme".into()).kind(), "slug");
    }

    // ----- the client cache: TTL + fail-static (architecture §7.3; contract 1.10) -----

    /// **The client cache honours the TTL and serves fresh-from-cache within it** — a second resolve
    /// inside `fresh_ttl` is served from the cache (the CP is not re-hit), incrementing
    /// `discovery_cache_hit`. This is the "off the hot path" property (architecture §8).
    #[test]
    fn cache_serves_fresh_within_ttl_and_increments_cache_hit() {
        let reg = placed_registry();
        let key = DiscoverKey::TenantId(TenantId::from_token("01J0ACME"));
        let cache =
            DiscoveryCache::try_new_with_clock(30, 300, drill_bound(), TestClock::at(1_000))
                .expect("valid bound");

        // First resolve: the CP is reached (fresh upstream read) → NOT a cache hit.
        let cp = |k: &DiscoverKey| Ok(reg.discover(k, 30));
        let a = cache.resolve(&key, cp);
        assert!(a.is_fresh());
        assert_eq!(
            cache.signals().discovery_cache_hit,
            0,
            "the first read is authoritative, not a hit"
        );

        // Within fresh_ttl: serve fresh-from-cache WITHOUT reaching the CP (the CP is now unreachable
        // — but the cached route is still fresh, so routing does not even need the CP). → a cache hit.
        clock_advance(&cache, 10); // age 10 ≤ fresh_ttl 30
        let unreachable = |_: &DiscoverKey| Err(ServeError("control plane unreachable".into()));
        let b = cache.resolve(&key, unreachable);
        assert!(b.is_fresh(), "within the TTL the cached route is fresh");
        assert_eq!(
            cache.signals().discovery_cache_hit,
            1,
            "a fresh-from-cache serve is a cache hit"
        );
    }

    /// **THE FAIL-STATIC ROUTING LEG (architecture §7.3; contract 1.10; the CP-D4 re-confirm seed):**
    /// when the control plane is **unreachable** and the route is past its TTL but inside the
    /// staleness budget, the client serves the last-known-good route **degraded-static** — routing
    /// degrades, it does NOT fail closed. `discovery_cache_hit` increments. Past the budget it
    /// correctly fails closed (never a fabricated route).
    #[test]
    fn cache_serves_fail_static_when_control_plane_unreachable() {
        let reg = placed_registry();
        let key = DiscoverKey::TenantId(TenantId::from_token("01J0ACME"));
        let cache = DiscoveryCache::try_new_with_clock(30, 300, drill_bound(), TestClock::at(0))
            .expect("valid bound");

        // Prime the cache with a fresh route (the CP is reachable once).
        assert!(cache.resolve(&key, |k| Ok(reg.discover(k, 30))).is_fresh());

        // The CP goes hard-down. Past fresh_ttl (age 100) but inside static_max (300): the route is
        // served DEGRADED-STATIC (fail-static for routing) — the cell keeps being routed to.
        clock_advance(&cache, 100);
        let unreachable = |_: &DiscoverKey| Err(ServeError("control plane hard-down".into()));
        let stale = cache.resolve(&key, unreachable);
        assert!(
            stale.is_degraded(),
            "a CP outage serves the route fail-static (degraded), not closed"
        );
        if let Answer::Static(route) = &stale {
            assert_eq!(
                route.cell_id.as_str(),
                "cell-w-1",
                "the last-known-good route is served"
            );
        }
        assert_eq!(
            cache.signals().discovery_cache_hit,
            1,
            "the fail-static serve is a cache hit"
        );

        // Past the staleness budget (age 301 > static_max 300): fail CLOSED — the route is genuinely
        // unknown now and the client must re-discover. NEVER a fabricated route, NEVER fail-open.
        clock_advance(&cache, 201);
        assert!(
            cache.resolve(&key, unreachable).is_closed(),
            "past the budget routing fails closed"
        );
    }

    /// **A misroute (the CP authoritatively says "no such tenant/slug") increments `misroute_count`
    /// and does NOT serve a stale route.** The CP was REACHED and said "unknown" — fail-static does
    /// not apply (we must not route a deleted/unknown tenant to a stale cell). The full gateway reject
    /// path is P-CP-08/P-CP-12; here the signal is the correction-signal counter.
    #[test]
    fn cache_records_a_misroute_when_cp_says_unknown() {
        let reg = placed_registry();
        let key = DiscoverKey::TenantId(TenantId::from_token("01J0GHOST"));
        let cache = DiscoveryCache::try_new_with_clock(30, 300, drill_bound(), TestClock::at(0))
            .expect("valid bound");
        // The CP is reached and answers "unknown" (None) → a misroute, no route served.
        let answer = cache.resolve(&key, |k| Ok(reg.discover(k, 30)));
        assert!(
            answer.is_closed(),
            "an unknown tenant has no route (closed, never fabricated)"
        );
        assert_eq!(
            cache.signals().misroute_count,
            1,
            "the CP-said-unknown case is a misroute"
        );
        assert_eq!(
            cache.signals().discovery_cache_hit,
            0,
            "a misroute is not a cache hit"
        );
    }

    /// A `Closed` answer when the CP is UNREACHABLE and the cache is empty/expired is NOT a misroute
    /// (the CP never said "unknown" — it was simply unreachable past the budget). `misroute_count`
    /// stays 0; this is the cold-start-during-outage case (correctly fail-closed, no route fabricated).
    #[test]
    fn cold_start_during_outage_is_closed_but_not_a_misroute() {
        let key = DiscoverKey::TenantId(TenantId::from_token("01J0ACME"));
        let cache = DiscoveryCache::try_new_with_clock(30, 300, drill_bound(), TestClock::at(0))
            .expect("valid bound");
        // The CP is unreachable from the very first call, no cache to fall back on → Closed, but the
        // CP never authoritatively said "unknown", so it is NOT a misroute.
        let unreachable = |_: &DiscoverKey| Err(ServeError("cp down".into()));
        assert!(cache.resolve(&key, unreachable).is_closed());
        assert_eq!(
            cache.signals().misroute_count,
            0,
            "an unreachable CP is not a misroute"
        );
        assert_eq!(cache.signals().discovery_cache_hit, 0);
    }

    /// The discovery cache enforces the §8.2 staleness bound (≤ revocation SLA) — a routing staleness
    /// budget that outlives the revocation SLA does not construct (a routing degrade must not outlive
    /// the authz-correctness window).
    #[test]
    fn cache_rejects_a_staleness_budget_over_the_revocation_sla() {
        let err = DiscoveryCache::try_new(30, 301, drill_bound())
            .expect_err("static_max 301 > revocation SLA 300 is rejected");
        assert_eq!(
            err,
            FailStaticError::ExceedsRevocationSla {
                static_max: 301,
                revocation_sla: 300
            }
        );
    }

    /// Helper: advance the discovery cache's `TestClock` across a boundary.
    fn clock_advance(cache: &DiscoveryCache<TestClock>, secs: u64) {
        cache.clock().advance(secs);
    }
}
