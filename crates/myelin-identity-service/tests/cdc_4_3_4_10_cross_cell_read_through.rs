//! # The CDC pair for the cross-cell read-through over 12.6 — rows 4.3/4.10 + 12.6 (P-ID-35)
//!
//! **Contract-index rows 4.3/4.10** (the zookie-bounded read-through) **+ row 12.6** (the cross-cell
//! PII-free pointer bridge, `CrossCellPointer{subject, type, correlation_id, home_cell}`). This is
//! the dedicated provider+consumer pair the P-ID-35 TESTS field names — the in-CI evidence that the
//! two sides of the cross-cell read-through seam cannot drift apart:
//!
//! - the **PROVIDER** is the home cell's authority ([`MultiCellAuthority::resolve_cross_cell`] over
//!   the frozen [`CrossCellPointer`] frame, contract 12.6): given a viewer in cell A and a pointer to
//!   an artifact homed in cell B, it ROUTES to cell B, resolves cell-locally **in B**
//!   (permission-checked against B's tuples), and returns ONLY the verdict — a projection-ready
//!   [`CrossCellResolution::Projection`] stamped at B's zookie (the zookie-bounded read-through,
//!   rows 4.3/4.10) or a [`CrossCellResolution::Tombstone`] (§OQ-I: unauthorized → tombstone). It
//!   NEVER returns raw tuples, NEVER pulls tuples cross-region (`cross_region_tuple_pulls == 0`).
//! - the **CONSUMER** is a cross-cell UNFURL/render (exactly the ISS portfolio rollup / KN cross-cell
//!   embed / CHAT cross-org channel shape, §OQ-I): it takes the `CrossCellResolution` verdict and the
//!   PII-free [`CrossCellPointer`] frame and renders — for `Projection` it shows the
//!   already-permission-filtered projection (here, the opaque pointer + the bounding zookie); for
//!   `Tombstone` it shows a tombstone. It only ever sees the verdict + the four-field PII-free frame
//!   — never B's tuples, never PII.
//!
//! The provider's promise (resolution is cell-local in the home cell; only the verdict crosses; 0
//! cross-region tuple pulls; the verdict is zookie-bounded) and the consumer's promise (it renders
//! exactly the verdict over the PII-free frame, never reaching past it) are pinned here so a change to
//! either side fails this test in the same CI job.

use myelin_events::{OutboxStore, Timestamp};
use myelin_identity::{
    Decision, ObjectId, Permission, Principal, PrincipalId, PrincipalKind, RelName, RelationTuple,
    TupleDelta,
};
use myelin_identity_service::{
    CellPartition, CrossCellResolution, MultiCellAuthority, StoreBackedCheck, TupleStore,
};
use myelin_storage::TenantScope;
use myelin_tenancy::{
    ArtifactRef, ArtifactType, CellId, CorrelationId, CrossCellPointer, OpaqueSubjectId, Region,
    TenantId,
};

const REGION: &str = "fr-par";

fn principal(tenant: &str, id: &str) -> Principal {
    let mut p = Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    p.region = Region(REGION.into());
    p
}

fn add(object: &str, relation: &str, subject: &str) -> TupleDelta {
    TupleDelta::Add(RelationTuple {
        object: ObjectId(object.into()),
        relation: RelName(relation.into()),
        subject: PrincipalId(subject.into()),
        caveat: None,
    })
}

/// A home cell B seeded so `member` of `team:eng` inherits `view` on `project:web` — the artifact the
/// cross-cell pointer points at. `lin` is a member; `ada` (the cross-cell viewer) is NOT.
fn home_cell_b() -> CellPartition {
    let admin = principal("acme", "p-admin");
    let scope = TenantScope::from_verified_token(&admin, admin.region.clone());
    let store = TupleStore::new(OutboxStore::new());
    store
        .write_tuples(
            &scope,
            &admin,
            &[
                add("project:web", "parent_team", "team:eng#view"),
                add("team:eng", "member", "p:lin"),
            ],
            None,
            None,
            Timestamp("2026-06-24T00:00:00Z".into()),
        )
        .expect("seed cell B");
    CellPartition::new(
        CellId::from_token("cell-b"),
        Region(REGION.into()),
        StoreBackedCheck::new(store),
    )
}

/// The frozen 12.6 pointer to the cell-B-homed artifact — PII-free (opaque subject + home cell only).
fn pointer_to_b() -> CrossCellPointer {
    CrossCellPointer::new(
        OpaqueSubjectId::from_ref(ArtifactRef(
            "myelin://01JACME/projects/project/web@cell-b".into(),
        )),
        ArtifactType::Page,
        CorrelationId("01JCORR".into()),
        CellId::from_token("cell-b"),
    )
}

/// **The CONSUMER side: a cross-cell unfurl/render.** It takes ONLY the verdict + the PII-free 12.6
/// frame and produces a render string — a projection (for an authorized viewer) or a tombstone. It
/// has NO access to the home cell's tuples; it cannot reach past the verdict. This is the ISS rollup /
/// KN embed / CHAT cross-org shape (§OQ-I): aggregate/render PROJECTIONS, never raw rows.
fn render_cross_cell(pointer: &CrossCellPointer, resolution: &CrossCellResolution) -> String {
    match resolution {
        CrossCellResolution::Projection { home_cell, zookie } => {
            // The consumer renders the already-permission-filtered projection: it shows the opaque
            // pointer kind + the home cell + the bounding zookie. It NEVER sees a tuple or PII — only
            // the PII-free frame (the opaque subject) + the verdict's bound.
            format!(
                "projection[{:?}]@{} zookie={} subject={}",
                pointer.artifact_type(),
                home_cell.as_str(),
                zookie.0,
                pointer.subject().artifact_ref().0,
            )
        }
        CrossCellResolution::Tombstone { home_cell } => {
            format!("tombstone@{}", home_cell.as_str())
        }
    }
}

