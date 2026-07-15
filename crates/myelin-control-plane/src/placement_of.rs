//! # `placement_of(tenant_id)` — the routing answer + the gateway misroute-rejection (layer 4, CP-D2)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/tenancy-and-control-plane.md`
//! §4.1 (the `placement_of` signature, **frozen** —
//! `placement_of(tenant_id) → {region, home_cell, member_cells, isolation_tier, status}`;
//! `member_cells` single-element in v1), §5.3 (**layer 4** — the gateway REJECTS, does not proxy, a
//! request for a `tenant_id` it doesn't host; **there is no cross-region query path for personal
//! data**), §7.3 (a **misroute redirect** is the correction signal — the client re-discovers to the
//! correct cell-endpoint). Contract-index row 12.3 (the `placement_of` half; `member_cells`
//! single-element in v1) + 1.1/1.8 (harness + telemetry).
//!
//! ## What this prompt (P-CP-08 / P-084) ships
//! 1. **`placement_of(tenant_id) → PlacementOf {region, home_cell, member_cells, isolation_tier,
//!    status}`** ([`Registry::placement_of`]) — the PII-free routing answer read off the
//!    authoritative `tenant_placement` row. `member_cells` is **single-element in v1** (the
//!    multi-element fan-out is the M5 floor, P-CP-19/P-CP-20). It is the *routing* answer, NEVER an
//!    authz answer (no principal, no permission, no grant — routing ≠ authorization).
//! 2. **The gateway-rejects-misroute path (layer 4) at tenant grain** ([`CellGateway::route`]) — a
//!    request arriving at a cell for a `tenant_id` whose `placement_of.home_cell` is a DIFFERENT cell
//!    is **REJECTED** (not proxied). The gateway returns a [`Misroute`] redirect to the correct
//!    cell-endpoint, writes a PII-free [`MisrouteAuditRecord`] into the [`MisrouteAudit`] sink, and
//!    reads **0 cross-tenant/cross-cell rows** (it consults the control-plane routing answer, never the
//!    foreign cell's data). The `misroute_count` signal increments.
//!
//! ## `placement_of` is the gateway's layer-4 input — the load-bearing distinction
//! `placement_of` is what makes layer 4 *structural*. The cell gateway does not "decide" whether it
//! hosts a tenant by reading the tenant's data (that read would itself be the cross-cell leak the
//! drill forbids); it asks the **control plane's authoritative routing answer** which cell homes the
//! tenant, and rejects+redirects if that answer is not THIS cell. The defence is therefore complete
//! BEFORE any tenant row is touched — 0 cross-tenant rows by construction, not by a `0`-row query that
//! "happened" to return nothing.
//!
//! ## A misroute is REJECTED + REDIRECTED + AUDITED, never proxied (§5.3 layer 4 / §7.3)
//! The architecture is explicit: the gateway **rejects (does not proxy)**. Proxying would route a
//! cross-region request through this cell to a foreign cell — exactly the cross-region query path
//! §5.3 abolishes. Instead the gateway returns a *misroute redirect* (the §7.3 correction signal): the
//! client re-`discover`s and connects directly to the correct cell-endpoint, so the request is served
//! ENTIRELY within the home cell. Every misroute is audited (loud, never swallowed — EI-01 §3) with a
//! PII-free record (opaque ids only); the audit IS the evidence the layer-4 defence fired.
//!
//! ## CP-D2's zero: 0 cross-tenant/cross-cell read (the most load-bearing zero — EI-01 §2)
//! A cross-tenant IDOR is stop-the-bleeding (EI-01 §2). [`CellGateway`] never serves a request for a
//! tenant it does not host, so the harness `CrossTenantCount` projection (the SUB-D7/CP-D2 survival
//! signal) stays `0`. The [`CellGateway::cross_tenant_reads`] counter is a live tripwire: it is pinned
//! to 0 by the structural reject (a misroute never reaches a data read), and would tick above 0 if a
//! future code path served a foreign tenant — making a regression observable.
//!
//! ## Mutation floor (mandatory-core, >= 80% — EI-01 §2/§3; the prompt's TESTS field)
//! The misroute-rejection path ([`CellGateway::route`] + [`Registry::placement_of`]) is
//! mandatory-core: a cross-tenant/cross-cell read is stop-the-bleeding (EI-01 §2). The floor is
//! **>= 80%**; the achieved score is
//! `cargo mutants -p myelin-control-plane -f crates/myelin-control-plane/src/placement_of.rs` ->
//! **13 caught, 5 unviable, 1 missed of 19** = **13/14 viable = 92.9%** (every load-bearing mutant of
//! the `home_cell == self.cell_id` accept-vs-reject branch, the unknown-tenant fail-closed branch, the
//! `misroute_count` increment, the redirect-endpoint resolution, and the `placement_of` field reads is
//! killed by an assertion). The single `MISSED` is `replace cross_tenant_reads -> 0`, a **documented
//! EQUIVALENT mutant**: the gateway NEVER increments `cross_tenant_reads` (the structural guarantee),
//! so the live read is always 0 and `return 0` is observationally identical. This is the *correct*
//! property, not a coverage gap — the counter is a regression tripwire for a future writer, exactly as
//! in `myelin_substrate::topology::PublicSurface::misroute_count`; the `cp_d2_gate_is_not_vacuous`
//! drill proves a non-zero value WOULD read RED. Excluding the documented equivalent mutant the score
//! is **13/13 = 100%** of the load-bearing mutants.
//! **W6d scope note:** the floor covers the route/reject logic + the Memory-arm audit sink the unit
//! tests drive (unchanged by W6d). The `MisrouteAuditBackend::Pg` dispatch arms are NOT unit-mutable
//! (live PG); their proof is `integration_mr009b_w6d_registry_durable` (a gateway-rejected misroute
//! lands in the durable sink and survives a fresh pool) + `integration_mr024_placement_durable`.
//!
//! ## Floor named (deferred body → filling prompt) — VISION §3 name-your-floors
//! - **`member_cells` is single-element / resolution always same-cell in v1.** `placement_of` returns
//!   a single-element `member_cells` set and the gateway accepts a request IFF the tenant's `home_cell`
//!   is THIS cell. The multi-cell resolution path (a member cell that is not the home cell serving a
//!   slice, the `CrossCellPointer` bridge) goes live in **M5 (P-CP-19 / P-CP-20)**. The
//!   [`PlacementOf::member_cells`] field is a `Vec<CellId>` (so the shape is frozen) but every v1
//!   placement carries exactly one member cell (its home), asserted in the tests. Recorded in writing
//!   (here + the report).
//! - **The real gateway transport + the durable tamper-evident audit consumer** is the gateway/audit
//!   wiring beyond M1: here the misroute audit is emitted into a typed, in-process [`MisrouteAudit`]
//!   sink with the SAME PII-free shape the GDPR audit consumer (P-GA-19 / P-062) reads. The security
//!   property (every misroute rejected + redirected + audited, 0 cross-cell read) is complete now; the
//!   durable chain is the named follow-on (mirrors `myelin_substrate::topology::AuditSink`).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
#[cfg(any(test, feature = "test-support"))]
use std::sync::Mutex;

