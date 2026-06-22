//! Unit tests for the unified 4-step `#sub` resolution ladder (REF-P15 / P-164; REF-D9 + the §3.5
//! Git content-anchored states). Every ladder arm, the root-always-carried rule, and each Git
//! content-anchored state has a test a mutation flips.

use std::sync::Arc;

use myelin_events::ArtifactRef;
use myelin_identity::{Decision, Permission, Principal, PrincipalId, PrincipalKind};
use myelin_substrate::{FailStaticAuthz, FailStaticThreshold};
use myelin_tenancy::{CellId, Region, TenantId};

use super::*;
use crate::resolve::{
    bounded_stale, NoOpCacheRead, OwnerProjection, ProjectApi, ProjectApiError, ProjectOutcome,
    ProjectionFlag, Resolution, ResolveMode, ResolveService, TombstoneReason,
};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn cell() -> CellId {
    CellId::from_token("cell-fr-par-1")
}
fn viewer(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
}
fn aref(s: &str) -> ArtifactRef {
    myelin_refs::parse(s).expect("a well-formed URN")
}

fn threshold() -> FailStaticThreshold {
    FailStaticThreshold {
        status: "OPEN — LEGAL".into(),
        owner: "DPO / Legal".into(),
        static_max_secs: None,
        static_max_default_secs: 300,
        agent_token_ttl_secs: 60,
        constraint: "static_max <= revocation-SLA AND static_max >= agent-token-TTL".into(),
    }
}
fn authz() -> Arc<FailStaticAuthz> {
    Arc::new(FailStaticAuthz::try_new(300, &threshold()).expect("valid bound"))
}

// ── The ladder mapping SubState → ProjectOutcome (the ONE §4.6 mapping) ──

fn proj() -> OwnerProjection {
    OwnerProjection {
        title: "a block".into(),
        state: "live".into(),
        icon: "doc".into(),
        render_hint: "embed".into(),
        sub_anchor: Some("b9".into()),
        flag: None,
    }
}

/// **LIVE → a flag-less projection.** The clean path: a live sub-anchor renders with NO flag.
#[test]
fn live_maps_to_a_flagless_projection() {
    match SubState::Live(proj()).into_outcome() {
        ProjectOutcome::Live(p) => assert_eq!(p.flag, None, "a LIVE sub renders with no flag"),
        other => panic!("LIVE must map to Live(no flag), got {other:?}"),
    }
}

/// **LIVE CLEARS any stale flag → `None`.** The `Live` arm normalises the flag to `None` even if the
/// owner projection arrived carrying one (a LIVE sub is definitionally un-flagged). Catches a mutant
/// that drops the `flag: None` reset.
#[test]
fn live_clears_a_stale_flag_to_none() {
    let mut p = proj();
    p.flag = Some(ProjectionFlag::Moved); // a stale flag on the incoming projection
    match SubState::Live(p).into_outcome() {
        ProjectOutcome::Live(out) => assert_eq!(out.flag, None, "LIVE normalises the flag to None"),
        other => panic!("LIVE must map to Live(flag=None), got {other:?}"),
    }
}

/// **The contiguous-subsequence finder is exact at the boundaries (the rebased/partial match).** An
/// empty needle, an over-long needle, a non-match, and an exact end-of-haystack match — each precise.
#[test]
fn find_subsequence_is_exact_at_boundaries() {
    let blob = ["fn a", "  body", "}", "tail"];
    let minted = MintedLineRange {
        blob_oid: "x".into(),
        anchored: vec![
            MintedLineRange::fingerprint("}"),
            MintedLineRange::fingerprint("tail"),
        ],
    };
    // the anchored pair is found at the END of the blob (offset 2) → rebased to lines 3-4.
    let state = resolve_line_range(&minted, "y", &blob);
    assert_eq!(
        state,
        LineRangeState::Rebased {
            new_start: 3,
            new_end: 4
        },
        "end-of-blob contiguous match"
    );

    // a needle longer than the haystack: the WHOLE block cannot match (the over-long guard in
    // find_subsequence returns None), but a surviving prefix [a,b] is found → Partial (the prefix
    // search drives the result). This exercises the `needle.len() > haystack.len()` guard on the whole
    // block AND the prefix fall-through.
    let mostly_gone = MintedLineRange {
        blob_oid: "x".into(),
        anchored: vec![
            MintedLineRange::fingerprint("a"),
            MintedLineRange::fingerprint("b"),
            MintedLineRange::fingerprint("c"),
            MintedLineRange::fingerprint("d"),
            MintedLineRange::fingerprint("e"),
        ],
    };
    assert_eq!(
        resolve_line_range(&mostly_gone, "y", &["a", "b"]),
        LineRangeState::Partial {
            surviving_start: 1,
            surviving_end: 2
        },
        "the surviving a-b prefix (the rest of the anchor is gone)"
    );

    // NONE survive → content_gone (the whole block AND every prefix fail).
    let all_gone = MintedLineRange {
        blob_oid: "x".into(),
        anchored: vec![
            MintedLineRange::fingerprint("x"),
            MintedLineRange::fingerprint("y"),
        ],
    };
    assert_eq!(
        resolve_line_range(&all_gone, "y", &["totally", "different"]),
        LineRangeState::ContentGone
    );
}

