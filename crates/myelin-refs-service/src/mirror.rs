//! The **TE-7 typed-edge mirror discipline** (REF-P14 / P-163; contract 5.5 — OWNED: the vocabulary
//! + the inverse pairing + the `rel_class='lifecycle'` mirror discipline).
//!
//! **Owning architecture doc:** `reference-graph.md` §3.3 (the TE-7 hybrid — lifecycle/semantic edges
//! are **dual-homed**: the source of truth is a typed relation table owned by the authoritative
//! subsystem; Refs holds the **rebuildable projection**; Refs FIXES the `rel` vocabulary, the
//! `rel_class='lifecycle'` mirror discipline, and the **inverse pairing** `blocks↔blocked_by`,
//! `parent↔child`; **on any Refs↔typed-table drift, a scoped reindex re-emits the typed snapshots and
//! Refs reconverges — the typed table always wins**), §3.2 (the `edge` row / `rel_class` seam), §4.1
//! (typed-relation writes are producer #2 — the SAME transaction that writes the typed row emits the
//! typed lifecycle event; Refs' extraction consumer maps it to a `lifecycle`-class edge, **both
//! inverse directions**), §4.7 (reindex-from-source — the TE-7 drift-correction reconverges Refs to
//! the typed table). **Contract-index row 5.5** (TE-7 typed-edge mirror: the lifecycle relation set
//! `closes/blocks/blocked_by/depends_on/parent/assigns/relates`, the inverse pairing, the typed table
//! wins on drift). **External insight:** `01-process-and-quality-doctrine.md` §3 (prove it — the
//! inverse pairing + drift-reconvergence are drilled green, not asserted in prose). **VISION §1** (the
//! reference graph as the connective tissue: cross-subsystem traversal is ONE Refs query, not a
//! five-way fan-out — which is exactly what the lifecycle mirror buys).
//!
//! ## What REF-P14 (P-163) ships — the OWNED half of 5.5
//! Refs does **not** own the typed relation TABLES (Issues `issue_relation`; Knowledge
//! `db_relation`/`page_parent`) — those arrive in R-M3 (KN `page_parent`, REF-P18) and R-M4 (Issues
//! `issue_relation`, REF-P20). Refs owns, and this module ships, the **discipline** the mirror runs
//! under:
//!
//! 1. **The frozen lifecycle `rel` vocabulary** ([`LifecycleRel`]):
//!    `closes`/`blocks`/`blocked_by`/`depends_on`/`parent`/`assigns`/`relates` — the §3.3 / contract
//!    5.5 set, and **nothing else** (an unknown lifecycle token is REJECTED, never guessed — REF-3
//!    "rejects ambiguity"). A `reference`-class rel (`mentions`/`links`/`embeds`) is NOT a lifecycle
//!    rel and is not in this vocabulary (the two classes never alias).
//! 2. **The inverse pairing** ([`LifecycleRel::inverse`]): the §3.3 frozen pairs `blocks↔blocked_by`
//!    and `parent↔child` ([`LifecycleRel::Child`] is the inverse direction of `parent`); `relates` is
//!    **symmetric** (its own inverse, endpoints swapped); `closes`/`depends_on`/`assigns` are
//!    directional with **no frozen inverse token yet** ([`Inverse::None`]) — the forward edge is
//!    mirrored, the inverse is named a FLOOR (the inverse token is the owning subsystem's mint, REF-P18/
//!    REF-P20). Naming the inverse explicitly (rather than inventing `dependency_of`/`closed_by`) keeps
//!    the stored vocabulary unambiguous (REF-3) until the subsystem freezes it.
//! 3. **The `rel_class='lifecycle'` mirror discipline** ([`mirror_edges`]): given ONE typed lifecycle
//!    event `(source, target, rel)`, project **the forward lifecycle edge AND its inverse edge (with
//!    the endpoints swapped)** — so "issue ENG-1 `blocks` ENG-2" yields BOTH a `blocks` edge
//!    (ENG-1→ENG-2) and a `blocked_by` edge (ENG-2→ENG-1), and **cross-subsystem traversal in either
//!    direction is one Refs query** (§3.3). Every projected edge is `rel_class = Lifecycle`.
//! 4. **Drift reconvergence — typed wins** ([`reconverge`]): the typed table is the source of truth;
//!    Refs' projection is rebuildable. When the projection drifts from the typed snapshot (a stale or
//!    spurious lifecycle edge), a scoped reindex re-emits the typed snapshots and Refs **reconverges to
//!    the typed table** — the typed snapshot's edges (forward + inverse) become the live set; any
//!    lifecycle edge NOT in the typed snapshot for that scope is tombstoned (the typed table always
//!    wins, §4.7 / drill REF-D4 TE-7 half). This is the ONE code path the reindex-from-source recovery
//!    drives (REF-P16 wires the full byte-parity reindex; here the TE-7 reconvergence SEMANTICS are
//!    frozen + drilled).
//!
//! ## Floors named (VISION §3 / prompt DoD)
//! - **The producers are SYNTHETIC at M2.** This module is exercised with **synthetic typed events**
//!   ([`SyntheticTypedEvent`]) — there is no real `issue_relation` / `page_parent` table at M2. The
//!   first REAL typed mirrors land in **R-M3 (KN `page_parent`, REF-P18)** and **R-M4 (Issues
//!   `issue_relation`, REF-P20)**. Named so the discipline is **not mistaken for a working mirror over
//!   real tables** — the VOCABULARY + INVERSE PAIRING + RECONVERGENCE are real + drilled here; the
//!   typed TABLES are the subsystems' deliverables.
//! - **The inverse token for `closes`/`depends_on`/`assigns` is a FLOOR** ([`Inverse::None`]): the §3.3
//!   contract freezes only `blocks↔blocked_by` and `parent↔child`; the inverse direction for the other
//!   directional rels is the owning subsystem's mint (REF-P18/REF-P20). Until then the forward edge is
//!   mirrored and the inverse is NOT invented (REF-3 — never guess a token).
//! - **Mutation floor (mandatory-core).** The mirror decision logic — the vocabulary parse (reject
//!   unknown), the inverse pairing (every rel's inverse, incl. symmetric `relates` + the `None`
//!   floor), the both-directions projection (forward + swapped-endpoint inverse, BOTH lifecycle-class),
//!   and the reconverge typed-wins set arithmetic (typed snapshot becomes live; the drifted edge is
//!   tombstoned) — is the mutation-tested core. The floor is stated + met by the unit + chained tests:
//!   a mutant that mis-pairs an inverse, drops the inverse edge, mislabels the class, or lets a drifted
//!   edge survive reconvergence is caught. The world-scale reindex-parity drill (REF-D4 full) is
//!   REF-P16/REF-P24.