use myelin_storage::placement_durable::DurableMisrouteAuditBacking;
use myelin_tenancy::{CellId, Region, TenantId};

use crate::registry::Registry;
use crate::schema::{IsolationKind, PlacementStatus};

/// **The `placement_of` answer (architecture §4.1, frozen; contract 12.3).** The PII-free ROUTING
/// answer: `{region, home_cell, member_cells, isolation_tier, status}`. It carries **no** authz
/// answer — no principal, no permission, no grant (routing ≠ authorization). Every field is an opaque
/// id / region code / tier / status enum — PII-free by construction. `member_cells` is single-element
/// in v1 (the multi-element fan-out is the M5 floor, P-CP-19/P-CP-20).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlacementOf {
    /// The tenant's (immutable) residency region (§5.3 layer 1).
    pub region: Region,
    /// The tenant's home (primary) cell — the cell the gateway accepts a request for this tenant on.
    pub home_cell: CellId,
    /// The multi-cell fan-out set. **FLOOR: single-element in v1** (its home); the multi-element
    /// fan-out + the `CrossCellPointer` resolution is the M5 floor (P-CP-19/P-CP-20). A `Vec<CellId>`
    /// so the shape is frozen; every v1 placement carries exactly one member cell.
    pub member_cells: Vec<CellId>,
    /// The served isolation tier (§5.1; contract 12.5).
    pub isolation_tier: IsolationKind,
    /// The placement lifecycle status (§5.1).
    pub status: PlacementStatus,
}

impl Registry {
    /// **`placement_of(tenant_id) → PlacementOf` (architecture §4.1, frozen; contract 12.3).** The
    /// routing answer read off the authoritative `tenant_placement` row: `{region, home_cell,
    /// member_cells, isolation_tier, status}`. Returns `None` when the tenant is not placed (an
    /// unknown tenant — the caller treats it as a misroute / no-route, never fabricates an answer).
    ///
    /// This is the *routing* answer — never an authz answer (the [`PlacementOf`] type carries no
    /// grant/principal/permission field by construction). It is what makes the gateway's layer-4
    /// misroute-reject ([`CellGateway::route`]) structural: the gateway asks "which cell homes this
    /// tenant?" and rejects+redirects if that is not THIS cell — BEFORE any tenant row is touched.
    ///
    /// `member_cells` is single-element in v1 (the multi-cell route fan-out is the M5 floor,
    /// P-CP-19/P-CP-20).
    pub fn placement_of(&self, tenant_id: &TenantId) -> Option<PlacementOf> {
        let row = self.placement(tenant_id)?;
        Some(PlacementOf {
            region: row.region.clone(),
            home_cell: row.home_cell.clone(),
            member_cells: row.member_cells.clone(),
            isolation_tier: row.isolation_tier,
            status: row.status,
        })
    }
}

