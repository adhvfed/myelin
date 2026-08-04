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
        status: "OPEN - LEGAL".into(),
        owner: "DPO / Legal".into(),
        static_max_secs: None,
        static_max_default_secs: 300,
        agent_token_ttl_secs: 60,
        constraint: "static_max <= revocation-SLA AND static_max >= agent-token-TTL".into(),
    };
    Arc::new(FailStaticAuthz::try_new(300, &threshold).expect("valid bound"))
}

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

#[test]
fn ref_d9_unified_ladder_across_three_content_shapes_root_always_carried() {
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
        Case {
            label: "KN deleted block → sub_gone",
            ref_: "myelin://acme/knowledge/page/7c2#b9",
            state: SubState::Gone,
            expect_tombstone: Some(TombstoneReason::SubGone),
            expect_flag: None,
            root: "myelin://acme/knowledge/page/7c2",
        },
        Case {
            label: "KN edited block → outdated",
            ref_: "myelin://acme/knowledge/page/7c2#b3",
            state: SubState::Outdated(live_proj()),
            expect_tombstone: None,
            expect_flag: Some(ProjectionFlag::Outdated),
            root: "myelin://acme/knowledge/page/7c2",
        },
        Case {
            label: "Chat deleted message → sub_gone",
            ref_: "myelin://acme/chat/message/01J0CH#message-m3",
            state: SubState::Gone,
            expect_tombstone: Some(TombstoneReason::SubGone),
            expect_flag: None,
            root: "myelin://acme/chat/message/01J0CH",
        },
        Case {
            label: "Git rebased range → moved",
            ref_: "myelin://acme/git/ref/main#L42-L88",
            state: SubState::Moved(live_proj()),
            expect_tombstone: None,
            expect_flag: Some(ProjectionFlag::Moved),
            root: "myelin://acme/git/ref/main",
        },
        Case {
            label: "Git content_gone range → sub_gone",
            ref_: "myelin://acme/git/ref/main#L100-L120",
            state: SubState::Gone,
            expect_tombstone: Some(TombstoneReason::SubGone),
            expect_flag: None,
            root: "myelin://acme/git/ref/main",
        },
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
                assert_eq!(
                    t.root.0, c.root,
                    "{}: the tombstone carries the root",
                    c.label
                );
            }
            (None, Resolution::Projection(p)) => {
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

#[test]
fn live_sub_anchor_renders_the_embed() {
    let sub = Arc::new(SyntheticSubResolver::new());
    let r = resolve(sub, &aref("myelin://acme/git/pr/42#comment-c9"));
    match r {
        Resolution::Projection(p) => assert_eq!(p.flag, None, "a clean LIVE anchor has no flag"),
        other => panic!("a live comment anchor must render, got {other:?}"),
    }
}

#[test]
fn git_owner_answers_via_the_content_anchored_resolver_in_the_one_vocabulary() {
    let lines = ["fn f() {", "  x", "}"];
    let minted = MintedLineRange::mint("oid-1", &lines, 1, 3);
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