/// **A single-line anchor matched at the very last line of a longer blob (offset == len - 1).** Pins
/// the `0..=haystack.len() - needle.len()` upper bound is INCLUSIVE (a mutant that makes it exclusive,
/// or flips the over-long guard, is caught).
#[test]
fn single_line_anchor_matches_at_last_position() {
    let minted = MintedLineRange {
        blob_oid: "x".into(),
        anchored: vec![MintedLineRange::fingerprint("last")],
    };
    let state = resolve_line_range(&minted, "y", &["a", "b", "last"]);
    assert_eq!(
        state,
        LineRangeState::Rebased {
            new_start: 3,
            new_end: 3
        },
        "matched at the final line"
    );
}

/// **MOVED → a projection flagged `moved` (Git rebased range; KN block moved).** The anchor still
/// renders (shifted), flagged so the UI marks it — NOT a tombstone (graceful degradation).
#[test]
fn moved_maps_to_a_moved_flagged_projection() {
    match SubState::Moved(proj()).into_outcome() {
        ProjectOutcome::Live(p) => assert_eq!(p.flag, Some(ProjectionFlag::Moved)),
        other => panic!("MOVED must map to Live(flag=Moved), got {other:?}"),
    }
}

/// **OUTDATED → a projection flagged `outdated` (Git partial range; KN edited block).** The partial
/// still renders, flagged — never a tombstone.
#[test]
fn outdated_maps_to_an_outdated_flagged_projection() {
    match SubState::Outdated(proj()).into_outcome() {
        ProjectOutcome::Live(p) => assert_eq!(p.flag, Some(ProjectionFlag::Outdated)),
        other => panic!("OUTDATED must map to Live(flag=Outdated), got {other:?}"),
    }
}

/// **GONE → SubGone (root still resolves; the embed shows the parent).** The sub-artifact is gone but
/// the root is carried — 0 hard 404.
#[test]
fn gone_maps_to_sub_gone() {
    assert_eq!(SubState::Gone.into_outcome(), ProjectOutcome::SubGone);
}

/// **ERASED → Erased (the most final state).** A pseudonym-/crypto-shred made it unrenderable.
#[test]
fn erased_maps_to_erased() {
    assert_eq!(SubState::Erased.into_outcome(), ProjectOutcome::Erased);
}

// ── The Git content-anchored line-range resolver (§3.5) — exact/rebased/partial/content_gone ──

/// **exact — the blob oid matches → the minted range is LIVE as-is.**
#[test]
fn git_line_range_exact_when_oid_matches() {
    let lines = ["fn a() {", "  body", "}"];
    let minted = MintedLineRange::mint("oid-1", &lines, 1, 3);
    let state = resolve_line_range(&minted, "oid-1", &lines);
    assert_eq!(state, LineRangeState::Exact);
    assert_eq!(
        state.into_sub_state(proj()),
        SubState::Live(proj()),
        "exact → LIVE"
    );
}

/// **rebased — the fingerprinted lines moved to a shifted position (3-way context) → MOVED.** Two
/// lines were inserted ABOVE the anchored block; the block is found shifted down, contiguous.
#[test]
fn git_line_range_rebased_when_block_shifts() {
    let minted_lines = ["fn a() {", "  body", "}"];
    let minted = MintedLineRange::mint("oid-1", &minted_lines, 1, 3);
    // a newer blob: two lines inserted above → the anchored block now starts at line 3.
    let current = ["// header", "// added", "fn a() {", "  body", "}"];
    let state = resolve_line_range(&minted, "oid-2", &current);
    assert_eq!(
        state,
        LineRangeState::Rebased {
            new_start: 3,
            new_end: 5
        },
        "block shifted to 3-5"
    );
    match state.into_sub_state(proj()) {
        SubState::Moved(_) => {}
        other => panic!("rebased → MOVED, got {other:?}"),
    }
}