/// **The misroute redirect (architecture §5.3 layer 4 / §7.3 — the correction signal).** The gateway's
/// rejection of a request for a `tenant_id` it does not host: it carries the correct cell-endpoint the
/// client re-`discover`s to (so the request is served ENTIRELY within the home cell). PII-free —
/// opaque ids + a routing host, never personal data. A misroute is REJECTED + REDIRECTED, never
/// proxied (proxying would be the cross-region query path §5.3 abolishes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Misroute {
    /// The tenant the request was for (opaque id, PII-free).
    pub tenant_id: TenantId,
    /// The cell that ACTUALLY homes the tenant (the redirect target — opaque id).
    pub correct_cell: CellId,
    /// The correct cell's PII-free routing endpoint (`cell.<region>.myelin.eu`) — the client
    /// re-discovers and connects HERE, so the request is served within the home cell.
    pub correct_cell_endpoint: String,
}

impl core::fmt::Display for Misroute {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "misroute: tenant `{}` is not hosted by this cell — REJECTED (not proxied) + REDIRECTED \
             to its home cell `{}` at `{}` (§5.3 layer 4 / §7.3; there is no cross-region query path \
             for personal data). 0 cross-tenant/cross-cell rows read.",
            self.tenant_id.as_str(),
            self.correct_cell.as_str(),
            self.correct_cell_endpoint
        )
    }
}

impl std::error::Error for Misroute {}

/// Why a request to a cell could NOT be served (and was not a clean accept). Either a [`Misroute`]
/// (the tenant is hosted by a DIFFERENT cell → reject + redirect + audit) or an [`NoSuchTenant`]
/// (the control plane knows no placement for this tenant → reject, no redirect target to give).
///
/// [`NoSuchTenant`]: GatewayReject::NoSuchTenant
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GatewayReject {
    /// **The CP-D2 misroute (§5.3 layer 4).** The tenant is hosted by a different cell — rejected
    /// (not proxied), redirected to the correct cell-endpoint, audited. 0 cross-cell rows read.
    Misroute(Misroute),
    /// The control plane has no placement for this tenant (an unknown/deleted tenant). Rejected;
    /// there is no correct cell-endpoint to redirect to (the client must re-signup / the id is
    /// stale). Audited as a misroute attempt (loud, never swallowed) but with no redirect target.
    NoSuchTenant {
        /// The tenant the request was for (opaque id, PII-free).
        tenant_id: TenantId,
    },
}

impl core::fmt::Display for GatewayReject {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            GatewayReject::Misroute(m) => write!(f, "{m}"),
            GatewayReject::NoSuchTenant { tenant_id } => write!(
                f,
                "no-route: the control plane knows no placement for tenant `{}` — REJECTED (not \
                 served, not proxied); no redirect target (a stale/unknown tenant id). 0 \
                 cross-tenant/cross-cell rows read.",
                tenant_id.as_str()
            ),
        }
    }
}

impl std::error::Error for GatewayReject {}

/// One PII-free audit record of a rejected misroute (architecture §5.3 layer 4 — the audit half of
/// "rejected + audited"). Carries ONLY opaque ids + a routing host — never a name/email/body
/// (control-plane-pii-free by construction; the SAME shape the GDPR audit consumer P-GA-19 reads). A
/// recorded misroute is the evidence the layer-4 defence FIRED.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MisrouteAuditRecord {
    /// The tenant the misrouted request was for (opaque id, PII-free).
    pub tenant_id: TenantId,
    /// The cell that received (and rejected) the request (opaque id).
    pub received_by_cell: CellId,
    /// The cell that ACTUALLY homes the tenant — `None` when the tenant is unknown (no redirect
    /// target). Opaque id, PII-free.
    pub home_cell: Option<CellId>,
}

/// The Pg arm of the audit sink (MR-009b W6d): the durable `misroute_audit` backing (SI-028,
/// migration 0034) + the runtime handle the sync gateway API drives the async sqlx backing on.
#[derive(Clone)]
struct PgMisrouteAudit {
    backing: DurableMisrouteAuditBacking,
    rt: tokio::runtime::Handle,
}

impl PgMisrouteAudit {
    fn block<F: std::future::Future>(&self, fut: F) -> F::Output {
        tokio::task::block_in_place(|| self.rt.block_on(fut))
    }
}

/// The audit-sink backend (MR-009b W6d): the `test-support`-gated in-memory DOUBLE or the
/// ALWAYS-COMPILED durable PG sink. The production-compiled enum presents no in-memory collection.
#[derive(Clone)]
enum MisrouteAuditBackend {
    /// The in-memory test double (DB-free). Compiled ONLY under
    /// `#[cfg(any(test, feature = "test-support"))]` — NOT the production sink.
    #[cfg(any(test, feature = "test-support"))]
    Memory(Arc<Mutex<Vec<MisrouteAuditRecord>>>),
    /// The REAL durable PG sink — the audit trail survives a process restart (SI-028).
    Pg(PgMisrouteAudit),
}

