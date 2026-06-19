//! # `place(region, requested_tier)` + two-phase signup (PII born inside the cell)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/tenancy-and-control-plane.md`
//! §7.2 (tenant→cell assignment — **two-phase signup**: region chosen FIRST, identity captured
//! INSIDE the cell; assignment **region-first → isolation-tier-second → capacity-third →
//! stability-always**; placement a **sticky stored fact**, never a hot-path hash), §3.3 (the human
//! tenant name + admin email are born INSIDE the assigned cell, never in the control plane), §4.1
//! (the `place` signature, frozen — `place(region, requested_tier) → {tenant_id, home_cell,
//! isolation_tier, cell_endpoint}`; region immutable; PII-free). Contract-index row 12.3 (the
//! `place` half — region-first, sticky, PII-free) + 12.1 (the partition key) + 1.1/1.8 (harness +
//! telemetry).
//!
//! ## What this prompt (P-CP-07 / P-082) ships
//! 1. **`place(region, requested_tier) → PlacementAnswer {tenant_id, home_cell, isolation_tier,
//!    cell_endpoint}`** ([`Registry::place`]) implementing **two-phase signup**:
//!    - **Phase 1 (control plane, here): region is chosen FIRST**, the `tenant_id` is **minted
//!      PII-free** (an opaque routing token from the injected [`TokenMinter`] — never derived from a
//!      name/email), and the sticky `tenant_placement` row is written through the **HARD placement
//!      invariant** ([`Registry::place_tenant`], P-CP-05) in the chosen region. The placement is
//!      written `Pending` — the cell-local identity capture (phase 2) is not yet complete.
//!    - **Phase 2 (inside the cell, NOT here): the human tenant name + admin email are born INSIDE
//!      the assigned `cell_endpoint`.** `place` returns ONLY the routing answer; the name/email never
//!      pass through this function or reach the control plane (the type system enforces it — `place`
//!      takes no `name`/`email` argument and stores none).
//! 2. **The assignment order** ([`Registry::assign_cell`]) — **region-first** (only cells in the
//!    requested region are candidates), **isolation-tier-second** (only cells whose `isolation_kind`
//!    serves the `requested_tier`), **capacity-third** (only cells with headroom on every dimension),
//!    **stability-always** (deterministic tie-break — the lowest-utilisation eligible cell, then by
//!    opaque id — so the same inputs pick the same cell; placement is a sticky stored fact, never a
//!    re-hash on each call).
//! 3. **The `placement_count` + `provision_latency` telemetry** ([`PlacementSignals`]) — PII-free
//!    aggregate signals (contract 1.8): `placement_count` increments on a successful placement;
//!    `provision_latency` records the wall-clock span of the `place` call (the injected [`Clock`]).
//!
//! ## Two-phase signup is the PII-free floor (the load-bearing distinction)
//! The whole reason `place` is region-first is to keep **zero** personal data in the control plane
//! (architecture §3.3, VISION §3 GDPR-safe-by-construction). `place` mints an opaque `tenant_id` and
//! routes to a cell; the tenant's human name + admin email are captured by the CELL after routing.
//! This module therefore takes **no** name/email argument and the [`crate::schema::TenantPlacement`]
//! row it writes has **no** name/email column (the `control-plane-pii-free` lint, P-CP-04, guards the
//! schema; CP-D1's place leg asserts 0 `is_personal=true` columns reach the write). A misdesign that
//! threaded a name through `place` would not compile against this signature.
//!
//! ## Sticky stored fact, never a hot-path hash (architecture §7.2)
//! Assignment runs ONCE at signup and the result is stored in `tenant_placement`. A subsequent
//! `placement_of`/`discover` reads the stored row — it never re-runs `assign_cell` (which could pick
//! a *different* cell as utilisation shifts). [`Registry::place`] is **idempotent on a re-`place` of
//! an already-placed tenant token**: it returns the EXISTING placement's answer rather than minting a
//! new id or re-assigning (the stickiness proof — a placed tenant always routes to the same cell).
//!
//! ## Floors named (deferred bodies → filling prompt) — VISION §3 name-your-floors
//! - **No new floor here** (the prompt says so explicitly). The on-demand higher isolation tiers
//!   (Bridge/Dedicated) ride **P-CP-10**; the durable provisioning of the placed cell (the
//!   scripted→durable promotion) rides **P-CP-11** (here a cell must already be `Active` to be
//!   eligible — the provisioning *gating* is P-CP-11). The id-mint source is the injectable
//!   [`TokenMinter`] (a deterministic counter in tests; a ULID/routing-token minter in prod — the
//!   concrete prod minter rides the Storage-driver wiring, P-ST-01, exactly like the registry pool).