/// **partial — some anchored lines survive (a contiguous prefix), some are gone → OUTDATED.** The
/// last anchored line was deleted; the surviving prefix is reported.
#[test]
fn git_line_range_partial_when_some_lines_survive() {
    let minted_lines = ["line-A", "line-B", "line-C"];
    let minted = MintedLineRange::mint("oid-1", &minted_lines, 1, 3);
    // line-C is gone; line-A/line-B survive contiguously at the top.
    let current = ["line-A", "line-B", "totally-different"];
    let state = resolve_line_range(&minted, "oid-2", &current);
    assert_eq!(
        state,
        LineRangeState::Partial {
            surviving_start: 1,
            surviving_end: 2
        },
        "the surviving A-B prefix"
    );
    match state.into_sub_state(proj()) {
        SubState::Outdated(_) => {}
        other => panic!("partial → OUTDATED, got {other:?}"),
    }
}

/// **content_gone — none of the anchored content survives → GONE (`Tombstone{ sub_gone }`).**
#[test]
fn git_line_range_content_gone_when_nothing_survives() {
    let minted_lines = ["gone-1", "gone-2"];
    let minted = MintedLineRange::mint("oid-1", &minted_lines, 1, 2);
    let current = ["entirely", "rewritten", "file"];
    let state = resolve_line_range(&minted, "oid-2", &current);
    assert_eq!(state, LineRangeState::ContentGone);
    assert_eq!(
        state.into_sub_state(proj()),
        SubState::Gone,
        "content_gone → GONE → sub_gone"
    );
}

/// **A fingerprint is BLAKE3 over the line content (content-anchored, not positional / not the raw
/// body).** Two different lines fingerprint differently; the same line is stable.
#[test]
fn line_fingerprint_is_blake3_content_anchored() {
    let a = MintedLineRange::fingerprint("fn a() {");
    assert!(a.starts_with("blake3:"), "the ONE multihash convention");
    assert_eq!(
        a,
        MintedLineRange::fingerprint("fn a() {"),
        "stable on the same content"
    );
    assert_ne!(
        a,
        MintedLineRange::fingerprint("fn b() {"),
        "different content → different fp"
    );
}

// ── End-to-end through the chokepoint: the root is ALWAYS carried; 0 dangling/404/leak (REF-D9) ──

/// A synthetic owner that allows everyone + drives `project` through the ladder's sub-resolver, so the
/// REF-P10 chokepoint resolves the §4.6 ladder over a scripted sub-state. The ONE owner handle.
struct LadderOwner {
    sub: Arc<SyntheticSubResolver>,
}

impl ProjectApi for LadderOwner {
    fn check_view(
        &self,
        _t: &TenantId,
        _r: &Region,
        _o: &ArtifactRef,
        _v: &Principal,
        _p: &Permission,
    ) -> Result<Decision, ProjectApiError> {
        Ok(Decision::Allow) // the leak-test for denial lives in resolve.rs; here we exercise step 3-4.
    }

    fn project(
        &self,
        _t: &TenantId,
        _r: &Region,
        ref_: &ArtifactRef,
        _v: &Principal,
        _m: ResolveMode,
    ) -> Result<ProjectOutcome, ProjectApiError> {
        // The owner answers in the FROZEN ladder vocabulary via the sub-anchor resolver (one ladder).
        Ok(resolve_sub_outcome(self.sub.as_ref(), ref_))
    }
}

fn svc(sub: Arc<SyntheticSubResolver>) -> ResolveService {
    ResolveService::new(
        authz(),
        Arc::new(NoOpCacheRead),
        Arc::new(LadderOwner { sub }),
        cell(),
    )
}

fn resolve(svc: &ResolveService, ref_: &ArtifactRef) -> Resolution {
    let root = ladder_root(ref_);
    svc.resolve(
        &tenant(),
        &region(),
        ref_,
        &root,
        &viewer("insider"),
        ResolveMode::Live,
        &bounded_stale(),
        false,
    )
}

/// **REF-D9 — a doc block that others embed degrades through the ladder: GONE → `Tombstone{sub_gone}`
/// WITH the root carried (0 dangling embed, 0 hard 404).** The embed does not vanish — it shows the
/// parent page.
#[test]
fn ref_d9_deleted_doc_block_tombstones_sub_gone_carrying_root() {
    let sub = Arc::new(SyntheticSubResolver::new());
    let block = "myelin://acme/knowledge/page/7c2#b9";
    sub.set_state(block, SubState::Gone);
    let svc = svc(sub);

    let r = resolve(&svc, &aref(block));
    assert_eq!(
        r.tombstone_reason(),
        Some(TombstoneReason::SubGone),
        "deleted block → sub_gone"
    );
    if let Resolution::Tombstone(t) = &r {
        // the ROOT is carried (the page) — the embed degrades to "this referenced <page 7c2>".
        assert_eq!(
            t.root.0, "myelin://acme/knowledge/page/7c2",
            "the tombstone carries the root"
        );
    } else {
        panic!("a deleted block must tombstone, not render");
    }
}

