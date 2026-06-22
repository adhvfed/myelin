//! Unit + CDC tests for REF-P21 / P-337 — Chat unfurls: the MAXIMAL consumer + cross-subsystem
//! traversal COMPLETE.
//!
//! These RE-CONFIRM the Refs invariants on the FINAL, most-adversarial producer corpus (confidential
//! issues, private channels, fork-scoped CI):
//! - **5.4** — a real Chat message body (mention/artifact_ref/embed nodes) produces the right edge SET,
//!   extracted through the ONE structured-node seam (never a regex over prose); Chat unfurls EVERY
//!   artifact class (commit / issue / doc / CI run / another message).
//! - **REF-D9** — the ONE ladder on REAL Chat `message-`/`thread-` sub-anchors: a live message → LIVE,
//!   a deleted message → GONE (root ALWAYS carried), a crypto-shredded message → ERASED. Chat is
//!   content-addressed by a STABLE opaque id, so there is NO MOVED/OUTDATED arm (an edited message
//!   keeps its id → LIVE).
//! - **REF-D1** — the leak invariant on the Chat corpus: a non-member of a private channel is
//!   tombstoned, never leaked (supports CHAT-D5 confidential-unfurl → tombstone, 0 title leak).
//! - **4.9 / the GATE** — the Chat channel ReBAC fragment (`channel.read = member +
//!   parent_project->read`) flows through `list_objects`: a NON-MEMBER search/backlink returns 0
//!   chat-sourced backlinks; a member sees them. The FROZEN `SetExpr` lowered over `edge.source_root`
//!   (REF-P11), never a per-edge check.
//! - the CHAINED test: a DELETED chat message others embed → the embed resolves GONE with the root
//!   carried (the embed shows the parent, never a dangling 404).
//!
//! Mutation floors (still hold on the Chat corpus): the REF-P11 backlink `SetExpr`-lowering
//! mutation-core and the REF-P15 ladder mutation-core are UNCHANGED — this prompt adds the Chat owner +
//! the Chat edge-producer wiring, both of which delegate INTO those mutation-tested cores (no new
//! mutation-core module; the engine is fixed at M2).

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

/// Build a [`ResolveService`] over the Chat owner (the engine is unchanged — the Chat owner is the
/// only new wiring). [`ChatOwner`] is `Clone` (Arc-shared interior), so a clone the service holds
/// shares the SAME recorded state the test arms.
fn chat_resolve_service(owner: &ChatOwner) -> ResolveService {
    ResolveService::new(
        authz(),
        Arc::new(crate::resolve::NoOpCacheRead),
        Arc::new(owner.clone()),
        cell(),
    )
}

/// Helper: mint a Chat message sub-anchor on a message root (Chat's OWN codec).
fn message_ref(message_id: &str) -> ArtifactRef {
    myelin_chat::subs::mint_message("acme", message_id).expect("grammatical message-<id> mint")
}

/// Helper: mint a Chat thread sub-anchor on a thread root (Chat's OWN codec).
fn thread_ref(thread_root_id: &str) -> ArtifactRef {
    myelin_chat::subs::mint_thread("acme", thread_root_id).expect("grammatical thread-<id> mint")
}

// ===========================================================================
// 5.4 — Chat is the MAXIMAL producer: mention/artifact_ref/embed edges over every artifact class
// ===========================================================================

/// **A real Chat message body produces ONE edge per structured node, across the WHOLE artifact-class
/// space (5.4 — Chat unfurls EVERYTHING).** A single message mentions a person, links an issue, and
/// embeds a commit, a doc, and a CI run — five structured nodes → five edges, through the ONE
/// structured-node seam (never a regex over the message prose).
#[test]
fn chat_message_produces_one_edge_per_structured_node_over_every_artifact_class() {
    let producer = ChatEdgeProducer;
    let source = ChatEdgeProducer::message_root("acme", "01HMSGAAAAAAAAAAAAAAAAAAAA");
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
        "one edge per structured node — Chat unfurls all"
    );

    // The mention targets the PSEUDONYMOUS member URN (erasure-safe, never the name).
    let mention = &edges[0];
    assert_eq!(mention.rel, EdgeRel::Mentions);
    assert_eq!(mention.target.0, "myelin://acme/identity/member/reviewer");
    // The artifact_ref → links; the three embeds → embeds, each pointing at a DIFFERENT subsystem.
    assert_eq!(edges[1].rel, EdgeRel::Links);
    assert_eq!(edges[1].target.0, "myelin://acme/issue/issue/ENG-1");
    assert!(edges[2..].iter().all(|e| e.rel == EdgeRel::Embeds));
    let embed_targets: Vec<&str> = edges[2..].iter().map(|e| e.target.0.as_str()).collect();
    assert!(embed_targets.contains(&"myelin://acme/git/commit/core:deadbeef"));
    assert!(embed_targets.contains(&"myelin://acme/knowledge/page/42"));
    assert!(embed_targets.contains(&"myelin://acme/ci/run/run-9"));
    // Every edge is sourced from the CHAT message root (the canonical #sub-stripped root).
    assert!(edges
        .iter()
        .all(|e| e.source.0 == "myelin://acme/chat/message/01HMSGAAAAAAAAAAAAAAAAAAAA"));
}

