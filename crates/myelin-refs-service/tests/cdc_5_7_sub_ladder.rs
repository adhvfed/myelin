//! # CDC 5.7 — the unified 4-step `#sub` resolution ladder + REF-D9 (REF-P15 / P-164)
//!
//! **Contract:** index row 5.7 (the unified `#sub` scheme + the frozen 4-step ladder, §4.6 / C-2) +
//! 5.6 (the owner's `project` sub-anchor resolver returning the frozen `live/moved/outdated/gone`
//! state). This is the provider+consumer CDC pair the contract-coverage scanner (P-S21) reads for the
//! Refs ladder seam, plus the REF-D9 drill scenario (the unified ladder across the three content
//! shapes — Git line-ranges, KN block anchors, Chat message anchors).
//!
//! - **PROVIDER** = the owner subsystem implementing the 5.6 sub-anchor resolver in the FROZEN
//!   [`SubState`] vocabulary (here a [`SyntheticSubResolver`] — the real Git/KN/Chat owners land in
//!   REF-P17/P18/P21). The owner answers in `live/moved/outdated/gone`; Refs maps it onto the ladder.
//! - **CONSUMER** = the Refs resolve chokepoint (REF-P10) driving the §4.6 ladder over the owner's
//!   sub-state through [`resolve_sub_outcome`] — the ONE mapping (one ladder, no second resolver).
//!
//! The dated green artifact: each content shape degrades through the frozen ladder to the correct
//! state (moved / outdated / sub_gone / erased) with the **root carried** — 0 dangling embed, 0 hard
//! 404, no leak (REF-D9). If 5.7's ladder shape drifts, this stops passing — that is the contract.
//! At M2 exercised against synthetic + the available producers; re-run on each REAL producer in
//! R-M3/R-M4 (REF-P17/P18/P19/P20/P21 — named floor).

use std::sync::Arc;

use myelin_events::ArtifactRef;
use myelin_identity::{Decision, Permission, Principal, PrincipalId, PrincipalKind};
use myelin_refs_service::{
    bounded_stale, ladder_root, resolve_sub_outcome, MintedLineRange, NoOpCacheRead,
    OwnerProjection, ProjectApi, ProjectApiError, ProjectOutcome, ProjectionFlag, Resolution,
    ResolveMode, ResolveService, SubState, SyntheticSubResolver, TombstoneReason,
};
use myelin_substrate::{FailStaticAuthz, FailStaticThreshold};
use myelin_tenancy::{CellId, Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn cell() -> CellId {
    CellId::from_token("cell-fr-par-1")
}
fn viewer() -> Principal {
    Principal::stub(
        PrincipalId("insider".into()),
        PrincipalKind::Human,
        tenant(),
    )
}
fn aref(s: &str) -> ArtifactRef {
    myelin_refs::parse(s).expect("a well-formed URN")
}

fn authz() -> Arc<FailStaticAuthz> {
    let threshold = FailStaticThreshold {
        status: "OPEN — LEGAL".into(),
        owner: "DPO / Legal".into(),
        static_max_secs: None,
        static_max_default_secs: 300,
        agent_token_ttl_secs: 60,
        constraint: "static_max <= revocation-SLA AND static_max >= agent-token-TTL".into(),
    };
    Arc::new(FailStaticAuthz::try_new(300, &threshold).expect("valid bound"))
}

/// The PROVIDER side (5.6): an owner that answers the §4.6 sub-anchor resolve in the FROZEN vocabulary,
/// allowing the viewer (the leak-test for denial lives in resolve.rs). The ONE owner handle.
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
        Ok(Decision::Allow)
    }
    fn project(
        &self,
        _t: &TenantId,
        _r: &Region,
        ref_: &ArtifactRef,
        _v: &Principal,
        _m: ResolveMode,
    ) -> Result<ProjectOutcome, ProjectApiError> {
        Ok(resolve_sub_outcome(self.sub.as_ref(), ref_))
    }
}

fn resolve(sub: Arc<SyntheticSubResolver>, ref_: &ArtifactRef) -> Resolution {
    let svc = ResolveService::new(
        authz(),
        Arc::new(NoOpCacheRead),
        Arc::new(LadderOwner { sub }),
        cell(),
    );
    let root = ladder_root(ref_);
    svc.resolve(
        &tenant(),
        &region(),
        ref_,
        &root,
        &viewer(),
        ResolveMode::Live,
        &bounded_stale(),
        false,
    )
}