use std::collections::HashSet;

use myelin_refs::{strip_sub, ArtifactRef};
use myelin_tenancy::{Region, TenantId};

use crate::edge_builder::{edge_id, EdgeProjection, EdgeRow, RelClass};

/// **The frozen lifecycle relation vocabulary (§3.3 / contract 5.5).** The complete v1 set Refs fixes
/// for the TE-7 mirror discipline — `closes`/`blocks`/`blocked_by`/`depends_on`/`parent`/`assigns`/
/// `relates` — plus [`LifecycleRel::Child`], the inverse direction of `parent` (the §3.3 frozen pair
/// `parent↔child`). An unknown lifecycle token is REJECTED ([`LifecycleRel::parse`] → `None`), never
/// guessed (REF-3 "rejects ambiguity; never guesses scope"). PII-free token. The per-subsystem
/// ENUMERATION + the typed-table columns are the subsystem's deliverable (Issues `issue_relation`,
/// Knowledge `db_relation`/`page_parent`); Refs owns this vocabulary + the inverse pairing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LifecycleRel {
    /// `closes` — a PR/commit closes an issue (directional; no frozen inverse token yet — floor).
    Closes,
    /// `blocks` — this artifact blocks the target (inverse of [`LifecycleRel::BlockedBy`]).
    Blocks,
    /// `blocked_by` — this artifact is blocked by the target (inverse of [`LifecycleRel::Blocks`]).
    BlockedBy,
    /// `depends_on` — this artifact depends on the target (directional; no frozen inverse yet — floor).
    DependsOn,
    /// `parent` — this artifact is the parent of the target (inverse of [`LifecycleRel::Child`]).
    Parent,
    /// `child` — this artifact is the child of the target — the §3.3 inverse direction of `parent`.
    Child,
    /// `assigns` — this artifact assigns the target (directional; no frozen inverse yet — floor).
    Assigns,
    /// `relates` — a symmetric relation (its own inverse, endpoints swapped).
    Relates,
}