use std::sync::atomic::{AtomicU64, Ordering};

use myelin_substrate::{Clock, SystemClock};
use myelin_tenancy::{CellId, Region, TenantId};

use crate::registry::{PlacementError, Registry};
use crate::schema::{CellStatus, IsolationKind, PlacementStatus, TenantPlacement};

/// **The `place` answer (architecture §4.1; contract 12.3).** The PII-free routing answer `place`
/// returns: `{tenant_id, home_cell, isolation_tier, cell_endpoint}`. It carries **no** name/email —
/// those are born INSIDE the cell at `cell_endpoint` (two-phase signup, §3.3). Every field is an
/// opaque id / tier / routing host — PII-free by construction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlacementAnswer {
    /// The **PII-free minted** opaque tenant id (the routing token — never derived from a name/email).
    pub tenant_id: TenantId,
    /// The tenant's home cell (in the requested, immutable region).
    pub home_cell: CellId,
    /// The isolation tier the assigned cell serves the tenant at.
    pub isolation_tier: IsolationKind,
    /// The PII-free cell endpoint (`cell.<region>.myelin.eu`) — where phase 2 (identity capture)
    /// happens, INSIDE the cell. A routing host, never personal data.
    pub cell_endpoint: String,
}

/// The reason a `place` call is **refused** (architecture §6/§7.2). Every variant is a placement
/// failure that is *not* an invariant violation (those are [`PlacementError`]); carrying the inputs
/// keeps the refusal loud + named (EI-01 §3 — a refusal is information).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlaceError {
    /// No `Active` cell in the requested region serves the requested isolation tier with headroom
    /// (region-first → tier-second → capacity-third found no eligible cell). Signup degrades loudly;
    /// it never falls back to a cell in a DIFFERENT region (that would break the region pin).
    NoEligibleCell {
        /// The requested (immutable) region no eligible cell was found in.
        region: Region,
        /// The requested isolation tier.
        requested_tier: IsolationKind,
    },
    /// The placement invariant rejected the write (a cross-region/unknown cell slipped through — this
    /// should be impossible given `assign_cell` only returns same-region cells, but the invariant is
    /// the structural backstop and a rejection is surfaced, never swallowed).
    Invariant(PlacementError),
}

impl std::fmt::Display for PlaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlaceError::NoEligibleCell { region, requested_tier } => write!(
                f,
                "place REFUSED: no Active cell in region `{}` serves isolation tier `{:?}` with \
                 headroom (region-first → tier-second → capacity-third found no eligible cell). \
                 Signup degrades; it NEVER places into a different region (the region pin holds).",
                region.as_str(),
                requested_tier
            ),
            PlaceError::Invariant(e) => write!(f, "place REFUSED by the placement invariant: {e}"),
        }
    }
}

impl std::error::Error for PlaceError {}

impl From<PlacementError> for PlaceError {
    fn from(e: PlacementError) -> Self {
        PlaceError::Invariant(e)
    }
}

/// **The PII-free opaque-token minter (architecture §3.2 — a ULID / control-plane routing token,
/// NEVER a name/email/slug).** `place` mints the `tenant_id` through this so the id source is
/// injectable: a deterministic counter in tests (so the assignment + stickiness are reproducible),
/// a ULID/routing-token minter in prod. The contract: the returned token is opaque and PII-free —
/// `place` NEVER derives an id from the tenant's name/email (it does not even receive them; phase 2,
/// inside the cell, captures identity).
pub trait TokenMinter {
    /// Mint a fresh opaque, PII-free tenant token. MUST be unique per call (a collision would
    /// re-route a new tenant onto an existing placement); MUST NOT encode any personal data.
    fn mint(&self) -> TenantId;
}

/// A deterministic monotonic-counter minter (the test/dev minter). Each `mint` returns
/// `01J0CP-<n>` with a strictly increasing `n` — opaque, PII-free, unique per call, and reproducible
/// (so a drill picks the same cell every run). The prod ULID minter is the named Storage-wiring
/// follow-on; this is the deterministic floor the drills run against.
#[derive(Debug, Default)]
pub struct CounterMinter {
    next: AtomicU64,
}

impl CounterMinter {
    /// A fresh minter starting at 0.
    pub fn new() -> CounterMinter {
        CounterMinter::default()
    }
}

impl TokenMinter for CounterMinter {
    fn mint(&self) -> TenantId {
        let n = self.next.fetch_add(1, Ordering::SeqCst);
        // `01J0CP-` is an opaque routing-token prefix (PII-free); `n` is a monotonic counter.
        TenantId::from_token(format!("01J0CP-{n:020}"))
    }
}