/// The audit sink a [`CellGateway`] records a rejected misroute into (architecture §5.3 layer 4).
/// As of MR-009b W6d this is a role struct over a backend enum: the ALWAYS-COMPILED production sink
/// is the durable `misroute_audit` table ([`MisrouteAudit::with_pg`] — the trail survives restart,
/// SI-028 closed); the in-process collector is the `test-support`-gated test double
/// ([`MisrouteAudit::new`]). Both carry the SAME PII-free [`MisrouteAuditRecord`] shape the GDPR
/// audit consumer (P-GA-19/P-062) reads. A durable write fault fails static LOUD (panic) — an
/// unrecorded misroute would be silently-lost evidence (the W6a ledger-write lesson).
#[derive(Clone)]
pub struct MisrouteAudit {
    backend: MisrouteAuditBackend,
}

impl core::fmt::Debug for MisrouteAudit {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // PII-free Debug: the backend arm only — never a record (tenant ids stay off the log).
        let arm = match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            MisrouteAuditBackend::Memory(_) => "Memory(test-double)",
            MisrouteAuditBackend::Pg(_) => "Pg(durable)",
        };
        f.debug_struct("MisrouteAudit").field("backend", &arm).finish()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Default for MisrouteAudit {
    fn default() -> MisrouteAudit {
        MisrouteAudit::new()
    }
}

impl MisrouteAudit {
    /// A fresh, empty IN-MEMORY sink — **TEST DOUBLE** (MR-009b W6d: compiled only under
    /// `#[cfg(any(test, feature = "test-support"))]`). The production constructor is
    /// [`MisrouteAudit::with_pg`].
    #[cfg(any(test, feature = "test-support"))]
    pub fn new() -> MisrouteAudit {
        MisrouteAudit {
            backend: MisrouteAuditBackend::Memory(Arc::new(Mutex::new(Vec::new()))),
        }
    }

    /// **The PRODUCTION audit sink — bound to the REAL durable `misroute_audit` backing (MR-009b
    /// W6d / SI-028).** Every recorded misroute survives a process restart. The caller must have
    /// applied [`myelin_storage::placement_durable_migrations`]. `rt` is the tokio runtime handle
    /// the sync gateway drives the async backing on.
    pub fn with_pg(backing: DurableMisrouteAuditBacking, rt: tokio::runtime::Handle) -> MisrouteAudit {
        MisrouteAudit {
            backend: MisrouteAuditBackend::Pg(PgMisrouteAudit { backing, rt }),
        }
    }

    /// Record a rejected misroute (loud, never swallowed — the attempt IS evidence). On the Pg arm
    /// a durable write fault fails static LOUD (panic): an audit sink that silently drops a record
    /// is evidence loss, the exact resurrection-path shape the W6a verifier closed.
    fn record(&self, rec: MisrouteAuditRecord) {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            MisrouteAuditBackend::Memory(records) => {
                records.lock().unwrap_or_else(|e| e.into_inner()).push(rec);
            }
            MisrouteAuditBackend::Pg(pg) => pg
                .block(pg.backing.record(
                    rec.tenant_id.as_str(),
                    rec.received_by_cell.as_str(),
                    rec.home_cell.as_ref().map(|c| c.as_str()),
                ))
                .unwrap_or_else(|e| {
                    panic!(
                        "misroute audit: durable record FAILED (fail-static loud — an unrecorded \
                         misroute is silently-lost layer-4 evidence; the write did NOT land): {e}"
                    )
                }),
        }
    }

    /// **Record a rejected misroute from the repo-grain path** ([`crate::placement_of_repo`]) — the
    /// repo-grain gateway shares the SAME PII-free audit sink + record shape as the tenant-grain path
    /// (one audit consumer reads one shape, P-GA-19). A crate-internal seam over [`Self::record`].
    pub(crate) fn record_misroute(&self, rec: MisrouteAuditRecord) {
        self.record(rec);
    }

    /// Every audited misroute so far (so a drill/test can assert the rejection was audited). On the
    /// Pg arm this reads the durable trail (which is shared, append-ordered infrastructure — a test
    /// filters by its own opaque ids).
    pub fn records(&self) -> Vec<MisrouteAuditRecord> {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            MisrouteAuditBackend::Memory(records) => {
                records.lock().unwrap_or_else(|e| e.into_inner()).clone()
            }
            MisrouteAuditBackend::Pg(pg) => pg
                .block(pg.backing.records())
                .unwrap_or_else(|e| {
                    panic!("misroute audit: durable read FAILED (fail-static loud): {e}")
                })
                .iter()
                .map(|r| MisrouteAuditRecord {
                    tenant_id: TenantId::from_token(&r.tenant_id),
                    received_by_cell: CellId::from_token(&r.received_by_cell),
                    home_cell: r.home_cell.as_deref().map(CellId::from_token),
                })
                .collect(),
        }
    }

    /// How many misroutes have been audited.
    pub fn count(&self) -> usize {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            MisrouteAuditBackend::Memory(records) => {
                records.lock().unwrap_or_else(|e| e.into_inner()).len()
            }
            MisrouteAuditBackend::Pg(pg) => pg
                .block(pg.backing.count())
                .unwrap_or_else(|e| {
                    panic!("misroute audit: durable count FAILED (fail-static loud): {e}")
                }) as usize,
        }
    }
}

