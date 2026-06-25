//! # CHAT-P30 drill — cross-org / federated channels on the cross-cell bridge (M5-C-X1 / P-504)
//!
//! The dated green artifact the CHAT-P30 GATE names (the bridge SHIPPED in M5, so this is BUILT, not
//! a designed-not-built floor). Two quantified properties, proven MEASURED (not asserted), against
//! the chat-owned cross-org channel layer ([`myelin_chat::cross_org`]) which CONSUMES the frozen
//! cross-cell PII-free pointer bridge (contract 12.6) + the Bus's propagation half (EB-25):
//!
//! 1. **cross-cell resolution is always cell-local** (0 raw cross-cell rows crossing; only the
//!    already-permission-filtered projection / tombstone crosses — contract 12.6 / 5.6, §OQ-I);
//! 2. **multi-cell DSR iterates `member_cells`** (0 holders missed across cells — contract 10.4).
//!
//! Plus the residency invariant: the cross-org fan-out carries ONLY the four-field PII-free frame —
//! never a body/topic/roster (`raw_rows_crossed == 0`).
//!
//! These are the same properties the control-plane CP-D8 / GA-D8 / CP-D7 drills assert at the bridge
//! layer (P-429/P-430); this drill re-runs them across the CHAT channel boundary (the M5-C-X1
//! cross-cell-resolution-cell-local property).

use myelin_chat::cross_org::{
    fanned_out_carried_fields, CellLocalChannelResolution, ChannelProjection, CrossOrgChannel,
    CrossOrgPointer, FederatedMember,
};
use myelin_chat::store::ConversationId;
use myelin_events::{CellId, CorrelationId};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::TenantId;

fn conv() -> ConversationId {
    ConversationId::new("acme", "fr-par", "01J0XORG")
}

fn viewer(id: &str) -> Principal {
    Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

/// A cross-org channel spanning the home cell (fr-par-1) + two foreign org cells (de-1, nl-1), one
/// of which holds two members — to prove the fan-out is per-CELL, never per-principal.
fn roster() -> Vec<FederatedMember> {
    vec![
        FederatedMember::new("psn:creator", CellId::from_token("cell-fr-par-1")),
        FederatedMember::new("psn:de-a", CellId::from_token("cell-de-1")),
        FederatedMember::new("psn:de-b", CellId::from_token("cell-de-1")),
        FederatedMember::new("psn:nl", CellId::from_token("cell-nl-1")),
    ]
}

/// An in-process home-cell resolver standing in for the channel's home cell (the SAME stand-in the
/// control-plane bridge tests use): it renders IFF the viewer is permitted THERE, returning ONLY a
/// projection — never a raw row / message log / body byte.
struct HomeCellResolver {
    allowed: Vec<String>,
}

impl CellLocalChannelResolution for HomeCellResolver {
    fn resolve_in_home_cell(&self, pointer: &CrossOrgPointer, v: &Principal) -> ChannelProjection {
        let subject = pointer.subject().clone();
        if self.allowed.iter().any(|id| id == &v.principal_id.0) {
            ChannelProjection::Rendered {
                subject,
                rendered: "#cross-org-incident · active".into(),
            }
        } else {
            ChannelProjection::Tombstone { subject }
        }
    }
}

/// **DRILL leg 1 — cross-cell resolution is ALWAYS cell-local (0 raw rows cross).** Cell A fans the
/// PII-free channel pointer to the foreign org cells; a member THERE resolves the channel THROUGH
/// the home cell — an authorised viewer gets the rendered projection, an unauthorised one a
/// tombstone (no leak across the org boundary). In NO case does a raw row / body byte cross.
#[test]
fn drill_cross_cell_resolution_is_cell_local_zero_raw_rows() {
    let xorg = CrossOrgChannel::new(CellId::from_token("cell-fr-par-1"));
    let fanned = xorg.fan_out_channel_event(&conv(), &CorrelationId("root".into()), &roster());

    // The fan-out reached BOTH foreign org cells (home filtered out; de-1 de-duplicated).
    let dests: Vec<&str> = fanned.iter().map(|p| p.to_cell.as_str()).collect();
    assert_eq!(dests, vec!["cell-de-1", "cell-nl-1"]);

    // Only the four-field frame crossed — never a body/topic/roster.
    for pp in &fanned {
        let (_to, subject, kind, _corr, home) = fanned_out_carried_fields(pp);
        assert_eq!(
            subject.artifact_ref().0,
            "myelin://acme/chat/channel/01J0XORG"
        );
        assert_eq!(kind, &myelin_events::ArtifactType::Channel);
        assert_eq!(home.as_str(), "cell-fr-par-1");
        let wire = serde_json::to_string(&pp.pointer).unwrap();
        assert!(!wire.contains("payload"), "no body field crossed: {wire}");
    }

    // Resolution is cell-local: an AUTHORISED viewer renders THROUGH the home cell.
    let home = HomeCellResolver {
        allowed: vec!["psn:de-a".into()],
    };
    let ok = xorg.resolve_cell_local(&fanned[0], &viewer("psn:de-a"), &home);
    assert!(
        ok.is_rendered(),
        "an authorised cross-org viewer gets the projection"
    );

    // An UNAUTHORISED cross-org viewer gets a tombstone — no leak across the org boundary.
    let denied = xorg.resolve_cell_local(&fanned[0], &viewer("psn:intruder"), &home);
    assert!(
        !denied.is_rendered(),
        "an unauthorised viewer gets a tombstone"
    );
    let wire = serde_json::to_string(&denied).unwrap();
    assert!(
        !wire.contains("cross-org-incident"),
        "the channel name never crossed: {wire}"
    );

    // THE ZERO: 0 raw cross-cell rows crossed the bridge.
    assert_eq!(
        xorg.raw_rows_crossed(),
        0,
        "0 raw rows crossed the cell boundary"
    );
}

/// **DRILL leg 2 — multi-cell DSR iterates `member_cells` (0 holders missed, contract 10.4).** The
/// DSR enumeration returns EVERY distinct cell the channel's membership spans, INCLUDING the home
/// cell (it holds the creator's authored bodies) — so a person's erasure reaches every cell they
/// participate in. 0 member cells dropped.
#[test]
fn drill_multi_cell_dsr_iterates_member_cells_zero_missed() {
    let xorg = CrossOrgChannel::new(CellId::from_token("cell-fr-par-1"));
    let cells = xorg.dsr_member_cells(&roster());
    let names: Vec<&str> = cells.iter().map(CellId::as_str).collect();
    // All THREE distinct cells — incl. the home cell (a DSR holder too).
    assert_eq!(names, vec!["cell-de-1", "cell-fr-par-1", "cell-nl-1"]);
    assert_eq!(
        cells.len(),
        3,
        "0 member cells missed by the DSR enumeration"
    );
}