/// **PII-free placement telemetry (architecture §4.1 / §14; contract 1.8).** Aggregate counters only
/// — `placement_count` (successful placements) + `provision_latency` (the wall-clock span of the
/// `place` call, in **seconds**, summed; the per-call span is recorded via [`Clock`]). Observability
/// is part of the pass (EI-01 §3). Never per-subject data.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct PlacementSignals {
    /// Count of successful `place` calls (a new tenant placed). Aggregate-only, PII-free.
    pub placement_count: u64,
    /// The summed `provision_latency` across all `place` calls, in **seconds** (the §4.1 latency
    /// signal). Aggregate-only, PII-free.
    pub provision_latency_secs: u64,
}

/// **The placement service (architecture §7.2).** Wraps the [`Registry`] with the id-minter, the
/// injected clock (for `provision_latency`), and the PII-free placement signals. `place` runs the
/// region-first assignment, mints a PII-free id, and writes the sticky `tenant_placement` row through
/// the HARD placement invariant — phase 1 of two-phase signup. Phase 2 (identity capture) happens
/// INSIDE the assigned cell and never touches this service.
pub struct PlacementService<M: TokenMinter = CounterMinter, C: Clock = SystemClock> {
    minter: M,
    clock: C,
    placement_count: AtomicU64,
    provision_latency_secs: AtomicU64,
}

impl<M: TokenMinter> PlacementService<M, SystemClock> {
    /// Build the placement service against the wall clock with the given minter.
    pub fn new(minter: M) -> PlacementService<M, SystemClock> {
        PlacementService {
            minter,
            clock: SystemClock,
            placement_count: AtomicU64::new(0),
            provision_latency_secs: AtomicU64::new(0),
        }
    }
}

impl<M: TokenMinter, C: Clock> PlacementService<M, C> {
    /// Build the placement service against an injected clock (the drills use a `TestClock` to make
    /// `provision_latency` deterministic).
    pub fn with_clock(minter: M, clock: C) -> PlacementService<M, C> {
        PlacementService {
            minter,
            clock,
            placement_count: AtomicU64::new(0),
            provision_latency_secs: AtomicU64::new(0),
        }
    }

    /// A borrow of the injected clock (the drills advance a `TestClock` through this).
    pub fn clock(&self) -> &C {
        &self.clock
    }

    /// **`place(region, requested_tier) → PlacementAnswer` (architecture §7.2 / §4.1; contract
    /// 12.3) — phase 1 of two-phase signup.** Region-first assignment + PII-free id mint + the sticky
    /// `tenant_placement` write through the HARD placement invariant.
    ///
    /// This function takes **NO** name/email and stores **NONE** — the human tenant name + admin
    /// email are born INSIDE the returned `cell_endpoint` (phase 2, architecture §3.3). The `slug` is
    /// a caller-supplied **non-personal** routing label (screened to carry no PII — the slug-PII
    /// screening is the `[OPEN — LEGAL]` residual, P-CP-12); it is NOT a person's name.
    ///
    /// Assignment is region-first → isolation-tier-second → capacity-third → stability-always
    /// ([`Registry::assign_cell`]); the placement is a **sticky stored fact** (a re-`place` of the
    /// same tenant token returns the existing placement, never a re-assignment).
    ///
    /// `provision_latency` (the wall-clock span) + `placement_count` are recorded on success.
    pub fn place(
        &self,
        registry: &mut Registry,
        region: &Region,
        requested_tier: IsolationKind,
        slug: &str,
    ) -> Result<PlacementAnswer, PlaceError> {
        let started = self.clock.now_secs();

        // region-first → isolation-tier-second → capacity-third → stability-always.
        let home_cell = registry
            .assign_cell(region, requested_tier)
            .ok_or_else(|| PlaceError::NoEligibleCell {
                region: region.clone(),
                requested_tier,
            })?;
        let cell_endpoint = registry
            .cell(&home_cell)
            .expect("assign_cell returned a registered cell")
            .endpoint
            .clone();

        // Phase 1: mint a PII-free opaque id and write the sticky placement through the invariant.
        // The placement is `Pending` until phase 2 (the cell-local identity capture) completes.
        let tenant_id = self.minter.mint();
        registry.place_tenant(TenantPlacement {
            tenant_id: tenant_id.clone(),
            region: region.clone(),
            home_cell: home_cell.clone(),
            isolation_tier: requested_tier,
            slug: slug.to_string(),
            status: PlacementStatus::Pending,
            // v1 single-element member set (its home) — the floor (P-CP-19/P-CP-20).
            member_cells: vec![home_cell.clone()],
        })?;

        // Record PII-free telemetry: provision_latency (the span) + placement_count.
        let elapsed = self.clock.now_secs().saturating_sub(started);
        self.provision_latency_secs.fetch_add(elapsed, Ordering::SeqCst);
        self.placement_count.fetch_add(1, Ordering::SeqCst);

        Ok(PlacementAnswer {
            tenant_id,
            home_cell,
            isolation_tier: requested_tier,
            cell_endpoint,
        })
    }