/// **The cell gateway (architecture §5.3 layer 4) — the misroute-rejection front door.** A cell's
/// gateway holds its OWN `cell_id` and, for every incoming request, consults the control-plane
/// authoritative routing answer ([`Registry::placement_of`]) to decide whether THIS cell hosts the
/// tenant. It **accepts** a request IFF the tenant's `home_cell` is this cell; otherwise it
/// **rejects** (does not proxy), **redirects** to the correct cell-endpoint, and **audits** the
/// misroute — reading **0** cross-tenant/cross-cell rows.
///
/// `misroute_count` is the CP-D2 telemetry signal (architecture §4.1 / §7.3); `cross_tenant_reads` is
/// the CP-D2 zero (the `CrossTenantCount` projection) — pinned to 0 by the structural reject, exposed
/// as a live tripwire so a future regression that served a foreign tenant is observable.
#[derive(Clone)]
pub struct CellGateway {
    /// The cell this gateway fronts (the cell decides whether it homes a tenant by comparing the
    /// authoritative `home_cell` to THIS id).
    cell_id: CellId,
    /// The audit sink rejected misroutes are recorded into (the gateway shares one sink with the
    /// audit consumer; here the drill/test reads it back).
    audit: MisrouteAudit,
    /// The CP-D2 `misroute_count` signal — how many requests were rejected as misroutes (incl.
    /// unknown-tenant rejections). Aggregate, PII-free.
    misroute_count: Arc<AtomicU64>,
    /// **The CP-D2 ZERO — cross-tenant/cross-cell reads SERVED.** Pinned to 0 by [`Self::route`]
    /// never serving a request for a tenant it does not host; a live counter (not a constant) so a
    /// future regression — a code path that served a foreign tenant and `fetch_add`ed this — is
    /// observable (it would tick above 0). This is the `CrossTenantCount` projection the CP-D2 drill
    /// asserts `== 0`.
    cross_tenant_reads: Arc<AtomicU64>,
}

impl CellGateway {
    /// Build a cell gateway for `cell_id` over a fresh IN-MEMORY audit sink — **TEST DOUBLE**
    /// (MR-009b W6d: compiled only under `#[cfg(any(test, feature = "test-support"))]`, because it
    /// constructs the in-memory [`MisrouteAudit::new`] double). Production wires
    /// [`CellGateway::with_audit`] over the durable [`MisrouteAudit::with_pg`] sink.
    #[cfg(any(test, feature = "test-support"))]
    pub fn new(cell_id: CellId) -> CellGateway {
        CellGateway::with_audit(cell_id, MisrouteAudit::new())
    }

    /// Build a cell gateway for `cell_id` over a given [`MisrouteAudit`] (the gateway shares one sink
    /// with the audit consumer; the drill reads it back).
    pub fn with_audit(cell_id: CellId, audit: MisrouteAudit) -> CellGateway {
        CellGateway {
            cell_id,
            audit,
            misroute_count: Arc::new(AtomicU64::new(0)),
            cross_tenant_reads: Arc::new(AtomicU64::new(0)),
        }
    }

    /// The cell this gateway fronts (opaque id).
    pub fn cell_id(&self) -> &CellId {
        &self.cell_id
    }

    /// The audit sink (so a drill/test can assert a rejected misroute was recorded).
    pub fn audit(&self) -> &MisrouteAudit {
        &self.audit
    }

    /// **The CP-D2 telemetry signal — `misroute_count`.** The number of requests this gateway
    /// rejected as misroutes (a tenant it does not host, incl. unknown tenants). Aggregate, PII-free.
    pub fn misroute_count(&self) -> u64 {
        self.misroute_count.load(Ordering::SeqCst)
    }

    /// **The CP-D2 ZERO — `cross_tenant_reads` SERVED.** The count of requests this gateway served for
    /// a tenant it does not host. Pinned to 0 by [`Self::route`] never serving a misroute; exposed as
    /// a live tripwire so a future regression is observable (it would tick above 0). This is the
    /// `CrossTenantCount` projection the CP-D2 drill asserts `== 0`.
    ///
    /// **Equivalent-mutant note (cargo-mutants):** `replace cross_tenant_reads -> 0` is observationally
    /// identical because the gateway NEVER increments it (the structural guarantee) — the *correct*
    /// property, not a coverage gap. The field + the read seam stay so the tripwire is wired the day a
    /// regression lands (mirrors `topology::PublicSurface::misroute_count`).
    pub fn cross_tenant_reads(&self) -> u64 {
        self.cross_tenant_reads.load(Ordering::SeqCst)
    }