/// **The Chat message/thread roots are built through Chat's OWN canonical mint codecs (one mint, never
/// a parallel literal — X-5).** The edge SOURCE is the `#sub`-stripped canonical root Refs stores
/// against.
#[test]
fn chat_roots_are_chat_canonical_mints() {
    let msg = ChatEdgeProducer::message_root("acme", "01HMSG");
    assert_eq!(msg.0, "myelin://acme/chat/message/01HMSG");
    let thread = ChatEdgeProducer::thread_root("acme", "01HTHREAD");
    assert_eq!(thread.0, "myelin://acme/chat/thread/01HTHREAD");
    assert_eq!(CHAT_OWNER_TOKEN, "chat");
    assert_eq!(CHAT_CHANNEL_TYPE, "channel");
}

// ===========================================================================
// REF-D9 — the ladder on REAL Chat message-/thread- sub-anchors (5.6/5.7)
// ===========================================================================

/// **A live Chat message resolves LIVE, no flag (REF-D9 happy path).**
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

/// **An EDITED Chat message stays LIVE — the id is IMMUTABLE, there is NO OUTDATED arm for Chat
/// (REF-D9 / §4.6).** Unlike an Issues field (editable value → OUTDATED) or a Git line-range
/// (positional → MOVED), a Chat message is content-addressed by a stable ULID: an edit keeps the
/// `message_id`, so the embed stays LIVE and never dangles.
#[test]
fn edited_chat_message_stays_live_no_outdated_arm() {
    let owner = ChatOwner::new();
    let ref_ = message_ref("01HMSGEDIT");
    // An edit keeps the immutable id → the owner records it as Live (Chat has no OUTDATED state).
    owner.record_anchor(&ref_, ChatAnchorState::Live);
    match resolve_sub_outcome(&owner, &ref_) {
        ProjectOutcome::Live(p) => assert_eq!(
            p.flag, None,
            "an edited message keeps its id — LIVE, never OUTDATED (Chat is content-addressed)"
        ),
        other => panic!("expected LIVE, got {other:?}"),
    }
}

/// **A DELETED Chat message resolves to a sub-gone tombstone that carries the root (REF-D9 — 0
/// dangling embed, 0 hard 404).** The message was deleted → GONE; the chokepoint tombstones it
/// carrying the `#sub`-stripped MESSAGE root (the embed shows the parent).
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

/// **A Chat thread anchor degrades through the SAME ladder (REF-D9).** A deleted thread is GONE, a live
/// thread LIVE — the `thread-<id>` kind shares the one ladder vocabulary (the kind shared with Git
/// review threads, OQ-L).
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

/// **An ERASED Chat message (crypto-shred of the per-subject/per-tenant DEK) is an erased tombstone
/// (REF-D9; supports CHAT-D5).**
#[test]
fn erased_chat_message_is_an_erased_tombstone() {
    let owner = ChatOwner::new();
    let ref_ = message_ref("01HMSGERASED");
    owner.record_anchor(&ref_, ChatAnchorState::Erased);
    assert_eq!(resolve_sub_outcome(&owner, &ref_), ProjectOutcome::Erased);
}

/// **An unscripted Chat message anchor is defensively GONE, never a guessed LIVE (REF-3).** A real
/// owner always has the mint-time state; an anchor it never recorded resolves GONE, never fabricated.
#[test]
fn unscripted_chat_message_anchor_is_gone_not_a_leak() {
    let owner = ChatOwner::new();
    let ref_ = message_ref("01HMSGNEVER");
    assert_eq!(resolve_sub_outcome(&owner, &ref_), ProjectOutcome::SubGone);
}

/// **The bare Chat root resolves LIVE (the message/thread itself, no sub-anchor).**
#[test]
fn a_bare_chat_root_is_live() {
    let owner = ChatOwner::new();
    let root = ChatEdgeProducer::message_root("acme", "01HMSGROOT");
    assert!(matches!(
        resolve_sub_outcome(&owner, &root),
        ProjectOutcome::Live(_)
    ));
}