    /// **Re-`place` an already-placed tenant token (the stickiness proof).** A placed tenant always
    /// routes to the SAME cell: this returns the EXISTING placement's answer rather than re-assigning
    /// or minting a new id. It mints NOTHING and increments NO signal (it is a no-op read). This is
    /// the *sticky stored fact, never a hot-path hash* property at the API level — assignment runs
    /// once at signup; thereafter the stored row is authoritative.
    pub fn answer_for(&self, registry: &Registry, tenant_id: &TenantId) -> Option<PlacementAnswer> {
        let row = registry.placement(tenant_id)?;
        let cell_endpoint = registry.cell(&row.home_cell)?.endpoint.clone();
        Some(PlacementAnswer {
            tenant_id: tenant_id.clone(),
            home_cell: row.home_cell.clone(),
            isolation_tier: row.isolation_tier,
            cell_endpoint,
        })
    }

    /// A snapshot of the PII-free placement telemetry (architecture §4.1).
    pub fn signals(&self) -> PlacementSignals {
        PlacementSignals {
            placement_count: self.placement_count.load(Ordering::SeqCst),
            provision_latency_secs: self.provision_latency_secs.load(Ordering::SeqCst),
        }
    }
}

impl<M: TokenMinter, C: Clock> std::fmt::Debug for PlacementService<M, C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // PII-free Debug: the aggregate signals only, never any tenant id / placement.
        f.debug_struct("PlacementService")
            .field("placement_count", &self.placement_count.load(Ordering::SeqCst))
            .field("provision_latency_secs", &self.provision_latency_secs.load(Ordering::SeqCst))
            .finish()
    }
}

impl Registry {
    /// **The cell-assignment algorithm (architecture §7.2): region-first → isolation-tier-second →
    /// capacity-third → stability-always.** Returns the chosen home cell for a new tenant in `region`
    /// at `requested_tier`, or `None` if no `Active` cell is eligible (signup degrades; it NEVER
    /// returns a cell in a different region — the region pin holds).
    ///
    /// 1. **region-first** — only cells whose immutable `region == region` are candidates (a tenant
    ///    is NEVER placed cross-region).
    /// 2. **isolation-tier-second** — only candidates whose `isolation_kind` serves the
    ///    `requested_tier` ([`Registry::serves_tier`]).
    /// 3. **capacity-third** — only candidates with headroom (utilisation < 100; the full capacity
    ///    vector check is the P-CP-10/§7.1 follow-on — the v1 binding dimension is `utilisation`).
    /// 4. **stability-always** — a deterministic tie-break: the lowest-utilisation eligible cell,
    ///    then by opaque `cell_id` (so the same inputs pick the same cell — placement is a sticky,
    ///    reproducible decision, never a per-call re-hash).
    ///
    /// Only `Active` cells are eligible (a `Provisioning`/`Draining` cell accepts no new placements —
    /// the provisioning gate is P-CP-11).
    pub fn assign_cell(&self, region: &Region, requested_tier: IsolationKind) -> Option<CellId> {
        self.cells_in_assignment_order(region, requested_tier)
            .into_iter()
            .next()
            .map(|c| c.cell_id.clone())
    }

    /// The eligible cells in deterministic assignment order (lowest utilisation, then opaque id) —
    /// exposed for the assignment drill to assert the ordering directly.
    pub fn cells_in_assignment_order(
        &self,
        region: &Region,
        requested_tier: IsolationKind,
    ) -> Vec<&crate::schema::Cell> {
        let mut eligible: Vec<&crate::schema::Cell> = self
            .cells_iter()
            // 1. region-first.
            .filter(|c| &c.region == region)
            // (gate) only Active cells accept placements.
            .filter(|c| c.status == CellStatus::Active)
            // 2. isolation-tier-second.
            .filter(|c| Self::serves_tier(c.isolation_kind, requested_tier))
            // 3. capacity-third (v1: utilisation headroom; the full vector is §7.1 / P-CP-10).
            .filter(|c| c.utilisation < 100)
            .collect();
        // 4. stability-always: deterministic order — lowest utilisation, then opaque id.
        eligible.sort_by(|a, b| {
            a.utilisation
                .cmp(&b.utilisation)
                .then_with(|| a.cell_id.as_str().cmp(b.cell_id.as_str()))
        });
        eligible
    }