/// The inverse of a lifecycle rel under the §3.3 frozen pairing — three shapes the mirror discipline
/// must distinguish (a mutant that collapses them mis-projects the inverse edge).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inverse {
    /// A distinct inverse rel: project an inverse edge of THIS rel with the endpoints swapped
    /// (`blocks`→`blocked_by`, `parent`→`child`, and back).
    Paired(LifecycleRel),
    /// A SYMMETRIC rel (`relates`): the inverse is the same rel with the endpoints swapped (so the
    /// relation is visible from both ends, but the rel token is unchanged).
    Symmetric,
    /// **FLOOR.** A directional rel with NO frozen inverse token yet (`closes`/`depends_on`/`assigns`):
    /// only the forward edge is mirrored; the inverse token is the owning subsystem's mint
    /// (REF-P18/REF-P20). The mirror NEVER invents a token (REF-3).
    None,
}

impl LifecycleRel {
    /// The frozen `rel` column token (§3.2/§3.3 vocabulary). PII-free.
    pub fn as_str(self) -> &'static str {
        match self {
            LifecycleRel::Closes => "closes",
            LifecycleRel::Blocks => "blocks",
            LifecycleRel::BlockedBy => "blocked_by",
            LifecycleRel::DependsOn => "depends_on",
            LifecycleRel::Parent => "parent",
            LifecycleRel::Child => "child",
            LifecycleRel::Assigns => "assigns",
            LifecycleRel::Relates => "relates",
        }
    }

    /// **Parse a lifecycle rel token, REJECTING anything outside the frozen vocabulary (REF-3).** An
    /// unknown token (`"unblocks"`, `"mentions"`, an empty string) → `None`; a `reference`-class rel is
    /// NOT a lifecycle rel and is rejected here (the two classes never alias). This is the validation
    /// chokepoint the mirror runs every typed lifecycle event through.
    pub fn parse(token: &str) -> Option<LifecycleRel> {
        match token {
            "closes" => Some(LifecycleRel::Closes),
            "blocks" => Some(LifecycleRel::Blocks),
            "blocked_by" => Some(LifecycleRel::BlockedBy),
            "depends_on" => Some(LifecycleRel::DependsOn),
            "parent" => Some(LifecycleRel::Parent),
            "child" => Some(LifecycleRel::Child),
            "assigns" => Some(LifecycleRel::Assigns),
            "relates" => Some(LifecycleRel::Relates),
            _ => None,
        }
    }

    /// **The inverse of this rel under the §3.3 frozen pairing.** `blocks↔blocked_by`, `parent↔child`
    /// are the named [`Inverse::Paired`] pairs; `relates` is [`Inverse::Symmetric`];
    /// `closes`/`depends_on`/`assigns` are [`Inverse::None`] (the FLOOR — no frozen inverse token yet).
    /// A mutant that mis-pairs (`blocks`→`parent`) or drops a pair is caught by the inverse-pairing
    /// unit tests.
    pub fn inverse(self) -> Inverse {
        match self {
            LifecycleRel::Blocks => Inverse::Paired(LifecycleRel::BlockedBy),
            LifecycleRel::BlockedBy => Inverse::Paired(LifecycleRel::Blocks),
            LifecycleRel::Parent => Inverse::Paired(LifecycleRel::Child),
            LifecycleRel::Child => Inverse::Paired(LifecycleRel::Parent),
            LifecycleRel::Relates => Inverse::Symmetric,
            LifecycleRel::Closes | LifecycleRel::DependsOn | LifecycleRel::Assigns => Inverse::None,
        }
    }

    /// The complete frozen forward vocabulary the subsystems mint (the 7 contract-5.5 rels; `child` is
    /// the inverse direction of `parent`, projected by the mirror, not minted by a subsystem). Used by
    /// the vocabulary-completeness test (the set is exactly the §3.3 list).
    pub const FORWARD_VOCABULARY: &'static [LifecycleRel] = &[
        LifecycleRel::Closes,
        LifecycleRel::Blocks,
        LifecycleRel::BlockedBy,
        LifecycleRel::DependsOn,
        LifecycleRel::Parent,
        LifecycleRel::Assigns,
        LifecycleRel::Relates,
    ];
}