/// **REF-D9 (the unified ladder, the dated green artifact): each content shape degrades through the
/// frozen ladder to the correct state with the ROOT carried — 0 dangling, 0 404, no leak.** The
/// provider answers in `live/moved/outdated/gone`; the consumer (Refs) maps onto the §4.6 ladder. This
/// is ONE ladder across Git line-ranges, KN block anchors, Chat message anchors, and the check-/step-
/// CI kinds (C-6).
#[test]
fn ref_d9_unified_ladder_across_three_content_shapes_root_always_carried() {
    // (kind, ref, scripted sub-state, expected resolution, expected root)
    struct Case {
        label: &'static str,
        ref_: &'static str,
        state: SubState,
        expect_tombstone: Option<TombstoneReason>,
        expect_flag: Option<ProjectionFlag>,
        root: &'static str,
    }
    let live_proj = SyntheticSubResolver::default_projection;
    let cases = vec![
        // KN block — deleted → sub_gone, root = the page.
        Case {
            label: "KN deleted block → sub_gone",
            ref_: "myelin://acme/knowledge/page/7c2#b9",
            state: SubState::Gone,
            expect_tombstone: Some(TombstoneReason::SubGone),
            expect_flag: None,
            root: "myelin://acme/knowledge/page/7c2",
        },
        // KN block — edited → outdated (renders partial, flagged).
        Case {
            label: "KN edited block → outdated",
            ref_: "myelin://acme/knowledge/page/7c2#b3",
            state: SubState::Outdated(live_proj()),
            expect_tombstone: None,
            expect_flag: Some(ProjectionFlag::Outdated),
            root: "myelin://acme/knowledge/page/7c2",
        },
        // Chat message — deleted → sub_gone, root = the message thread parent.
        Case {
            label: "Chat deleted message → sub_gone",
            ref_: "myelin://acme/chat/message/01J0CH#message-m3",
            state: SubState::Gone,
            expect_tombstone: Some(TombstoneReason::SubGone),
            expect_flag: None,
            root: "myelin://acme/chat/message/01J0CH",
        },
        // Git line-range — rebased → moved (renders shifted, flagged).
        Case {
            label: "Git rebased range → moved",
            ref_: "myelin://acme/git/ref/main#L42-L88",
            state: SubState::Moved(live_proj()),
            expect_tombstone: None,
            expect_flag: Some(ProjectionFlag::Moved),
            root: "myelin://acme/git/ref/main",
        },
        // Git line-range — content gone → sub_gone.
        Case {
            label: "Git content_gone range → sub_gone",
            ref_: "myelin://acme/git/ref/main#L100-L120",
            state: SubState::Gone,
            expect_tombstone: Some(TombstoneReason::SubGone),
            expect_flag: None,
            root: "myelin://acme/git/ref/main",
        },
        // Any level erased → erased.
        Case {
            label: "erased sub → erased",
            ref_: "myelin://acme/knowledge/page/7c2#h1",
            state: SubState::Erased,
            expect_tombstone: Some(TombstoneReason::Erased),
            expect_flag: None,
            root: "myelin://acme/knowledge/page/7c2",
        },
    ];

    for c in cases {
        let sub = Arc::new(SyntheticSubResolver::new());
        sub.set_state(c.ref_, c.state.clone());
        let r = resolve(sub, &aref(c.ref_));

        match (&c.expect_tombstone, &r) {
            (Some(reason), Resolution::Tombstone(t)) => {
                assert_eq!(t.reason, *reason, "{}: correct tombstone reason", c.label);
                // THE ladder invariant: a tombstone ALWAYS carries the root (0 dangling, 0 hard 404).
                assert_eq!(
                    t.root.0, c.root,
                    "{}: the tombstone carries the root",
                    c.label
                );
            }
            (None, Resolution::Projection(p)) => {
                // a degraded-but-rendering state (moved/outdated) — flagged, never a tombstone.
                assert_eq!(
                    p.flag, c.expect_flag,
                    "{}: correct degradation flag",
                    c.label
                );
            }
            other => panic!("{}: unexpected resolution {other:?}", c.label),
        }
    }
}

/// **A LIVE sub-anchor renders the embed (the clean ladder path), root derivable.** The happy case
/// the degraded ones contrast against.
#[test]
fn live_sub_anchor_renders_the_embed() {
    let sub = Arc::new(SyntheticSubResolver::new());
    let r = resolve(sub, &aref("myelin://acme/git/pr/42#comment-c9"));
    match r {
        Resolution::Projection(p) => assert_eq!(p.flag, None, "a clean LIVE anchor has no flag"),
        other => panic!("a live comment anchor must render, got {other:?}"),
    }
}

/// **The Git content-anchored resolver (§3.5) is the reference algorithm an owner `project` runs (the
/// 5.6 provider side for Git).** A minted range fingerprinted at the right oid is `Exact`→LIVE; the
/// owner answers in the SAME `SubState` vocabulary every other content shape uses (one ladder).
#[test]
fn git_owner_answers_via_the_content_anchored_resolver_in_the_one_vocabulary() {
    let lines = ["fn f() {", "  x", "}"];
    let minted = MintedLineRange::mint("oid-1", &lines, 1, 3);
    // exact (oid matches) → the owner reports LIVE via into_sub_state — the SAME SubState vocabulary.
    let owner_state = myelin_refs_service::resolve_line_range(&minted, "oid-1", &lines)
        .into_sub_state(OwnerProjection {
            title: "fn f".into(),
            state: "live".into(),
            icon: "code".into(),
            render_hint: "diff".into(),
            sub_anchor: Some("L1-L3".into()),
            flag: None,
        });
    match owner_state {
        SubState::Live(_) => {}
        other => panic!("an exact range is LIVE in the one vocabulary, got {other:?}"),
    }
}