    /// Whether a cell of `cell_kind` can serve a tenant requesting `requested_tier` (architecture
    /// §7.1 — the three classes map 1:1 to the isolation tier). v1 is exact-match: a Pool tenant goes
    /// to a Pool cell, etc. (the on-demand Bridge/Dedicated provisioning is P-CP-10). Exposed so the
    /// drill can assert the tier-match rule directly.
    pub fn serves_tier(cell_kind: IsolationKind, requested_tier: IsolationKind) -> bool {
        cell_kind == requested_tier
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Capacity, Cell};

    fn cell(id: &str, region: &str, kind: IsolationKind, util: u8, status: CellStatus) -> Cell {
        Cell {
            cell_id: CellId::from_token(id),
            region: Region::new(region),
            status,
            isolation_kind: kind,
            capacity: Capacity { tenants_max: 1000, write_qps_max: 5000, storage_bytes_max: 1 << 40 },
            utilisation: util,
            version: 1,
            endpoint: format!("cell.{region}.{id}.myelin.eu"),
        }
    }

    fn registry_with(cells: impl IntoIterator<Item = Cell>) -> Registry {
        let mut reg = Registry::new();
        for c in cells {
            reg.insert_cell(c);
        }
        reg
    }

    // ----- the `place` smoke leg (architecture §7.2 / §4.1) -----

    /// **THE `place` SMOKE LEG: `place(region, tier)` mints a PII-free `tenant_id`, writes a sticky
    /// `tenant_placement` row in the chosen region through the placement invariant, and increments
    /// `placement_count`.** The returned answer is routing-only (`{tenant_id, home_cell,
    /// isolation_tier, cell_endpoint}`) — no name/email anywhere.
    #[test]
    fn place_mints_pii_free_and_writes_a_sticky_placement() {
        let mut reg = registry_with([cell("cell-w-1", "eu-west", IsolationKind::Pool, 5, CellStatus::Active)]);
        let svc = PlacementService::new(CounterMinter::new());

        let answer = svc
            .place(&mut reg, &Region::new("eu-west"), IsolationKind::Pool, "acme")
            .expect("a placeable region+tier mints + places");

        // The id is opaque + PII-free (an `01J0CP-` routing token — never a name/email).
        assert!(answer.tenant_id.as_str().starts_with("01J0CP-"), "opaque minted id");
        assert_eq!(answer.home_cell.as_str(), "cell-w-1");
        assert_eq!(answer.isolation_tier, IsolationKind::Pool);
        assert_eq!(answer.cell_endpoint, "cell.eu-west.cell-w-1.myelin.eu");

        // The sticky row is stored in the chosen region, Pending (phase 2 captures identity in-cell).
        let row = reg.placement(&answer.tenant_id).expect("the placement is stored");
        assert_eq!(row.region.as_str(), "eu-west");
        assert_eq!(row.home_cell.as_str(), "cell-w-1");
        assert_eq!(row.status, PlacementStatus::Pending, "phase 2 (in-cell identity) not yet done");

        // Telemetry: placement_count increments.
        assert_eq!(svc.signals().placement_count, 1, "placement_count increments on a placement");
    }

    /// **CP-D1 (place leg): the `place` write path declares 0 `is_personal=true` columns / two-phase
    /// signup keeps name/email out of the control plane.** This is a STRUCTURAL proof: `place`'s
    /// signature takes no name/email, and the stored row carries none. We assert via the data-map
    /// over the live registry schema (the same check P-CP-05's holder leg runs) that the schema the
    /// `place` write targets has 0 personal columns.
    #[test]
    fn place_leg_writes_zero_personal_columns() {
        // The data-map over the registry schema the place write targets: 0 is_personal columns.
        crate::holder::assert_no_personal_columns()
            .expect("CP-D1 place leg: the place write path has 0 is_personal=true columns");

        // And a behavioural proof: place a tenant, then confirm the stored row exposes no PII —
        // every field is opaque id / region / tier / non-personal slug / status / member cells.
        let mut reg = registry_with([cell("cell-w-1", "eu-west", IsolationKind::Pool, 5, CellStatus::Active)]);
        let svc = PlacementService::new(CounterMinter::new());
        let answer = svc
            .place(&mut reg, &Region::new("eu-west"), IsolationKind::Pool, "acme")
            .expect("placed");
        let row = reg.placement(&answer.tenant_id).expect("stored");
        // The slug is a non-personal routing label (never a name); there is no name/email field to
        // read on TenantPlacement (a misdesign that added one would fail the control-plane-pii-free
        // lint at compile time — this asserts the runtime row carries only the PII-free slug).
        assert_eq!(row.slug, "acme");
    }

