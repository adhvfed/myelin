//! # The durable-cell-table PROJECTION for the cross-cell resolver registry (MR-009b W6c-cp).
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/tenancy-and-control-plane.md` §5.1 (the PII-free
//! `cell` table + its routing `endpoint`) / §6.2 (the always-cell-local bridge resolution).
//!
//! This is the PRODUCTION arm of [`crate::cross_cell_bridge::CellResolverRegistry`]: it PROJECTS the
//! durable `cell` table into the live cell-local resolver handles the bridge dispatches to. The
//! registry's AUTHORITY is the durable, PII-free per-cell `endpoint`
//! ([`myelin_storage::placement_durable::DurableCellRow::endpoint`]) — the SAME column `discover`
//! already routes on — NOT any in-memory system-of-record. At boot the projection reads every cell
//! row and, for each, constructs an `Arc<dyn CellLocalResolver>` from the endpoint via a caller-supplied
//! resolver FACTORY (endpoint string → transport-client handle). The map of handles it holds is a
//! process-local CONNECTION CACHE, rebuilt from the durable rows on every boot — never durable state.
//!
//! ## Why a projection, not a net-new table (design decision — reuse, don't duplicate)
//! `cell.endpoint` is the durable system-of-record for each cell's PII-free routing endpoint already
//! (contract 12.3; `discover`'s `RouteTuple.cell_endpoint` reads it). A net-new `cell_resolver_endpoint`
//! table would DUPLICATE that authority and invite drift. So the registry becomes a boot-time PROJECTION
//! of `cell` (the honest shape the W6-grounding pass leaned to): the durable cell endpoints are the
//! authority; the live resolver handles are a connection cache the resolver factory rebuilds at boot.
//!
//! ## Fail LOUD, never a silent empty registry (EI-01 §3)
//! A cell row with an empty/missing endpoint, or one whose factory cannot construct a handle, is a HARD
//! error ([`ProjectionError`]) at boot-projection time — the projection is NOT built with that cell
//! silently dropped (which would degrade its pointers to tombstones with no signal). The composition
//! root (W6d / W3b.4 boot wiring — NOT wired here) exits non-zero on a projection error.
//!
//! Compiled UNCONDITIONALLY (durable-by-default, MR-009b): sqlx is a non-optional dependency of
//! `myelin-storage` post-W1 and this module is async-native (no tokio bridge), so nothing forces a
//! feature gate — and an `integration` gate here would make this, the ONLY durable projection
//! builder, unreachable in production builds (the correct-but-latent shape MR-009b kills; W6c-cp
//! verifier finding). The always-compiled `cross_cell_bridge` seam stays DB-free behind the
//! [`crate::cross_cell_bridge::ResolverProjection`] trait object, exactly as the events
//! `Durable(Arc<dyn …>)` seams keep `myelin-events` DB-free.

use std::collections::HashMap;
use std::sync::Arc;

use myelin_storage::placement_durable::DurablePlacementBacking;
use myelin_tenancy::CellId;

use crate::cross_cell_bridge::{CellLocalResolver, CellResolverRegistry, ResolverProjection};

/// **The resolver FACTORY: a durable cell endpoint → a live cell-local resolver handle.** Given a
/// cell's opaque id + its durable, PII-free routing `endpoint`, construct the `Arc<dyn
/// CellLocalResolver>` the bridge dispatches to (in production: the resilient transport client to that
/// cell's `resolve(ref, viewer, mode)` endpoint). Returns `Err(reason)` if the endpoint is unusable —
/// the projection then fails LOUD (never a silent skip). `Send + Sync` so boot can share it.
pub type ResolverFactory =
    dyn Fn(&CellId, &str) -> Result<Arc<dyn CellLocalResolver>, String> + Send + Sync;

/// The boot-time projection of the durable `cell` table into live resolver handles. Its authority is
/// the durable cell endpoints; the `resolvers` map is a process-local CONNECTION CACHE (live transport
/// handles), rebuilt at boot — not an in-memory system-of-record.
struct ProjectedResolverSet {
    resolvers: HashMap<CellId, Arc<dyn CellLocalResolver>>,
}

