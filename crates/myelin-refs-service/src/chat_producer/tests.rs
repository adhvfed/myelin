use std::sync::Arc;

use myelin_identity::{
    ListObjectsResult, Principal, PrincipalId, PrincipalKind, RelName, SetExpr, Zookie,
};
use myelin_refs::{mint, strip_sub, sub_kind, ArtifactRef, Sub};
use myelin_substrate::{FailStaticAuthz, FailStaticThreshold};
use myelin_tenancy::{CellId, Region, TenantId};

use super::*;
use crate::backlinks::{source_root_colref, AuthzVisibleIndex, BacklinkRead};
use crate::edge_builder::{edge_id, EdgeProjection, EdgeRow, RelClass};
use crate::emit::EdgeRel;
use crate::ladder::resolve_sub_outcome;
use crate::resolve::{bounded_stale, ProjectOutcome, ResolveMode, ResolveService, TombstoneReason};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn cell() -> CellId {
    CellId::from_token("cell-fr-par-1")
}
fn viewer(id: &str, t: &TenantId) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, t.clone())
}
fn threshold() -> FailStaticThreshold {
    FailStaticThreshold {
        status: "OPEN - LEGAL".into(),
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

fn chat_resolve_service(owner: &ChatOwner) -> ResolveService {
    ResolveService::new(
        authz(),
        Arc::new(crate::resolve::NoOpCacheRead),
        Arc::new(owner.clone()),
        cell(),
    )
}

fn message_ref(message_id: &str) -> ArtifactRef {
    myelin_chat::subs::mint_message("acme", message_id).expect("grammatical message-<id> mint")
}

fn thread_ref(thread_root_id: &str) -> ArtifactRef {
    myelin_chat::subs::mint_thread("acme", thread_root_id).expect("grammatical thread-<id> mint")
}

#[test]
fn chat_message_produces_one_edge_per_structured_node_over_every_artifact_class() {
    let producer = ChatEdgeProducer;
    let source = ChatEdgeProducer::message_root("acme", "01HMSGAAAAAAAAAAAAAAAAAAAA")
        .expect("canonical chat root");
    let mentionee = viewer("reviewer", &tenant());
    let body = vec![
        InlineNode::Mention(mentionee.clone()),
        InlineNode::ArtifactRefNode(ArtifactRef("myelin://acme/issue/issue/ENG-1".into())),
        InlineNode::Embed(ArtifactRef("myelin://acme/git/commit/core:deadbeef".into())),
        InlineNode::Embed(ArtifactRef("myelin://acme/knowledge/page/42".into())),
        InlineNode::Embed(ArtifactRef("myelin://acme/ci/run/run-9".into())),
    ];
    let edges = producer.chat_edges(&source, &body);
    assert_eq!(
        edges.len(),
        5,
        "one edge per structured node - Chat unfurls all"
    );

    let mention = &edges[0];
    assert_eq!(mention.rel, EdgeRel::Mentions);
    assert_eq!(mention.target.0, "myelin://acme/identity/member/reviewer");
    assert_eq!(edges[1].rel, EdgeRel::Links);
    assert_eq!(edges[1].target.0, "myelin://acme/issue/issue/ENG-1");
    assert!(edges[2..].iter().all(|e| e.rel == EdgeRel::Embeds));
    let embed_targets: Vec<&str> = edges[2..].iter().map(|e| e.target.0.as_str()).collect();
    assert!(embed_targets.contains(&"myelin://acme/git/commit/core:deadbeef"));
    assert!(embed_targets.contains(&"myelin://acme/knowledge/page/42"));
    assert!(embed_targets.contains(&"myelin://acme/ci/run/run-9"));
    assert!(edges
        .iter()
        .all(|e| e.source.0 == "myelin://acme/chat/message/01HMSGAAAAAAAAAAAAAAAAAAAA"));
}

#[test]
fn chat_roots_are_chat_canonical_mints() {
    let msg = ChatEdgeProducer::message_root("acme", "01HMSG").expect("canonical message root");
    assert_eq!(msg.0, "myelin://acme/chat/message/01HMSG");
    let thread = ChatEdgeProducer::thread_root("acme", "01HTHREAD").expect("canonical thread root");
    assert_eq!(thread.0, "myelin://acme/chat/thread/01HTHREAD");
    assert_eq!(CHAT_OWNER_TOKEN, "chat");
    assert!(ChatEdgeProducer::message_root("", "01HMSG").is_err());
    assert!(ChatEdgeProducer::thread_root("acme", "").is_err());
    assert_eq!(CHAT_CHANNEL_TYPE, "channel");
}

#[test]
fn live_chat_message_resolves_live() {
    let owner = ChatOwner::new();
    let ref_ = message_ref("01HMSGLIVE");
    owner.record_anchor(&ref_, ChatAnchorState::Live);
    match resolve_sub_outcome(&owner, &ref_) {
        ProjectOutcome::Live(p) => assert_eq!(p.flag, None, "a live message is a clean LIVE"),
        other => panic!("expected LIVE, got {other:?}"),
    }
}

#[test]
fn edited_chat_message_stays_live_no_outdated_arm() {
    let owner = ChatOwner::new();
    let ref_ = message_ref("01HMSGEDIT");
    owner.record_anchor(&ref_, ChatAnchorState::Live);
    match resolve_sub_outcome(&owner, &ref_) {
        ProjectOutcome::Live(p) => assert_eq!(
            p.flag, None,
            "an edited message keeps its id - LIVE, never OUTDATED (Chat is content-addressed)"
        ),
        other => panic!("expected LIVE, got {other:?}"),
    }
}

#[test]
fn deleted_chat_message_tombstones_carrying_the_root() {
    let owner = ChatOwner::new();
    let ref_ = message_ref("01HMSGDEL");
    owner.record_anchor(&ref_, ChatAnchorState::Deleted);

    let svc = chat_resolve_service(&owner);
    let v = viewer("member", &tenant());
    owner.grant_view(&tenant(), &region(), &v, &strip_sub(&ref_));
    let res = svc.resolve(
        &tenant(),
        &region(),
        &ref_,
        &strip_sub(&ref_),
        &v,
        ResolveMode::Live,
        &bounded_stale(),
        false,
    );
    assert!(res.is_tombstone(), "deleted message → tombstone");
    assert_eq!(res.tombstone_reason(), Some(TombstoneReason::SubGone));
    if let crate::resolve::Resolution::Tombstone(t) = res {
        assert_eq!(t.root, strip_sub(&ref_));
        assert_eq!(t.root.0, "myelin://acme/chat/message/01HMSGDEL");
    }
}

#[test]
fn chat_thread_anchor_degrades_through_the_ladder() {
    let owner = ChatOwner::new();
    let live = thread_ref("01HTHRLIVE");
    let dead = thread_ref("01HTHRDEAD");
    owner.record_anchor(&live, ChatAnchorState::Live);
    owner.record_anchor(&dead, ChatAnchorState::Deleted);
    match resolve_sub_outcome(&owner, &live) {
        ProjectOutcome::Live(p) => assert_eq!(p.flag, None),
        other => panic!("live thread → LIVE, got {other:?}"),
    }
    assert_eq!(resolve_sub_outcome(&owner, &dead), ProjectOutcome::SubGone);
}

#[test]
fn erased_chat_message_is_an_erased_tombstone() {
    let owner = ChatOwner::new();
    let ref_ = message_ref("01HMSGERASED");
    owner.record_anchor(&ref_, ChatAnchorState::Erased);
    assert_eq!(resolve_sub_outcome(&owner, &ref_), ProjectOutcome::Erased);
}

#[test]
fn unscripted_chat_message_anchor_is_gone_not_a_leak() {
    let owner = ChatOwner::new();
    let ref_ = message_ref("01HMSGNEVER");
    assert_eq!(resolve_sub_outcome(&owner, &ref_), ProjectOutcome::SubGone);
}

#[test]
fn a_bare_chat_root_is_live() {
    let owner = ChatOwner::new();
    let root = ChatEdgeProducer::message_root("acme", "01HMSGROOT").expect("canonical chat root");
    assert!(matches!(
        resolve_sub_outcome(&owner, &root),
        ProjectOutcome::Live(_)
    ));
}

#[test]
fn chat_message_ref_classifies_through_the_one_grammar() {
    let ref_ = message_ref("01HMSGCLASS");
    assert_eq!(sub_kind(&ref_), Some(Sub::Message("01HMSGCLASS".into())));
    assert_eq!(strip_sub(&ref_).0, "myelin://acme/chat/message/01HMSGCLASS");
    let tref = thread_ref("01HTHRCLASS");
    assert_eq!(sub_kind(&tref), Some(Sub::Thread("01HTHRCLASS".into())));
}

#[test]
fn ref_d1_denied_viewer_of_a_private_channel_message_is_tombstoned() {
    let owner = ChatOwner::new();
    let msg = ChatEdgeProducer::message_root("acme", "01HMSGSECRET").expect("canonical chat root");
    let outsider = viewer("non-member", &tenant());
    let svc = chat_resolve_service(&owner);
    let res = svc.resolve(
        &tenant(),
        &region(),
        &msg,
        &msg,
        &outsider,
        ResolveMode::Live,
        &bounded_stale(),
        false,
    );
    assert!(res.is_tombstone(), "a non-member is tombstoned");
    assert_eq!(res.tombstone_reason(), Some(TombstoneReason::Denied));
    if let crate::resolve::Resolution::Tombstone(t) = res {
        assert_eq!(
            t.root, msg,
            "the tombstone carries only the root, never the message title"
        );
    }
    let member = viewer("member", &tenant());
    owner.grant_view(&tenant(), &region(), &member, &msg);
    let res = svc.resolve(
        &tenant(),
        &region(),
        &msg,
        &msg,
        &member,
        ResolveMode::Live,
        &bounded_stale(),
        false,
    );
    assert!(!res.is_tombstone(), "a member resolves LIVE");
}

#[test]
fn chained_deleted_chat_message_others_embed_resolves_gone_with_root() {
    let owner = ChatOwner::new();
    let decision = message_ref("01HMSGDECISION");
    owner.record_anchor(&decision, ChatAnchorState::Deleted);

    let svc = chat_resolve_service(&owner);
    let v = viewer("doc-reader", &tenant());
    owner.grant_view(&tenant(), &region(), &v, &strip_sub(&decision));
    let res = svc.resolve(
        &tenant(),
        &region(),
        &decision,
        &strip_sub(&decision),
        &v,
        ResolveMode::Live,
        &bounded_stale(),
        false,
    );
    assert!(
        res.is_tombstone(),
        "the embedded-but-deleted message → tombstone"
    );
    assert_eq!(
        res.tombstone_reason(),
        Some(TombstoneReason::SubGone),
        "sub-gone (the message was deleted), NOT denied (the viewer is a member)"
    );
    if let crate::resolve::Resolution::Tombstone(t) = res {
        assert_eq!(
            t.root.0, "myelin://acme/chat/message/01HMSGDECISION",
            "the embed degrades to the parent chat message, never a dangling 404"
        );
    }
}

#[test]
fn chat_channel_fragment_flows_through_list_objects_nonmember_returns_zero() {
    let t = tenant();
    let r = region();
    let edges = EdgeProjection::new();

    let issue = "myelin://acme/issue/issue/ENG-1";
    let public_msg = "myelin://acme/chat/message/01HPUBLIC";
    let private_msg = "myelin://acme/chat/message/01HPRIVATE";
    for src in [public_msg, private_msg] {
        let id = edge_id(&t, src, issue, "links");
        edges.upsert(
            &t,
            &r,
            EdgeRow {
                edge_id: id.clone(),
                source: ArtifactRef(src.into()),
                source_root: strip_sub(&ArtifactRef(src.into())),
                target: ArtifactRef(issue.into()),
                target_root: strip_sub(&ArtifactRef(issue.into())),
                rel: "links".into(),
                rel_class: RelClass::Reference,
                origin_event: format!("evt-{id}"),
                origin_actor: "chat-pseudonym".into(),
                zookie: Some("zk-1".into()),
                tombstoned: false,
            },
        );
    }

    let authz = AuthzVisibleIndex::new();
    let nonmember = viewer("outsider", &t);
    authz.grant(
        &t,
        &r,
        &nonmember.principal_id.0,
        "view",
        public_msg,
        "zk-1",
    );

    let read = BacklinkRead::new(edges, authz);
    let list_objects = ListObjectsResult::Filter {
        set_expr: SetExpr::InRelation {
            relation: RelName("view".into()),
            via_column: source_root_colref(),
        },
        zookie: Zookie("zk-1".into()),
    };
    let page = read
        .backlinks(
            &t,
            &r,
            &ArtifactRef(issue.into()),
            &nonmember,
            &list_objects,
            &bounded_stale(),
            10,
        )
        .expect("backlink read");
    assert_eq!(
        page.edges.len(),
        1,
        "the Chat fragment's member-gated read admits only the public channel's backlink"
    );
    assert_eq!(
        page.edges[0].source.0, public_msg,
        "the private channel's chat backlink is ABSENT for the non-member (0 leak)"
    );
}

#[test]
fn chat_nonmember_with_no_membership_sees_zero_backlinks() {
    let t = tenant();
    let r = region();
    let edges = EdgeProjection::new();
    let issue = "myelin://acme/issue/issue/ENG-2";
    let msg = "myelin://acme/chat/message/01HONLY";
    let id = edge_id(&t, msg, issue, "links");
    edges.upsert(
        &t,
        &r,
        EdgeRow {
            edge_id: id.clone(),
            source: ArtifactRef(msg.into()),
            source_root: strip_sub(&ArtifactRef(msg.into())),
            target: ArtifactRef(issue.into()),
            target_root: strip_sub(&ArtifactRef(issue.into())),
            rel: "links".into(),
            rel_class: RelClass::Reference,
            origin_event: format!("evt-{id}"),
            origin_actor: "chat-pseudonym".into(),
            zookie: Some("zk-1".into()),
            tombstoned: false,
        },
    );
    let authz = AuthzVisibleIndex::new();
    authz.advance_watermark(&t, &r, "zk-1");
    let read = BacklinkRead::new(edges, authz);
    let nonmember = viewer("ghost", &t);
    let list_objects = ListObjectsResult::Filter {
        set_expr: SetExpr::InRelation {
            relation: RelName("view".into()),
            via_column: source_root_colref(),
        },
        zookie: Zookie("zk-1".into()),
    };
    let page = read
        .backlinks(
            &t,
            &r,
            &ArtifactRef(issue.into()),
            &nonmember,
            &list_objects,
            &bounded_stale(),
            10,
        )
        .expect("backlink read");
    assert_eq!(
        page.edges.len(),
        0,
        "a member of no channel sees 0 chat backlinks (the non-member-returns-0 GATE)"
    );
}

#[test]
fn chat_terminal_node_unfurls_every_prior_producer_class() {
    let producer = ChatEdgeProducer;
    let source =
        ChatEdgeProducer::message_root("acme", "01HMSGDECIDE").expect("canonical chat root");
    let body = vec![
        InlineNode::ArtifactRefNode(ArtifactRef("myelin://acme/issue/issue/PLAT-9".into())),
        InlineNode::Embed(ArtifactRef("myelin://acme/git/commit/core:cafe".into())),
        InlineNode::Embed(ArtifactRef("myelin://acme/ci/run/run-42".into())),
        InlineNode::Embed(ArtifactRef("myelin://acme/knowledge/page/7".into())),
        InlineNode::Embed(ArtifactRef("myelin://acme/chat/message/01HOTHER".into())),
    ];
    let edges = producer.chat_edges(&source, &body);
    let subsystems: std::collections::BTreeSet<&str> = edges
        .iter()
        .map(|e| e.target.0.split('/').nth(3).unwrap())
        .collect();
    assert_eq!(
        subsystems,
        ["chat", "ci", "git", "issue", "knowledge"]
            .into_iter()
            .collect(),
        "Chat unfurls every prior producer class - cross-subsystem traversal complete"
    );
}

#[test]
fn chat_thread_edge_source_is_the_stripped_thread_root() {
    let producer = ChatEdgeProducer;
    let source =
        ChatEdgeProducer::thread_root("acme", "01HTHREDGE").expect("canonical thread root");
    let body = vec![InlineNode::ArtifactRefNode(ArtifactRef(
        "myelin://acme/issue/issue/ENG-7".into(),
    ))];
    let edges = producer.chat_edges(&source, &body);
    assert_eq!(edges.len(), 1);
    assert_eq!(
        edges[0].source.0, "myelin://acme/chat/thread/01HTHREDGE",
        "the edge is sourced from the canonical thread root"
    );
}

#[test]
fn chat_mint_is_grammatical_by_construction() {
    let root = ChatEdgeProducer::message_root("acme", "01HMSGMINT").expect("canonical chat root");
    let minted = mint(&root, Sub::Message("01HMSGMINT2".into())).expect("grammatical mint");
    assert_eq!(sub_kind(&minted), Some(Sub::Message("01HMSGMINT2".into())));
}