/// **A synthetic typed lifecycle event** — the M2 stand-in for a real `issue.relation.*` /
/// `knowledge.page.*` event off a typed table (which does not exist until REF-P18/REF-P20). It carries
/// exactly what a real typed lifecycle event carries: the `(source, target, rel)` triple + provenance.
/// PII-free: `source`/`target` are opaque `ArtifactRef` URNs; `origin_actor` is the PSEUDONYMOUS
/// Principal ref (erasure-safe, §4.6). FLOOR: real typed mirrors over real tables land R-M3/R-M4.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntheticTypedEvent {
    /// The referencing side — the typed-relation row's `source` (e.g. issue ENG-1).
    pub source: ArtifactRef,
    /// The referenced side — the typed-relation row's `target` (e.g. issue ENG-2).
    pub target: ArtifactRef,
    /// The lifecycle rel the typed row carries (validated against [`LifecycleRel::parse`]).
    pub rel: LifecycleRel,
    /// The provenance event id (audit) — which typed lifecycle event wrote this.
    pub origin_event: String,
    /// The PSEUDONYMOUS Principal ref that authored the typed write (erasure-safe; never the name).
    pub origin_actor: String,
    /// The consistency token at write time (§4.4).
    pub zookie: Option<String>,
}

/// Why a typed lifecycle event could not be mirrored — an unknown rel token (outside the frozen
/// vocabulary) is a LOUD rejection (REF-3 "rejects ambiguity"), never a silent guess.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MirrorError {
    /// The rel token is not in the frozen lifecycle vocabulary (§3.3) — rejected, never guessed.
    UnknownRel(String),
}

/// **Mirror ONE typed lifecycle event into BOTH inverse-paired `lifecycle`-class edges (§3.3).**
/// Given `(source, target, rel)`, returns the forward lifecycle edge AND its inverse edge (endpoints
/// swapped), so cross-subsystem traversal in either direction is one Refs query. Every returned edge
/// is `rel_class = Lifecycle` and carries the deterministic `edge_id` (idempotent rebuild) + the
/// `#sub`-stripped `source_root`/`target_root` (the index keys). For a [`Inverse::None`] rel only the
/// forward edge is returned (the inverse token is the FLOOR). A mutant that drops the inverse, swaps
/// the class, or mis-derives the roots is caught.
pub fn mirror_edges(tenant: &TenantId, ev: &SyntheticTypedEvent) -> Vec<EdgeRow> {
    let mut rows = vec![lifecycle_edge(tenant, &ev.source, &ev.target, ev.rel, ev)];
    match ev.rel.inverse() {
        // A distinct paired inverse (`blocks`→`blocked_by`): project the inverse rel, endpoints swapped.
        Inverse::Paired(inv) => {
            rows.push(lifecycle_edge(tenant, &ev.target, &ev.source, inv, ev));
        }
        // A symmetric rel (`relates`): project the SAME rel, endpoints swapped (visible from both ends).
        Inverse::Symmetric => {
            rows.push(lifecycle_edge(tenant, &ev.target, &ev.source, ev.rel, ev));
        }
        // FLOOR: no frozen inverse token yet (`closes`/`depends_on`/`assigns`) — forward only.
        Inverse::None => {}
    }
    rows
}

/// Build ONE `lifecycle`-class [`EdgeRow`] for `(source, target, rel)` (the deterministic `edge_id` +
/// the `#sub`-stripped roots + the lifecycle class discipline). Shared by the forward + inverse legs.
fn lifecycle_edge(
    tenant: &TenantId,
    source: &ArtifactRef,
    target: &ArtifactRef,
    rel: LifecycleRel,
    ev: &SyntheticTypedEvent,
) -> EdgeRow {
    EdgeRow {
        edge_id: edge_id(tenant, &source.0, &target.0, rel.as_str()),
        source_root: strip_sub(source),
        target_root: strip_sub(target),
        source: source.clone(),
        target: target.clone(),
        rel: rel.as_str().to_string(),
        // THE discipline: a mirror edge is ALWAYS lifecycle-class (never reference) — §3.2/§3.3 seam.
        rel_class: RelClass::Lifecycle,
        origin_event: ev.origin_event.clone(),
        origin_actor: ev.origin_actor.clone(),
        zookie: ev.zookie.clone(),
        tombstoned: false,
    }
}