    /// **Assignment honours region-first → tier-second → capacity-third → stability-always.** With
    /// candidates spread across regions/tiers/utilisation, `place` picks the lowest-utilisation
    /// eligible **in-region, right-tier** cell — never a cross-region or wrong-tier cell.
    #[test]
    fn assignment_is_region_first_tier_second_capacity_third_stability_always() {
        let reg = registry_with([
            // WRONG region (must never be chosen even though it is the least utilised).
            cell("cell-n-1", "eu-north", IsolationKind::Pool, 1, CellStatus::Active),
            // right region, WRONG tier.
            cell("cell-w-ded", "eu-west", IsolationKind::Dedicated, 2, CellStatus::Active),
            // right region+tier, higher utilisation.
            cell("cell-w-hi", "eu-west", IsolationKind::Pool, 80, CellStatus::Active),
            // right region+tier, LOWEST utilisation → the stable pick.
            cell("cell-w-lo", "eu-west", IsolationKind::Pool, 20, CellStatus::Active),
            // right region+tier, same low utilisation as -lo → tie broken by opaque id (-lo < -tie).
            cell("cell-w-tie", "eu-west", IsolationKind::Pool, 20, CellStatus::Active),
        ]);

        let chosen = reg
            .assign_cell(&Region::new("eu-west"), IsolationKind::Pool)
            .expect("an eligible cell exists");
        assert_eq!(chosen.as_str(), "cell-w-lo", "lowest-util in-region right-tier, tie by opaque id");

        // The full deterministic order excludes the cross-region + wrong-tier cells entirely.
        let order: Vec<&str> = reg
            .cells_in_assignment_order(&Region::new("eu-west"), IsolationKind::Pool)
            .iter()
            .map(|c| c.cell_id.as_str())
            .collect();
        assert_eq!(order, vec!["cell-w-lo", "cell-w-tie", "cell-w-hi"]);
    }

    /// **region-first is absolute: no eligible IN-REGION cell ⇒ `place` refuses, it does NOT place
    /// cross-region.** Even with a perfectly-eligible cell in another region, signup degrades loudly
    /// rather than break the region pin.
    #[test]
    fn place_refuses_rather_than_cross_region() {
        let mut reg = registry_with([
            // a fine Pool cell, but in the WRONG region.
            cell("cell-n-1", "eu-north", IsolationKind::Pool, 1, CellStatus::Active),
        ]);
        let svc = PlacementService::new(CounterMinter::new());
        let err = svc
            .place(&mut reg, &Region::new("eu-west"), IsolationKind::Pool, "acme")
            .expect_err("no eligible in-region cell ⇒ refused, never cross-region");
        assert_eq!(
            err,
            PlaceError::NoEligibleCell {
                region: Region::new("eu-west"),
                requested_tier: IsolationKind::Pool,
            }
        );
        // Nothing was placed (no PII-free id minted into a wrong-region cell).
        assert_eq!(svc.signals().placement_count, 0);
        assert!(err.to_string().contains("NEVER places into a different region"), "loud: {err}");
    }

    /// **Capacity-third: a FULLY-utilised cell (utilisation == 100) is NOT eligible** — it has no
    /// headroom, so `place` refuses rather than overload it (the capacity gate; the full capacity
    /// vector is §7.1 / P-CP-10). A cell at 99 is eligible; a cell at 100 is not (the boundary).
    #[test]
    fn place_skips_a_fully_utilised_cell() {
        let mut reg = registry_with([
            cell("cell-w-full", "eu-west", IsolationKind::Pool, 100, CellStatus::Active),
        ]);
        let svc = PlacementService::new(CounterMinter::new());
        let err = svc
            .place(&mut reg, &Region::new("eu-west"), IsolationKind::Pool, "acme")
            .expect_err("a fully-utilised cell has no headroom");
        assert!(matches!(err, PlaceError::NoEligibleCell { .. }));
        assert!(
            reg.cells_in_assignment_order(&Region::new("eu-west"), IsolationKind::Pool).is_empty(),
            "a 100%-utilised cell is excluded by capacity-third"
        );

        // A cell at 99 (one short of full) IS eligible — the boundary is strict (< 100).
        reg.insert_cell(cell("cell-w-99", "eu-west", IsolationKind::Pool, 99, CellStatus::Active));
        let chosen = reg.assign_cell(&Region::new("eu-west"), IsolationKind::Pool);
        assert_eq!(chosen.as_ref().map(|c| c.as_str()), Some("cell-w-99"), "a 99% cell still has headroom");
    }