/// **PROVIDER + CONSUMER: the authorized cross-cell read-through round-trips.** Lin (a member in cell
/// B) resolves a pointer to a B-homed artifact from cell A → the provider routes to B, resolves
/// cell-local (Lin is a member → Allow), returns a projection stamped at B's zookie; the consumer
/// renders the projection. 0 cross-region tuple pulls.
#[test]
fn provider_consumer_authorized_cross_cell_read_through() {
    let mut authority = MultiCellAuthority::new();
    authority.register_cell(home_cell_b());

    let pointer = pointer_to_b();
    let lin = principal("acme", "p:lin");

    // PROVIDER: resolve the cross-cell pointer (route to B, resolve in B, only the verdict crosses).
    let (resolution, audit) = authority.resolve_cross_cell(
        &CellId::from_token("cell-a"),
        &lin,
        &pointer,
        &Permission("view".into()),
        &ArtifactRef("project:web".into()),
    );
    // The provider's promise: cell-local resolution, 0 cross-region tuple pulls, zookie-bounded.
    assert!(
        resolution.is_authorized(),
        "Lin is a member in the home cell B"
    );
    assert_eq!(
        audit.home_cell,
        CellId::from_token("cell-b"),
        "resolved in the home cell"
    );
    assert_eq!(
        audit.cross_region_tuple_pulls, 0,
        "0 cross-region tuple pulls (rows 4.3/4.10 + 12.6)"
    );
    assert!(audit.is_pii_free());
    match &resolution {
        CrossCellResolution::Projection { zookie, .. } => {
            assert!(
                !zookie.0.is_empty(),
                "the verdict is zookie-bounded at the home cell"
            );
        }
        _ => panic!("expected a projection"),
    }

    // CONSUMER: render the verdict over the PII-free frame — a projection, never a raw row.
    let rendered = render_cross_cell(&pointer, &resolution);
    assert!(rendered.starts_with("projection"), "{rendered}");
    assert!(
        rendered.contains("cell-b"),
        "the render names the home cell that authoritatively resolved"
    );
    // The consumer NEVER renders PII: the subject is the opaque artifact ref, not a name/email.
    assert!(
        rendered.contains("myelin://"),
        "the rendered subject is the opaque pointer, never a person"
    );
}

/// **PROVIDER + CONSUMER: the unauthorized cross-cell read-through tombstones (§OQ-I).** Ada (NOT a
/// member in cell B) resolves the same pointer → the provider resolves cell-local in B (Ada is denied
/// → Deny), returns a tombstone; the consumer renders a tombstone, never the artifact. 0 cross-region
/// tuple pulls. This is the no-leak floor: a cross-cell viewer who lacks the grant sees a tombstone.
#[test]
fn provider_consumer_unauthorized_cross_cell_tombstone() {
    let mut authority = MultiCellAuthority::new();
    authority.register_cell(home_cell_b());

    let pointer = pointer_to_b();
    let ada = principal("acme", "p:ada"); // not a member in cell B

    let (resolution, audit) = authority.resolve_cross_cell(
        &CellId::from_token("cell-a"),
        &ada,
        &pointer,
        &Permission("view".into()),
        &ArtifactRef("project:web".into()),
    );
    assert!(
        !resolution.is_authorized(),
        "Ada is not a member in the home cell B"
    );
    assert!(
        matches!(resolution, CrossCellResolution::Tombstone { .. }),
        "unauthorized → tombstone"
    );
    assert_eq!(audit.cross_region_tuple_pulls, 0);
    assert!(audit.is_pii_free());

    let rendered = render_cross_cell(&pointer, &resolution);
    assert_eq!(
        rendered, "tombstone@cell-b",
        "the consumer renders a tombstone, never the artifact"
    );
}

/// **The coarse-grant read-through is zookie-bounded (rows 4.3/4.10).** The provider's
/// `read_through_coarse_grant` resolves cell-local in the home cell and stamps the home cell's
/// snapshot zookie; the consumer (a cross-cell access gate) admits iff the grant is a bounded Allow.
#[test]
fn provider_consumer_coarse_grant_zookie_bounded() {
    let mut authority = MultiCellAuthority::new();
    authority.register_cell(home_cell_b());
    let lin = principal("acme", "p:lin");

    let grant = authority.read_through_coarse_grant(
        &lin,
        &Permission("view".into()),
        &ArtifactRef("project:web".into()),
        &CellId::from_token("cell-b"),
    );
    // PROVIDER promise: a bounded Allow (decision + a non-empty home-cell zookie).
    assert_eq!(grant.decision, Decision::Allow);
    assert!(
        grant.is_bounded_allow(),
        "the coarse grant is zookie-bounded"
    );
    // CONSUMER promise: a cross-cell access gate admits iff the grant is a bounded Allow.
    let admitted = grant.is_bounded_allow();
    assert!(
        admitted,
        "the cross-cell access gate admits a bounded Allow"
    );
}