/// **Project a synthetic typed lifecycle event into the edge projection (the consumer-side mirror).**
/// Validates the rel against the frozen vocabulary (rejects unknown — REF-3), then upserts BOTH
/// inverse-paired lifecycle edges via the deterministic `edge_id` (idempotent — a replay is one pair,
/// not duplicates). This is the discipline the REF-P6 edge-builder runs for `issue.relation.*` /
/// `knowledge.page.*` events; at M2 it is driven by [`SyntheticTypedEvent`] (the typed tables are
/// REF-P18/REF-P20). Tenant-first (no cross-tenant path).
pub fn project_typed_event(
    proj: &EdgeProjection,
    tenant: &TenantId,
    region: &Region,
    ev: &SyntheticTypedEvent,
) -> Result<Vec<String>, MirrorError> {
    let rows = mirror_edges(tenant, ev);
    let ids: Vec<String> = rows.iter().map(|r| r.edge_id.clone()).collect();
    for row in rows {
        proj.upsert(tenant, region, row);
    }
    Ok(ids)
}

/// **The TE-7 drift reconvergence — the typed table always wins (§3.3 / §4.7; drill REF-D4 TE-7 half).**
/// Given the AUTHORITATIVE typed snapshot for a scope (the set of typed lifecycle events a `reindex`
/// re-emits for that scope) + the scope's `target_root`s the snapshot covers, reconverge the
/// projection: (1) project every snapshot event's inverse-paired edges (the typed truth becomes live);
/// (2) **tombstone any lifecycle edge inbound to a covered `target_root` that is NOT in the typed
/// snapshot** (a drifted / stale / spurious projection edge the typed table does not back). The typed
/// table wins: after reconvergence the live lifecycle set for the scope == exactly the typed
/// snapshot's edges. `reference`-class edges are UNTOUCHED (they are Refs-authoritative, not mirrored).
///
/// `covered_roots` is the set of `target_root`s the typed snapshot is authoritative over (a scoped
/// reindex re-emits a bounded scope, not the whole tenant — §4.7). A lifecycle edge inbound to a
/// covered root that the snapshot does not re-emit is drift → tombstoned. A lifecycle edge inbound to
/// a root OUTSIDE the scope is left alone (the reindex did not cover it). Returns the count of
/// (re-projected-pairs, tombstoned-drift) for the drill's quantified gate. Tenant-first.
pub fn reconverge(
    proj: &EdgeProjection,
    tenant: &TenantId,
    region: &Region,
    typed_snapshot: &[SyntheticTypedEvent],
    covered_roots: &[ArtifactRef],
    reindex_event_id: &str,
) -> Result<(usize, usize), MirrorError> {
    // (1) The typed truth becomes live: project (idempotent upsert) every snapshot event's edge pair.
    // Collect the edge_ids the typed snapshot backs (the "should-live" set) so step (2) can find drift.
    let mut backed: HashSet<String> = HashSet::new();
    let mut reprojected = 0usize;
    for ev in typed_snapshot {
        let rows = mirror_edges(tenant, ev);
        for row in rows {
            backed.insert(row.edge_id.clone());
            proj.upsert(tenant, region, row);
            reprojected += 1;
        }
    }

    // (2) Tombstone drift: any LIVE lifecycle edge inbound to a COVERED root that the typed snapshot
    // does NOT back is a stale/spurious projection the typed table does not authorise → the typed
    // table wins. `reference`-class edges are never touched (Refs-authoritative). Edges outside the
    // covered scope are left alone (the scoped reindex did not cover them).
    let mut tombstoned = 0usize;
    for root in covered_roots {
        for row in proj.inbound_live(tenant, region, root) {
            if row.rel_class == RelClass::Lifecycle && !backed.contains(&row.edge_id) {
                proj.tombstone(tenant, region, &row.edge_id, reindex_event_id);
                tombstoned += 1;
            }
        }
    }
    Ok((reprojected, tombstoned))
}

#[cfg(test)]
mod tests;