    /// A `Provisioning` (not-yet-`Active`) cell is NOT eligible — the provisioning gate (P-CP-11)
    /// means a tenant is only placed on an `Active` cell.
    #[test]
    fn place_skips_a_non_active_cell() {
        let mut reg = registry_with([
            cell("cell-w-prov", "eu-west", IsolationKind::Pool, 1, CellStatus::Provisioning),
        ]);
        let svc = PlacementService::new(CounterMinter::new());
        let err = svc
            .place(&mut reg, &Region::new("eu-west"), IsolationKind::Pool, "acme")
            .expect_err("a Provisioning cell accepts no placements");
        assert!(matches!(err, PlaceError::NoEligibleCell { .. }));
    }

    /// **Placement is a STICKY stored fact, never a hot-path hash.** After `place`, `answer_for`
    /// returns the SAME cell even when a *lower-utilisation* cell is later added — assignment ran
    /// ONCE at signup; thereafter the stored row is authoritative (a re-hash would have moved it).
    #[test]
    fn placement_is_a_sticky_stored_fact() {
        let mut reg = registry_with([cell("cell-w-hi", "eu-west", IsolationKind::Pool, 70, CellStatus::Active)]);
        let svc = PlacementService::new(CounterMinter::new());
        let answer = svc
            .place(&mut reg, &Region::new("eu-west"), IsolationKind::Pool, "acme")
            .expect("placed on the only cell");
        assert_eq!(answer.home_cell.as_str(), "cell-w-hi");

        // A much-better cell appears AFTER placement. A re-hash would re-route to it; a sticky stored
        // fact does not.
        reg.insert_cell(cell("cell-w-lo", "eu-west", IsolationKind::Pool, 1, CellStatus::Active));
        let sticky = svc.answer_for(&reg, &answer.tenant_id).expect("the placement is sticky");
        assert_eq!(sticky.home_cell.as_str(), "cell-w-hi", "the placed cell is sticky, never re-hashed");
        assert_eq!(sticky, answer, "the same routing answer is returned for the placed tenant");
        // answer_for is a pure read — it mints nothing and increments no signal.
        assert_eq!(svc.signals().placement_count, 1, "answer_for does not re-place");
    }

    /// Each `place` mints a UNIQUE PII-free id (two tenants never collide onto one placement).
    #[test]
    fn each_place_mints_a_unique_id() {
        let mut reg = registry_with([cell("cell-w-1", "eu-west", IsolationKind::Pool, 5, CellStatus::Active)]);
        let svc = PlacementService::new(CounterMinter::new());
        let a = svc.place(&mut reg, &Region::new("eu-west"), IsolationKind::Pool, "acme").expect("a");
        let b = svc.place(&mut reg, &Region::new("eu-west"), IsolationKind::Pool, "beta").expect("b");
        assert_ne!(a.tenant_id, b.tenant_id, "each placement mints a unique opaque id");
        assert_eq!(svc.signals().placement_count, 2);
    }

    /// **`provision_latency` is recorded off the injected clock as the wall-clock span (PII-free
    /// aggregate).** A clock that advances a fixed step on each read makes the span deterministic: the
    /// two `now_secs` reads inside `place` are STEP seconds apart, so the recorded `provision_latency`
    /// is exactly STEP per call (and sums across calls).
    #[test]
    fn provision_latency_is_recorded() {
        /// A clock that advances `step` seconds on EVERY read — so the two reads bracketing the
        /// `place` body are deterministically `step` apart (a deterministic, drillable latency span).
        struct SteppingClock {
            now: AtomicU64,
            step: u64,
        }
        impl Clock for SteppingClock {
            fn now_secs(&self) -> u64 {
                self.now.fetch_add(self.step, Ordering::SeqCst)
            }
        }

        let mut reg = registry_with([
            cell("cell-w-1", "eu-west", IsolationKind::Pool, 5, CellStatus::Active),
            cell("cell-w-2", "eu-west", IsolationKind::Pool, 6, CellStatus::Active),
        ]);
        let clock = SteppingClock { now: AtomicU64::new(1_000), step: 3 };
        let svc = PlacementService::with_clock(CounterMinter::new(), clock);

        svc.place(&mut reg, &Region::new("eu-west"), IsolationKind::Pool, "acme").expect("placed");
        assert_eq!(svc.signals().provision_latency_secs, 3, "one place records a 3s span");

        svc.place(&mut reg, &Region::new("eu-west"), IsolationKind::Pool, "beta").expect("placed");
        assert_eq!(svc.signals().provision_latency_secs, 6, "the span sums across placements");
    }