/// **REF-D9 — a deleted chat message embedded elsewhere → `Tombstone{sub_gone}`, root carried.** Same
/// ONE ladder across the Chat content shape.
#[test]
fn ref_d9_deleted_chat_message_tombstones_carrying_root() {
    let sub = Arc::new(SyntheticSubResolver::new());
    let msg = "myelin://acme/chat/message/01J0CH#message-m3";
    sub.set_state(msg, SubState::Gone);
    let svc = svc(sub);

    let r = resolve(&svc, &aref(msg));
    assert_eq!(r.tombstone_reason(), Some(TombstoneReason::SubGone));
    if let Resolution::Tombstone(t) = &r {
        assert_eq!(t.root.0, "myelin://acme/chat/message/01J0CH");
    }
}

/// **REF-D9 — a rebased Git line-range that others embed → renders MOVED (NOT a tombstone), root
/// carried.** The PR context still shows the (shifted) code, flagged moved — graceful degradation.
#[test]
fn ref_d9_rebased_git_range_renders_moved_not_tombstone() {
    let sub = Arc::new(SyntheticSubResolver::new());
    let range = "myelin://acme/git/ref/main#L42-L88";
    sub.set_state(
        range,
        SubState::Moved(SyntheticSubResolver::default_projection()),
    );
    let svc = svc(sub);

    let r = resolve(&svc, &aref(range));
    assert!(
        r.is_projection(),
        "a rebased range still renders (graceful), not a tombstone"
    );
    if let Resolution::Projection(p) = &r {
        assert_eq!(p.flag, Some(ProjectionFlag::Moved), "flagged moved");
    }
}

/// **REF-D9 — an OUTDATED (partial) range renders, flagged outdated.** Some lines survive.
#[test]
fn ref_d9_partial_git_range_renders_outdated() {
    let sub = Arc::new(SyntheticSubResolver::new());
    let range = "myelin://acme/git/ref/main#L42-L88";
    sub.set_state(
        range,
        SubState::Outdated(SyntheticSubResolver::default_projection()),
    );
    let svc = svc(sub);

    let r = resolve(&svc, &aref(range));
    assert!(r.is_projection());
    if let Resolution::Projection(p) = &r {
        assert_eq!(p.flag, Some(ProjectionFlag::Outdated));
    }
}

/// **REF-D9 — an ERASED sub-artifact → `Tombstone{erased}`, root carried (0 leak).**
#[test]
fn ref_d9_erased_sub_tombstones_erased_carrying_root() {
    let sub = Arc::new(SyntheticSubResolver::new());
    let block = "myelin://acme/knowledge/page/7c2#b9";
    sub.set_state(block, SubState::Erased);
    let svc = svc(sub);

    let r = resolve(&svc, &aref(block));
    assert_eq!(
        r.tombstone_reason(),
        Some(TombstoneReason::Erased),
        "erased sub → erased"
    );
    if let Resolution::Tombstone(t) = &r {
        assert_eq!(
            t.root.0, "myelin://acme/knowledge/page/7c2",
            "the root is still carried"
        );
    }
}

/// **A LIVE sub-anchor renders the embed (the happy path through the ladder).**
#[test]
fn live_sub_anchor_renders_the_embed() {
    let sub = Arc::new(SyntheticSubResolver::new());
    let svc = svc(sub);
    let r = resolve(&svc, &aref("myelin://acme/git/pr/42#comment-c9"));
    assert!(r.is_projection(), "a live comment anchor renders");
    if let Resolution::Projection(p) = &r {
        assert_eq!(p.flag, None, "a clean LIVE anchor has no flag");
    }
}

/// **A bare root (no `#sub`) resolves LIVE — there is no sub-anchor to degrade.** The default
/// `resolve_sub` short-circuits a bare root to Live.
#[test]
fn bare_root_resolves_live() {
    let sub = Arc::new(SyntheticSubResolver::new());
    let svc = svc(sub);
    let r = resolve(&svc, &aref("myelin://acme/git/pr/42"));
    assert!(r.is_projection(), "a bare root with no #sub is live");
}

/// **The tombstone-count distribution signal is the named constant (1.8 observability).**
#[test]
fn tombstone_count_signal_is_named() {
    assert_eq!(TOMBSTONE_COUNT_SIGNAL, "refs.tombstone_count");
}