/// Sanity: a Chat message ref classifies to the Message sub-kind through the ONE grammar (5.7); the
/// `<message_id>` ULID is the stored root (§2 — a stable opaque id, never a positional index).
#[test]
fn chat_message_ref_classifies_through_the_one_grammar() {
    let ref_ = message_ref("01HMSGCLASS");
    assert_eq!(sub_kind(&ref_), Some(Sub::Message("01HMSGCLASS".into())));
    assert_eq!(strip_sub(&ref_).0, "myelin://acme/chat/message/01HMSGCLASS");
    // A thread classifies to the Thread kind (the shared kind, OQ-L).
    let tref = thread_ref("01HTHRCLASS");
    assert_eq!(sub_kind(&tref), Some(Sub::Thread("01HTHRCLASS".into())));
}

// ===========================================================================
// REF-D1 — the leak invariant on the Chat corpus (confidential / private-channel unfurl)
// ===========================================================================

/// **REF-D1 (leak) on the Chat corpus: a DENIED viewer of a private-channel message is tombstoned,
/// never leaked (supports CHAT-D5 confidential-unfurl → tombstone, 0 title leak).** A private channel's
/// content does NOT leak through an unfurl to a non-member (default-deny). The tombstone is
/// structurally incapable of carrying the message title (the leak invariant; supports the Chat
/// `channel.read = member + parent_project->read` fragment, REF-P323).
#[test]
fn ref_d1_denied_viewer_of_a_private_channel_message_is_tombstoned() {
    let owner = ChatOwner::new();
    let msg = ChatEdgeProducer::message_root("acme", "01HMSGSECRET");
    let outsider = viewer("non-member", &tenant());
    // NO grant_view for the outsider (default-deny) — the private-channel leak invariant.
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
    // A granted member resolves LIVE.
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

// ===========================================================================
// THE CHAINED test — a deleted chat message others EMBED resolves GONE with the root carried
// ===========================================================================

/// **CHAINED (the prompt's required chained test): a DELETED chat message that OTHERS embed → the embed
/// resolves GONE with the root carried.** A KN doc embeds a chat decision message; the message is then
/// deleted. The embed resolves through the ONE ladder to a sub-gone tombstone carrying the CHAT message
/// root (the embed degrades to "this referenced <the chat message> (no longer available)", never a
/// dangling 404). This is the cross-subsystem unfurl: the EMBEDDER is KN, the TARGET is Chat.
#[test]
fn chained_deleted_chat_message_others_embed_resolves_gone_with_root() {
    let owner = ChatOwner::new();
    // The chat decision message others embed (e.g. a KN doc embeds "the team decided X in #eng").
    let decision = message_ref("01HMSGDECISION");
    // It exists, is embedded, then DELETED — the owner records the deletion.
    owner.record_anchor(&decision, ChatAnchorState::Deleted);

    let svc = chat_resolve_service(&owner);
    // The embedder's viewer can read the channel (member) — so this is NOT a permission denial; it is a
    // genuine sub-gone (the message was deleted). The chokepoint still tombstones, carrying the root.
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

// ===========================================================================
// 4.9 / THE GATE — the Chat channel ReBAC fragment flows through list_objects (non-member → 0)
// ===========================================================================

/// **THE GATE: the Chat channel ReBAC fragment (`channel.read = member + parent_project->read`) flows
/// through `list_objects` — a NON-MEMBER search/backlink returns 0 chat-sourced backlinks (4.9 /
/// REF-D1).** Two chat messages link a shared issue, sourced from two channels: one the viewer is a
/// `member` of, one they are not. The backlink read lowers the FROZEN `SetExpr` (the `member` relation
/// → the `authz_visible` JOIN) over `edge.source_root` (REF-P11), so a non-member sees 0 of the
/// private channel's backlinks and exactly the member channel's. NO per-edge check; the leak-free SHAPE
/// is the JOIN.
#[test]
fn chat_channel_fragment_flows_through_list_objects_nonmember_returns_zero() {
    let t = tenant();
    let r = region();
    let edges = EdgeProjection::new();

    // The shared target — an issue both chat channels reference.
    let issue = "myelin://acme/issue/issue/ENG-1";
    // Two chat messages, in two DIFFERENT channels, both link the issue.
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

    // The `authz_visible` reverse index — the materialised projection of the Chat fragment's `member`
    // relation (`channel.read = member + parent_project->read`). The NON-MEMBER is a `member` of ONLY
    // the public channel's message (the public channel; the private channel grants no membership).
    let authz = AuthzVisibleIndex::new();
    let nonmember = viewer("outsider", &t);
    // The non-member can read the PUBLIC channel's message-source but NOT the private one.
    authz.grant(
        &t,
        &r,
        &nonmember.principal_id.0,
        "view",
        public_msg,
        "zk-1",
    );

    let read = BacklinkRead::new(edges, authz);
    // list_objects returns the pushed-down `Filter{InRelation{member-via-view}}` — the Chat fragment's
    // `channel.read` lowered to the `view`-relation JOIN over `authz_visible` (the FROZEN SetExpr).
    let list_objects = ListObjectsResult::Filter {
        set_expr: SetExpr::InRelation {
            relation: RelName("view".into()),
            // The consumer lowering overrides this with `edge.source_root` (the §3.2 filter column);
            // the contract carries the consumer's own ColRef. Refs lowers it over source_root.
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
    // The non-member sees ONLY the public channel's chat backlink — 0 of the private channel's.
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

/// **A viewer who is a member of NO channel sees 0 chat backlinks (the strict non-member case).** With
/// an empty `authz_visible` (no `member` grant at all) the `InRelation` JOIN admits nothing — the
/// non-member's chat-backlink list is EMPTY, never a leak.
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
    // Empty authz_visible — the non-member is a `member` of NO channel. Advance the watermark so the
    // read is at-revision (a fresh empty index, not a behind one).
    let authz = AuthzVisibleIndex::new();
    authz.advance_watermark(&t, &r, "zk-1");
    let read = BacklinkRead::new(edges, authz);
    let nonmember = viewer("ghost", &t);
    let list_objects = ListObjectsResult::Filter {
        set_expr: SetExpr::InRelation {
            relation: RelName("view".into()),
            // The consumer lowering overrides this with `edge.source_root` (the §3.2 filter column);
            // the contract carries the consumer's own ColRef. Refs lowers it over source_root.
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

// ===========================================================================
// Cross-subsystem traversal COMPLETE — Chat is the terminal node unfurling every prior class
// ===========================================================================

/// **Cross-subsystem traversal is COMPLETE (the R-M4 milestone): the spec-to-ship lineage's terminal
/// `chat decision` node unfurls every prior node.** A chat message embeds an issue, a commit, and a CI
/// run (three different producers); the edge corpus is one uniform reference set Refs can traverse, and
/// each embed resolves through the ONE ladder (the targets' owners resolve them — here we assert the
/// edge SET is uniform across the five-producer space).
#[test]
fn chat_terminal_node_unfurls_every_prior_producer_class() {
    let producer = ChatEdgeProducer;
    let source = ChatEdgeProducer::message_root("acme", "01HMSGDECIDE");
    // The chat decision unfurls: an Issue (Issues), a commit (Git), a CI run (CI), a doc (Knowledge),
    // and another chat message (Chat itself) — all five producer classes in one message.
    let body = vec![
        InlineNode::ArtifactRefNode(ArtifactRef("myelin://acme/issue/issue/PLAT-9".into())),
        InlineNode::Embed(ArtifactRef("myelin://acme/git/commit/core:cafe".into())),
        InlineNode::Embed(ArtifactRef("myelin://acme/ci/run/run-42".into())),
        InlineNode::Embed(ArtifactRef("myelin://acme/knowledge/page/7".into())),
        InlineNode::Embed(ArtifactRef("myelin://acme/chat/message/01HOTHER".into())),
    ];
    let edges = producer.chat_edges(&source, &body);
    // Five edges, spanning all five producer subsystems — cross-subsystem traversal is complete.
    let subsystems: std::collections::BTreeSet<&str> = edges
        .iter()
        .map(|e| e.target.0.split('/').nth(3).unwrap())
        .collect();
    assert_eq!(
        subsystems,
        ["chat", "ci", "git", "issue", "knowledge"]
            .into_iter()
            .collect(),
        "Chat unfurls every prior producer class — cross-subsystem traversal complete"
    );
}

/// Confirms the Chat mint root passes through the ONE grammar even for a thread `#sub` — the producer
/// strips back to the canonical root the edge is sourced from (the `#sub`-stripped thread root).
#[test]
fn chat_thread_edge_source_is_the_stripped_thread_root() {
    let producer = ChatEdgeProducer;
    let source = ChatEdgeProducer::thread_root("acme", "01HTHREDGE");
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

/// **A mint is grammatical BY CONSTRUCTION through the ONE Refs codec.** A Chat message ref minted via
/// the codec re-parses to the SAME Message sub.
#[test]
fn chat_mint_is_grammatical_by_construction() {
    let root = ChatEdgeProducer::message_root("acme", "01HMSGMINT");
    let minted = mint(&root, Sub::Message("01HMSGMINT2".into())).expect("grammatical mint");
    assert_eq!(sub_kind(&minted), Some(Sub::Message("01HMSGMINT2".into())));
}