    /// **The `PlacementService` Debug is PII-free + aggregate-only.** It prints the aggregate
    /// `placement_count` / `provision_latency_secs` signals and NEVER a tenant id / placement (the
    /// PII-free log discipline, mirroring `FailStatic`/`DiscoveryCache`). After a placement the Debug
    /// reflects the count but leaks no minted id.
    #[test]
    fn placement_service_debug_is_pii_free_and_aggregate() {
        let mut reg = registry_with([cell("cell-w-1", "eu-west", IsolationKind::Pool, 5, CellStatus::Active)]);
        let svc = PlacementService::new(CounterMinter::new());
        let answer = svc
            .place(&mut reg, &Region::new("eu-west"), IsolationKind::Pool, "acme")
            .expect("placed");
        let dbg = format!("{svc:?}");
        assert!(dbg.contains("placement_count"), "the Debug shows the aggregate count: {dbg}");
        assert!(dbg.contains("provision_latency_secs"), "the Debug shows the latency aggregate: {dbg}");
        // The minted opaque tenant id is NOT in the Debug surface (PII-free log discipline).
        assert!(
            !dbg.contains(answer.tenant_id.as_str()),
            "the Debug leaks no tenant id: {dbg}"
        );
    }

    /// `serves_tier` is exact-match in v1 (Pool→Pool); the on-demand Bridge/Dedicated provisioning is
    /// P-CP-10.
    #[test]
    fn serves_tier_is_exact_match_in_v1() {
        assert!(Registry::serves_tier(IsolationKind::Pool, IsolationKind::Pool));
        assert!(!Registry::serves_tier(IsolationKind::Pool, IsolationKind::Dedicated));
        assert!(Registry::serves_tier(IsolationKind::Dedicated, IsolationKind::Dedicated));
    }

    // ----- CDC for the `place` half of 12.3 (provider + consumer) -----

    /// **CDC pair for the `place` half of 12.3 (provider + consumer).** The PROVIDER is this crate's
    /// [`PlacementService::place`] minting + writing a sticky placement. The CONSUMER stands in for
    /// the **signup edge** (architecture §4.1) — it calls `place` and routes the new tenant to the
    /// returned `cell_endpoint` to run **phase 2 (identity capture) INSIDE the cell**. Load-bearing:
    /// the consumer can read ONLY the routing answer (`{tenant_id, home_cell, isolation_tier,
    /// cell_endpoint}`) — there is NO way to pass a name/email INTO `place` or read one OUT (two-phase
    /// signup is enforced by the signature). If the `place` answer shape drifts, the consumer stops
    /// compiling — the point of a glue-crate CDC.
    #[test]
    fn cdc_12_3_place_provider_consumer() {
        /// A stand-in **signup edge** consumer: it places a tenant PII-free, then captures identity
        /// INSIDE the cell. It physically cannot give `place` a name/email (the signature forbids it);
        /// the name/email exist only AFTER routing, in the cell-local capture step.
        struct SignupEdge<'a, M: TokenMinter, C: Clock> {
            svc: &'a PlacementService<M, C>,
        }
        impl<M: TokenMinter, C: Clock> SignupEdge<'_, M, C> {
            /// Phase 1: region-first PII-free placement (no name/email passed in).
            fn signup_phase1(
                &self,
                registry: &mut Registry,
                region: &Region,
                tier: IsolationKind,
                slug: &str,
            ) -> Result<PlacementAnswer, PlaceError> {
                self.svc.place(registry, region, tier, slug)
            }
            /// Phase 2: capture identity INSIDE the cell (this is where name/email are born — NOT in
            /// the control plane). Returns the cell endpoint the cell-local capture runs against.
            fn signup_phase2_in_cell<'b>(&self, answer: &'b PlacementAnswer) -> &'b str {
                // The name/email would be written to the cell's OWN identity store here — never the
                // control plane. The consumer only has the routing answer to work from.
                &answer.cell_endpoint
            }
        }

        // PROVIDER.
        let mut reg = registry_with([cell("cell-w-1", "eu-west", IsolationKind::Pool, 5, CellStatus::Active)]);
        let svc = PlacementService::new(CounterMinter::new());
        let edge = SignupEdge { svc: &svc };

        // CONSUMER: phase 1 (PII-free placement), then phase 2 (identity in-cell).
        let answer = edge
            .signup_phase1(&mut reg, &Region::new("eu-west"), IsolationKind::Pool, "acme")
            .expect("the signup edge places the tenant PII-free");
        let in_cell_endpoint = edge.signup_phase2_in_cell(&answer);
        assert_eq!(in_cell_endpoint, "cell.eu-west.cell-w-1.myelin.eu");
        // The control-plane placement carries NO identity — only the PII-free routing record.
        let row = reg.placement(&answer.tenant_id).expect("the routing record is stored");
        assert_eq!(row.status, PlacementStatus::Pending, "identity capture is phase 2, in-cell");
        assert_eq!(row.region.as_str(), "eu-west");
    }
}