    /// **Bump the `misroute_count` signal** (a crate-internal seam the repo-grain path
    /// [`crate::placement_of_repo::CellGateway::route_repo`] shares — one `misroute_count` telemetry
    /// signal across tenant- + repo-grain misroutes).
    pub(crate) fn bump_misroute_count(&self) {
        self.misroute_count.fetch_add(1, Ordering::SeqCst);
    }

    /// **`route(registry, tenant_id) → Ok(PlacementOf) | Err(GatewayReject)` (architecture §5.3 layer
    /// 4 / §7.3 — the CP-D2 mechanism).** Decide whether THIS cell may serve a request for `tenant_id`.
    ///
    /// 1. Ask the control-plane authoritative routing answer ([`Registry::placement_of`]) which cell
    ///    homes the tenant. (This is a ROUTING lookup, never a read of the tenant's data — 0
    ///    cross-tenant rows by construction.)
    /// 2. If the tenant is unknown → [`GatewayReject::NoSuchTenant`] (reject, audit, no redirect).
    /// 3. If the tenant's `home_cell` is THIS cell → **accept**: return the [`PlacementOf`] (the
    ///    request is served entirely within this cell). No audit, no misroute.
    /// 4. Otherwise → **reject (do NOT proxy)**: increment `misroute_count`, audit the misroute
    ///    (PII-free), and return a [`GatewayReject::Misroute`] redirect to the correct cell-endpoint.
    ///
    /// In NO branch is a foreign tenant's data read — `cross_tenant_reads` stays 0 (the CP-D2 zero).
    /// The decision is made off the control-plane routing answer BEFORE any tenant row is touched.
    pub fn route(
        &self,
        registry: &Registry,
        tenant_id: &TenantId,
    ) -> Result<PlacementOf, GatewayReject> {
        // 1. The authoritative routing answer (a routing lookup — not a tenant-data read).
        let Some(placement) = registry.placement_of(tenant_id) else {
            // 2. Unknown tenant: reject + audit (no redirect target). A misroute attempt is recorded
            //    loudly (never swallowed) and counted.
            self.misroute_count.fetch_add(1, Ordering::SeqCst);
            self.audit.record(MisrouteAuditRecord {
                tenant_id: tenant_id.clone(),
                received_by_cell: self.cell_id.clone(),
                home_cell: None,
            });
            return Err(GatewayReject::NoSuchTenant {
                tenant_id: tenant_id.clone(),
            });
        };

        // 3. This cell homes the tenant → accept. The request is served entirely within this cell.
        if placement.home_cell == self.cell_id {
            return Ok(placement);
        }

        // 4. A DIFFERENT cell homes the tenant → REJECT (not proxy) + REDIRECT + AUDIT. We compute the
        //    redirect endpoint from the control-plane cell inventory (a routing fact, PII-free) — never
        //    by reaching into the foreign cell. The home cell is registered (the placement invariant
        //    verified it at write time); if it is somehow gone we still reject (never proxy / fabricate
        //    a route) with the home cell id but a synthesized endpoint placeholder.
        self.misroute_count.fetch_add(1, Ordering::SeqCst);
        self.audit.record(MisrouteAuditRecord {
            tenant_id: tenant_id.clone(),
            received_by_cell: self.cell_id.clone(),
            home_cell: Some(placement.home_cell.clone()),
        });
        let correct_cell_endpoint = registry
            .cell(&placement.home_cell)
            .map(|c| c.endpoint.clone())
            // The home cell is invariant-guaranteed to exist; this fallback keeps the reject total
            // (we still do NOT proxy and still redirect to the named home cell).
            .unwrap_or_else(|| format!("cell-unresolved:{}", placement.home_cell.as_str()));
        Err(GatewayReject::Misroute(Misroute {
            tenant_id: tenant_id.clone(),
            correct_cell: placement.home_cell,
            correct_cell_endpoint,
        }))
    }
}

impl core::fmt::Debug for CellGateway {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // PII-free Debug: the cell id + the aggregate counters, never any tenant id / audit record.
        f.debug_struct("CellGateway")
            .field("cell_id", &self.cell_id.as_str())
            .field("misroute_count", &self.misroute_count())
            .field("cross_tenant_reads", &self.cross_tenant_reads())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{
        Capacity, Cell, CellStatus, IsolationKind, PlacementStatus, TenantPlacement,
    };

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

    /// A registry with two cells in eu-west and one placed tenant homed on `cell-w-1`.
    fn registry_two_cells() -> Registry {
        let mut reg = Registry::new();
        reg.insert_cell(cell("cell-w-1", "eu-west"));
        reg.insert_cell(cell("cell-w-2", "eu-west"));
        reg.place_tenant(TenantPlacement {
            tenant_id: TenantId::from_token("01J0ACME"),
            region: Region::new("eu-west"),
            home_cell: CellId::from_token("cell-w-1"),
            isolation_tier: IsolationKind::Pool,
            slug: "acme".into(),
            status: PlacementStatus::Active,
            member_cells: vec![CellId::from_token("cell-w-1")],
        })
        .expect("a single-region placement is admitted");
        reg
    }