impl ResolverProjection for ProjectedResolverSet {
    fn resolver_for(&self, cell: &CellId) -> Option<Arc<dyn CellLocalResolver>> {
        self.resolvers.get(cell).cloned()
    }
}

/// **A boot-projection error (fail LOUD, EI-01 §3).** Distinguishes an unusable cell endpoint / an
/// unresolvable factory result from a durable-read fault, so the boot root (and the verifier) can tell
/// "a cell endpoint was bad" from "the DB was down".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectionError {
    /// A cell row carried an empty/missing routing `endpoint` — refuse the projection (never a silent
    /// empty registry; the cell's pointers must not degrade to tombstones with no signal).
    MissingEndpoint {
        /// The opaque cell id whose endpoint was empty.
        cell_id: String,
    },
    /// The resolver factory could not construct a handle for a cell's endpoint (unresolvable transport).
    Unresolvable {
        /// The opaque cell id.
        cell_id: String,
        /// The factory's loud, named reason.
        why: String,
    },
    /// A durable read of the `cell` table failed (connection/query fault) — the projection was NOT built.
    Db(String),
}

impl core::fmt::Display for ProjectionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ProjectionError::MissingEndpoint { cell_id } => write!(
                f,
                "cross-cell resolver projection REFUSED: cell `{cell_id}` has an empty/missing routing \
                 endpoint — fail loud rather than project a silently-unreachable cell (never a silent \
                 empty registry)"
            ),
            ProjectionError::Unresolvable { cell_id, why } => write!(
                f,
                "cross-cell resolver projection REFUSED: could not construct a resolver handle for cell \
                 `{cell_id}` from its durable endpoint: {why}"
            ),
            ProjectionError::Db(why) => write!(
                f,
                "cross-cell resolver projection FAILED (durable `cell` read error, projection NOT built): \
                 {why}"
            ),
        }
    }
}

impl std::error::Error for ProjectionError {}

impl CellResolverRegistry {
    /// **Build the PRODUCTION registry as a boot-time PROJECTION of the durable `cell` table (MR-009b
    /// W6c-cp).** Reads every durable cell row from `backing` and constructs its live cell-local
    /// resolver handle from the durable, PII-free `endpoint` via `factory`. Fails LOUD
    /// ([`ProjectionError`]) on a missing/empty endpoint or an unresolvable factory result — NEVER a
    /// silent empty registry. Because the authority is the durable rows, re-running this over a FRESH
    /// pool reconstructs the SAME registry (proven in the W6c-cp integration test), which is exactly
    /// what makes the registry durable-by-authority rather than an in-memory store.
    ///
    /// The composition root (W6d / W3b.4 boot wiring — NOT wired here) calls this at boot and hands the
    /// result to [`crate::cross_cell_bridge::CrossCellBridge::new`], exiting non-zero on an `Err`.
    pub async fn project_from_durable_cells(
        backing: &DurablePlacementBacking,
        factory: &ResolverFactory,
    ) -> Result<CellResolverRegistry, ProjectionError> {
        let cells = backing
            .all_cells()
            .await
            .map_err(|e| ProjectionError::Db(e.to_string()))?;

        let mut resolvers: HashMap<CellId, Arc<dyn CellLocalResolver>> = HashMap::new();
        for row in &cells {
            if row.endpoint.trim().is_empty() {
                return Err(ProjectionError::MissingEndpoint {
                    cell_id: row.cell_id.clone(),
                });
            }
            let cell = CellId::from_token(&row.cell_id);
            let handle = factory(&cell, &row.endpoint).map_err(|why| ProjectionError::Unresolvable {
                cell_id: row.cell_id.clone(),
                why,
            })?;
            resolvers.insert(cell, handle);
        }
        Ok(CellResolverRegistry::projected(Arc::new(ProjectedResolverSet {
            resolvers,
        })))
    }
}