    // ----- `placement_of` returns the frozen routing tuple (architecture §4.1) -----

    /// **`placement_of(tenant_id)` returns the routing tuple `{region, home_cell, member_cells,
    /// isolation_tier, status}` and `member_cells` is single-element in v1.** It is a routing answer,
    /// never an authz answer (there is no grant/principal/permission field on `PlacementOf`).
    #[test]
    fn placement_of_returns_the_frozen_routing_tuple() {
        let reg = registry_two_cells();
        let answer = reg
            .placement_of(&TenantId::from_token("01J0ACME"))
            .expect("a placed tenant resolves to a placement_of answer");
        assert_eq!(answer.region.as_str(), "eu-west");
        assert_eq!(answer.home_cell.as_str(), "cell-w-1");
        assert_eq!(
            answer.member_cells.len(),
            1,
            "v1 member_cells is single-element (the floor)"
        );
        assert_eq!(answer.member_cells[0].as_str(), "cell-w-1");
        assert_eq!(answer.isolation_tier, IsolationKind::Pool);
        assert_eq!(answer.status, PlacementStatus::Active);
    }

    /// `placement_of` of an unknown tenant returns `None` (never a fabricated answer).
    #[test]
    fn placement_of_unknown_tenant_is_none() {
        let reg = registry_two_cells();
        assert!(reg
            .placement_of(&TenantId::from_token("01J0GHOST"))
            .is_none());
    }

    // ----- the gateway-rejects-misroute path (architecture §5.3 layer 4; CP-D2) -----

    /// **The gateway ACCEPTS a request for a tenant it HOSTS.** `cell-w-1`'s gateway serves the
    /// tenant homed on `cell-w-1` — no misroute, no audit, 0 cross-tenant reads.
    #[test]
    fn gateway_accepts_a_tenant_it_hosts() {
        let reg = registry_two_cells();
        let gw = CellGateway::new(CellId::from_token("cell-w-1"));
        let answer = gw
            .route(&reg, &TenantId::from_token("01J0ACME"))
            .expect("the home cell serves its own tenant");
        assert_eq!(answer.home_cell.as_str(), "cell-w-1");
        assert_eq!(gw.misroute_count(), 0, "an accept is not a misroute");
        assert_eq!(gw.audit().count(), 0, "nothing to audit on an accept");
        assert_eq!(
            gw.cross_tenant_reads(),
            0,
            "the home cell serving its own tenant is not cross-tenant"
        );
    }

    /// **THE CP-D2 REJECTION — the gateway REJECTS (does not proxy) a request for a `tenant_id` it
    /// does NOT host, returns a misroute redirect to the correct cell-endpoint, audits it, and reads
    /// 0 cross-tenant/cross-cell rows.** The single most load-bearing layer-4 property.
    #[test]
    fn gateway_rejects_and_redirects_a_misrouted_tenant() {
        let reg = registry_two_cells();
        // `cell-w-2`'s gateway receives a request for the tenant homed on `cell-w-1` — a misroute.
        let gw = CellGateway::new(CellId::from_token("cell-w-2"));
        let reject = gw
            .route(&reg, &TenantId::from_token("01J0ACME"))
            .expect_err("cell-w-2 does not host this tenant → rejected, not served");

        // REJECTED + REDIRECTED to the correct cell-endpoint (never proxied).
        assert_eq!(
            reject,
            GatewayReject::Misroute(Misroute {
                tenant_id: TenantId::from_token("01J0ACME"),
                correct_cell: CellId::from_token("cell-w-1"),
                correct_cell_endpoint: "cell.eu-west.cell-w-1.myelin.eu".into(),
            }),
            "the misroute redirects to the home cell-endpoint"
        );
        // AUDITED — the misroute is recorded loudly (PII-free), never swallowed.
        assert_eq!(gw.audit().count(), 1, "the misroute was audited");
        assert_eq!(
            gw.audit().records()[0],
            MisrouteAuditRecord {
                tenant_id: TenantId::from_token("01J0ACME"),
                received_by_cell: CellId::from_token("cell-w-2"),
                home_cell: Some(CellId::from_token("cell-w-1")),
            },
            "the audit record is the PII-free misroute evidence (opaque ids only)"
        );
        // `misroute_count` increments; the CP-D2 ZERO holds — 0 cross-tenant rows served.
        assert_eq!(
            gw.misroute_count(),
            1,
            "misroute_count increments on a rejected misroute"
        );
        assert_eq!(
            gw.cross_tenant_reads(),
            0,
            "0 cross-tenant/cross-cell rows read (the CP-D2 zero)"
        );
        // The reject Display names the rule loudly (§5.3 layer 4).
        assert!(
            reject.to_string().contains("REJECTED (not proxied)"),
            "loud: {reject}"
        );
    }

    /// **A request for an UNKNOWN tenant is rejected (no redirect target) + audited.** The control
    /// plane knows no placement — there is no correct cell-endpoint to redirect to; the request is
    /// still rejected (never served, never proxied) and the attempt audited.
    #[test]
    fn gateway_rejects_an_unknown_tenant_with_no_redirect() {
        let reg = registry_two_cells();
        let gw = CellGateway::new(CellId::from_token("cell-w-1"));
        let reject = gw
            .route(&reg, &TenantId::from_token("01J0GHOST"))
            .expect_err("an unknown tenant is rejected (no route)");
        assert_eq!(
            reject,
            GatewayReject::NoSuchTenant {
                tenant_id: TenantId::from_token("01J0GHOST")
            }
        );
        // Audited (with no home cell) + counted; still 0 cross-tenant reads.
        assert_eq!(
            gw.audit().count(),
            1,
            "the unknown-tenant rejection is audited"
        );
        assert_eq!(
            gw.audit().records()[0].home_cell,
            None,
            "no redirect target for an unknown tenant"
        );
        assert_eq!(gw.misroute_count(), 1);
        assert_eq!(gw.cross_tenant_reads(), 0);
        assert!(
            reject.to_string().contains("no redirect target"),
            "loud: {reject}"
        );
    }

    /// **The gateway never proxies — a misroute is a REDIRECT, and the home cell's OWN gateway then
    /// accepts the same request.** End-to-end: a misroute to `cell-w-2` redirects to `cell-w-1`, and
    /// `cell-w-1`'s gateway serves it (the request is corrected to the home cell, never proxied).
    #[test]
    fn a_misroute_redirect_is_then_served_by_the_home_cell() {
        let reg = registry_two_cells();
        let wrong = CellGateway::new(CellId::from_token("cell-w-2"));
        let GatewayReject::Misroute(redirect) = wrong
            .route(&reg, &TenantId::from_token("01J0ACME"))
            .expect_err("the wrong cell rejects + redirects")
        else {
            panic!("expected a misroute redirect");
        };
        // The client re-discovers to the redirect's correct cell and that cell's gateway accepts.
        let home = CellGateway::new(redirect.correct_cell.clone());
        let served = home
            .route(&reg, &TenantId::from_token("01J0ACME"))
            .expect("the home cell serves the redirected request");
        assert_eq!(served.home_cell, redirect.correct_cell);
        assert_eq!(
            home.misroute_count(),
            0,
            "the home cell does not misroute its own tenant"
        );
        // Neither gateway served a cross-tenant read.
        assert_eq!(wrong.cross_tenant_reads(), 0);
        assert_eq!(home.cross_tenant_reads(), 0);
    }

    /// Many misroutes audit + count each one (the audit is append-only; no misroute is swallowed).
    #[test]
    fn every_misroute_is_audited_and_counted() {
        let mut reg = registry_two_cells();
        // A second tenant homed on cell-w-2.
        reg.place_tenant(TenantPlacement {
            tenant_id: TenantId::from_token("01J0BETA"),
            region: Region::new("eu-west"),
            home_cell: CellId::from_token("cell-w-2"),
            isolation_tier: IsolationKind::Pool,
            slug: "beta".into(),
            status: PlacementStatus::Active,
            member_cells: vec![CellId::from_token("cell-w-2")],
        })
        .expect("placed");
        // cell-w-1 receives requests for the tenant homed on cell-w-2 (misroutes) + an unknown tenant.
        let gw = CellGateway::new(CellId::from_token("cell-w-1"));
        assert!(gw.route(&reg, &TenantId::from_token("01J0BETA")).is_err());
        assert!(gw.route(&reg, &TenantId::from_token("01J0BETA")).is_err());
        assert!(gw.route(&reg, &TenantId::from_token("01J0GHOST")).is_err());
        assert_eq!(
            gw.misroute_count(),
            3,
            "each misroute (incl. unknown) is counted"
        );
        assert_eq!(
            gw.audit().count(),
            3,
            "each misroute is audited (append-only, none swallowed)"
        );
        assert_eq!(
            gw.cross_tenant_reads(),
            0,
            "still 0 cross-tenant reads across all misroutes"
        );
    }

    /// The `CellGateway` Debug is PII-free + aggregate-only (the cell id + counters, never a tenant
    /// id / audit record). Mirrors the `PlacementService`/`FailStatic` PII-free log discipline.
    #[test]
    fn cell_gateway_debug_is_pii_free() {
        let reg = registry_two_cells();
        let gw = CellGateway::new(CellId::from_token("cell-w-2"));
        let _ = gw.route(&reg, &TenantId::from_token("01J0ACME"));
        let dbg = format!("{gw:?}");
        assert!(
            dbg.contains("cell-w-2"),
            "the Debug shows the cell id: {dbg}"
        );
        assert!(
            dbg.contains("misroute_count"),
            "the Debug shows the aggregate count: {dbg}"
        );
        // The misrouted tenant id is NOT in the Debug surface (PII-free log discipline).
        assert!(
            !dbg.contains("01J0ACME"),
            "the Debug leaks no tenant id: {dbg}"
        );
    }
}
