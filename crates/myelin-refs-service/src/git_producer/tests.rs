//! Unit + CDC tests for REF-P17 / P-258 — Refs consumes the REAL Git producer edges + content-
//! anchored line-range sub-anchors + per-blob/ref replay.
//!
//! These RE-CONFIRM the Refs invariants on a production-shaped Git corpus (not the M2 synthetic
//! corpus): REF-D1 (leak: a denied viewer is tombstoned, never leaked), REF-D2 (IDOR / cross-tenant:
//! a viewer of tenant B cannot read tenant A's Git backlinks), REF-D9 (the ladder on REAL Git
//! sub-anchors: a force-pushed line range resolves MOVED/OUTDATED/GONE with the root ALWAYS carried;
//! a Git comment/thread anchor degrades through the same ladder), REF-D4 (reindex-parity over a Git
//! edge corpus — the DB-free CDC half; the live-Postgres proof is the integration test). The engine
//! is UNCHANGED — these prove the Git WIRING drives the engine correctly.

use std::sync::Arc;

use myelin_content::InlineNode;
use myelin_events::ArtifactRef;
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs::{strip_sub, sub_kind};
use myelin_substrate::{FailStaticAuthz, FailStaticThreshold};
use myelin_tenancy::{CellId, Region, TenantId};

use super::*;
use crate::backlinks::{AuthzVisibleIndex, BacklinkRead};
use crate::edge_builder::{edge_id, EdgeProjection, EdgeRow, RelClass};
use crate::ladder::{resolve_sub_outcome, MintedLineRange};
use crate::resolve::{bounded_stale, ProjectOutcome, ResolveMode, ResolveService, TombstoneReason};

fn tenant() -> TenantId {
    TenantId("acme-eu".into())
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

// ===========================================================================
// 5.4 — the REAL Git producer edges (commit-trailer / PR-link / "Closes <issue>")
// ===========================================================================

/// A real Git PR body extracts exactly the structured ref edges (a PR-link, a "Closes <issue>"
/// artifact ref, an `@`-reviewer mention) — one edge per structured node, NOT a regex over the PR
/// prose. The producer is no longer synthetic — it is a Git PR body. (Contract 5.4.)
#[test]
fn real_git_pr_body_extracts_one_edge_per_structured_ref_node() {
    let producer = GitEdgeProducer;
    let source = GitEdgeProducer::pr_root("acme-eu", "repo7", 4291);

    // The PR description: "Closes ENG-12, see #4288. /cc @alice" — structured nodes (not prose).
    let body = vec![
        InlineNode::ArtifactRefNode(ArtifactRef("myelin://acme-eu/issue/issue/ENG-12".into())),
        InlineNode::ArtifactRefNode(ArtifactRef("myelin://acme-eu/git/pr/repo7:4288".into())),
        InlineNode::Mention(viewer("alice", &tenant())),
    ];
    let edges = producer.git_edges(&source, &body);
    assert_eq!(edges.len(), 3, "three structured ref nodes → three edges");
    // Every edge sources from the Git PR root (the referencing side).
    for e in &edges {
        assert_eq!(e.source, source);
        assert_eq!(e.rel_class, RelClass::Reference);
    }
    // The "Closes ENG-12" link → a `links` edge to the issue.
    assert_eq!(edges[0].target.0, "myelin://acme-eu/issue/issue/ENG-12");
    assert_eq!(edges[0].rel.as_str(), "links");
    // The `@alice` reviewer mention targets the PSEUDONYMOUS member URN, never the name (erasure-safe).
    assert_eq!(edges[2].target.0, "myelin://acme-eu/identity/member/alice");
    assert_eq!(edges[2].rel.as_str(), "mentions");
}

/// A commit-trailer "Closes ENG-7" reference sources from the Git COMMIT root (the §2 `commit/<repo>:<oid>`
/// canonical key). Proves the commit producer side (a commit message body's structured ref).
#[test]
fn commit_trailer_reference_sources_from_the_commit_root() {
    let producer = GitEdgeProducer;
    let source = GitEdgeProducer::commit_root("acme-eu", "repo7", "abc123def");
    let body = vec![InlineNode::ArtifactRefNode(ArtifactRef(
        "myelin://acme-eu/issue/issue/ENG-7".into(),
    ))];
    let edges = producer.git_edges(&source, &body);
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].source.0, "myelin://acme-eu/git/commit/repo7:abc123def");
    assert_eq!(edges[0].target.0, "myelin://acme-eu/issue/issue/ENG-7");
}

// ===========================================================================
// REF-D9 — the ladder on REAL Git content-anchored line-range sub-anchors (the Refs half of GIT-D7)
// ===========================================================================

/// **A force-pushed Git PR line-range others embed resolves MOVED, the root carried (REF-D9).** A
/// `#L42-L44` is minted content-anchored (BLAKE3 fingerprint + blob oid). A force-push rewrites the
/// blob (new oid) but the fingerprinted lines survive at a SHIFTED position → the resolver returns
/// MOVED (a `Live` projection flagged `moved`), and the resolved anchor reflects the SHIFTED range,
/// never the stale raw `L42-L44`.
#[test]
fn force_pushed_line_range_resolves_moved_with_shifted_anchor() {
    let owner = GitOwner::new();
    let ref_ = myelin_git::subs::mint_blob_line_range("acme-eu", "repo7", "main", "src/lib.rs", 42, 44)
        .expect("grammatical line-range mint");

    // Mint-time blob: the anchored lines at 42..=44 of the ORIGINAL blob.
    let original = [
        "// header", "use std::fmt;", "", // 1..3
        "fn answer() -> u32 {", "    let x = 40;", "    x + 2", // 4..6
    ];
    // We anchor 3 real lines; build the mint over a blob where those lines sit at 42..=44 by padding.
    let mut padded: Vec<&str> = (0..41).map(|_| "// pad").collect();
    padded.extend_from_slice(&original[3..6]); // the answer() body at lines 42..=44
    let minted = MintedLineRange::mint("oid-original", &padded, 42, 44);

    // Force-push: the blob is rewritten (new oid), the answer() body MOVED down (3 lines prepended).
    let mut rewritten: Vec<&str> = vec!["// new banner", "// added", "// added 2"];
    rewritten.extend_from_slice(&padded);
    owner.record_line_range(&ref_, minted, "oid-forcepushed", &rewritten);

    let outcome = resolve_sub_outcome(&owner, &ref_);
    match outcome {
        ProjectOutcome::Live(p) => {
            assert_eq!(
                p.flag,
                Some(crate::resolve::ProjectionFlag::Moved),
                "a rebased range is MOVED"
            );
            // The anchor reflects the SHIFTED position (45..=47 — moved down by the 3 prepended lines),
            // never the stale raw L42-L44.
            assert_eq!(p.sub_anchor.as_deref().unwrap(), "myelin://acme-eu/git/blob/repo7:main:src%2Flib.rs#L45-L47");
        }
        other => panic!("expected MOVED Live, got {other:?}"),
    }
}

/// **A partial line-range edit resolves OUTDATED, the surviving sub-range carried (REF-D9).** Some
/// anchored lines survive, some are gone → OUTDATED (a partial `Live` flagged `outdated`).
#[test]
fn partially_edited_line_range_resolves_outdated() {
    let owner = GitOwner::new();
    let ref_ = myelin_git::subs::mint_blob_line_range("acme-eu", "repo7", "main", "f.rs", 1, 3)
        .expect("grammatical");
    let original = vec!["line A", "line B", "line C"];
    let minted = MintedLineRange::mint("oid-1", &original, 1, 3);
    // Edit: line A + B survive contiguously at the top; line C was rewritten.
    let edited = vec!["line A", "line B", "line C REWRITTEN", "tail"];
    owner.record_line_range(&ref_, minted, "oid-2", &edited);

    let outcome = resolve_sub_outcome(&owner, &ref_);
    match outcome {
        ProjectOutcome::Live(p) => {
            assert_eq!(p.flag, Some(crate::resolve::ProjectionFlag::Outdated));
            assert_eq!(p.sub_anchor.as_deref().unwrap(), "myelin://acme-eu/git/blob/repo7:main:f.rs#L1-L2");
        }
        other => panic!("expected OUTDATED Live, got {other:?}"),
    }
}

/// **A line-range whose content is entirely GONE resolves to a sub-gone tombstone that carries the
/// root (REF-D9 — 0 dangling embed, 0 hard 404).** None of the anchored lines survive → GONE; the
/// chokepoint tombstones it carrying the `#sub`-stripped BLOB root (the embed shows the parent file).
#[test]
fn content_gone_line_range_tombstones_carrying_the_root() {
    let owner = GitOwner::new();
    let ref_ = myelin_git::subs::mint_blob_line_range("acme-eu", "repo7", "main", "g.rs", 1, 2)
        .expect("grammatical");
    let original = vec!["secret line 1", "secret line 2"];
    let minted = MintedLineRange::mint("oid-1", &original, 1, 2);
    // The whole function was deleted — none of the anchored content survives.
    let rewritten = vec!["// file is now empty-ish", "// nothing here"];
    owner.record_line_range(&ref_, minted, "oid-2", &rewritten);

    // Through the full chokepoint (steps 1–2 + the §4.6 mapping) the GONE sub maps to a SubGone
    // tombstone carrying the root.
    let svc = git_resolve_service(&owner);
    let v = viewer("insider", &tenant());
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
    assert!(res.is_tombstone(), "content_gone → tombstone");
    assert_eq!(res.tombstone_reason(), Some(TombstoneReason::SubGone));
    // The tombstone carries the BLOB root (never a hard 404) — the embed shows the parent file.
    if let crate::resolve::Resolution::Tombstone(t) = res {
        assert_eq!(t.root, strip_sub(&ref_));
        assert_eq!(t.root.0, "myelin://acme-eu/git/blob/repo7:main:g.rs");
    }
}

/// **An EXACT line range (the blob oid still matches) resolves LIVE, no flag (REF-D9 happy path).**
#[test]
fn exact_line_range_resolves_live() {
    let owner = GitOwner::new();
    let ref_ = myelin_git::subs::mint_blob_line_range("acme-eu", "repo7", "main", "h.rs", 1, 1)
        .expect("grammatical");
    let lines = vec!["fn main() {}"];
    let minted = MintedLineRange::mint("oid-X", &lines, 1, 1);
    owner.record_line_range(&ref_, minted, "oid-X", &lines); // SAME oid → exact
    match resolve_sub_outcome(&owner, &ref_) {
        ProjectOutcome::Live(p) => assert_eq!(p.flag, None, "an exact range is a clean LIVE"),
        other => panic!("expected LIVE, got {other:?}"),
    }
}

/// **A Git PR review-comment / thread anchor degrades through the SAME ladder (REF-D9, comment-/thread-).**
/// A moved-by-edit comment is MOVED (the id is immutable, the stability obligation is git's); a
/// resolved thread is OUTDATED; a deleted comment is a root-carrying sub-gone tombstone.
#[test]
fn git_comment_and_thread_anchors_degrade_through_the_ladder() {
    let owner = GitOwner::new();
    let comment = myelin_git::subs::mint_pr_comment("acme-eu", "repo7", 4291, "cAbc")
        .expect("grammatical");
    let thread = myelin_git::subs::mint_pr_thread("acme-eu", "repo7", 4291, "tXyz")
        .expect("grammatical");
    let deleted = myelin_git::subs::mint_pr_comment("acme-eu", "repo7", 4291, "cDel")
        .expect("grammatical");

    owner.record_comment(&comment, CommentState::Moved);
    owner.record_comment(&thread, CommentState::Resolved);
    owner.record_comment(&deleted, CommentState::Gone);

    // A moved-by-edit comment → MOVED (a Live projection flagged `moved`); the id is immutable.
    match resolve_sub_outcome(&owner, &comment) {
        ProjectOutcome::Live(p) => assert_eq!(p.flag, Some(crate::resolve::ProjectionFlag::Moved)),
        other => panic!("comment moved → MOVED Live, got {other:?}"),
    }
    match resolve_sub_outcome(&owner, &thread) {
        ProjectOutcome::Live(p) => assert_eq!(p.flag, Some(crate::resolve::ProjectionFlag::Outdated)),
        other => panic!("thread resolved → OUTDATED, got {other:?}"),
    }
    // A deleted comment → sub-gone (the root PR is carried by the chokepoint).
    assert_eq!(resolve_sub_outcome(&owner, &deleted), ProjectOutcome::SubGone);
}

/// **The CI `check-`/`step-` kinds are USED (not built) — they resolve through the SAME ladder (C-6).**
/// Git's `check_status` projection (`check-<context>`) + the `details_ref` jump-to-failure
/// (`step-<n>`) degrade through the one ladder; CI's PRODUCER is REF-P19 (the floor). Here the
/// SUB-ANCHOR resolution (Refs' half) is proven.
#[test]
fn ci_check_and_step_sub_anchors_resolve_through_the_same_ladder() {
    use myelin_refs::{mint, Sub};
    let owner = GitOwner::new();
    let commit_root = ArtifactRef("myelin://acme-eu/git/commit/repo7:abc".into());
    let check = mint(&commit_root, Sub::Check("ci/build".into())).expect("check- mint");
    let step = mint(&commit_root, Sub::Step(7)).expect("step- mint");

    owner.record_check(&check, CommentState::Live); // a live check status
    owner.record_check(&step, CommentState::Gone); // a pruned run → sub-gone

    assert!(matches!(resolve_sub_outcome(&owner, &check), ProjectOutcome::Live(_)));
    assert_eq!(resolve_sub_outcome(&owner, &step), ProjectOutcome::SubGone);
    // Both are first-class #sub kinds in the one grammar (C-6).
    assert!(sub_kind(&check).is_some());
    assert!(sub_kind(&step).is_some());
}

// ===========================================================================
// REF-D1 / REF-D2 — leak + IDOR re-confirmed on the REAL Git edge corpus
// ===========================================================================

/// **REF-D1 (leak) re-confirmed on real Git edges: a DENIED viewer is tombstoned, never leaked.** A
/// confidential Git PR's content does NOT leak through an unfurl to a viewer with no `repo->pull`.
#[test]
fn ref_d1_denied_viewer_of_a_git_pr_is_tombstoned_never_leaked() {
    let owner = GitOwner::new();
    let pr = GitEdgeProducer::pr_root("acme-eu", "secret-repo", 1);
    let outsider = viewer("outsider", &tenant());
    // NO grant_view for the outsider (default-deny) — the GIT-D11 leak invariant.
    let svc = git_resolve_service(&owner);
    let res = svc.resolve(
        &tenant(),
        &region(),
        &pr,
        &pr,
        &outsider,
        ResolveMode::Live,
        &bounded_stale(),
        false,
    );
    assert!(res.is_tombstone(), "a denied viewer is tombstoned");
    assert_eq!(res.tombstone_reason(), Some(TombstoneReason::Denied));
    // The tombstone is structurally incapable of carrying content — the leak cannot regress.
    if let crate::resolve::Resolution::Tombstone(t) = res {
        assert_eq!(t.root, pr);
    }
}

/// **REF-D2 (IDOR / cross-tenant) on the real Git backlink read: a tenant-B viewer cannot read a
/// tenant-A Git PR's backlinks.** The backlink read is tenant-first (`WHERE tenant = :viewer.tenant`)
/// — a foreign-tenant edge is not even a candidate. (Reuses the REF-P11 BacklinkRead — the engine is
/// unchanged; this proves it on Git-shaped edges.)
#[test]
fn ref_d2_cross_tenant_git_backlink_read_returns_nothing() {
    use myelin_identity::ListObjectsResult;

    let tenant_a = TenantId("tenantA".into());
    let edges = EdgeProjection::new();
    // A tenant-A Git edge: a PR links an issue.
    let pr = "myelin://tenantA/git/pr/repo:9";
    let issue = "myelin://tenantA/issue/issue/ENG-1";
    let id = edge_id(&tenant_a, pr, issue, "links");
    edges.upsert(
        &tenant_a,
        &region(),
        EdgeRow {
            edge_id: id.clone(),
            source: ArtifactRef(pr.into()),
            source_root: strip_sub(&ArtifactRef(pr.into())),
            target: ArtifactRef(issue.into()),
            target_root: strip_sub(&ArtifactRef(issue.into())),
            rel: "links".into(),
            rel_class: RelClass::Reference,
            origin_event: format!("evt-{id}"),
            origin_actor: "git-pseudonym-1".into(),
            zookie: Some("zk-1".into()),
            tombstoned: false,
        },
    );

    let read = BacklinkRead::new(edges, AuthzVisibleIndex::new());
    // A tenant-B viewer queries the tenant-A issue's backlinks (the IDOR attempt). The read is keyed
    // to the VIEWER's tenant (tenantB) — the tenant-A edge is in a different partition, 0 candidates.
    let tenant_b = TenantId("tenantB".into());
    let viewer_b = viewer("attacker", &tenant_b);
    let page = read
        .backlinks(
            &tenant_b,
            &region(),
            &ArtifactRef(issue.into()),
            &viewer_b,
            &ListObjectsResult::Filter {
                set_expr: myelin_identity::SetExpr::All,
                zookie: myelin_identity::Zookie("zk-1".into()),
            },
            &bounded_stale(),
            10,
        )
        .expect("backlink read");
    assert_eq!(page.edges.len(), 0, "no cross-tenant Git backlink is visible (REF-D2)");
}

/// **The Git ReBAC fragment flows through `list_objects` (4.9 / GIT-D11): a viewer outside the
/// `repo->pull` allow-set sees 0 PR backlinks; an inside viewer sees them.** The backlink read lowers
/// the FROZEN SetExpr over `edge.source_root` (REF-P11), so the Git fragment's `repo->pull` (modelled
/// as the `Ids` allow-set) is the leak-free filter.
#[test]
fn git_fragment_flows_through_list_objects_leak_free() {
    use myelin_identity::ListObjectsResult;

    let t = tenant();
    let edges = EdgeProjection::new();
    // Two Git PRs link a shared issue. PR-public is readable; PR-secret is not.
    let issue = "myelin://acme-eu/issue/issue/ENG-1";
    for pr in ["myelin://acme-eu/git/pr/repo:public", "myelin://acme-eu/git/pr/repo:secret"] {
        let id = edge_id(&t, pr, issue, "links");
        edges.upsert(
            &t,
            &region(),
            EdgeRow {
                edge_id: id.clone(),
                source: ArtifactRef(pr.into()),
                source_root: strip_sub(&ArtifactRef(pr.into())),
                target: ArtifactRef(issue.into()),
                target_root: strip_sub(&ArtifactRef(issue.into())),
                rel: "links".into(),
                rel_class: RelClass::Reference,
                origin_event: format!("evt-{id}"),
                origin_actor: "git-pseudonym".into(),
                zookie: Some("zk-1".into()),
                tombstoned: false,
            },
        );
    }
    let read = BacklinkRead::new(edges, AuthzVisibleIndex::new());
    let v = viewer("dev", &t);
    // The Git fragment's `repo->pull` allow-set (list_objects) admits ONLY the public PR.
    let allowed = ListObjectsResult::Ids {
        ids: vec![myelin_identity::ObjectId("myelin://acme-eu/git/pr/repo:public".into())],
        zookie: myelin_identity::Zookie("zk-1".into()),
    };
    let page = read
        .backlinks(&t, &region(), &ArtifactRef(issue.into()), &v, &allowed, &bounded_stale(), 10)
        .expect("backlink read");
    assert_eq!(page.edges.len(), 1, "only the repo->pull-admitted PR is visible");
    assert_eq!(page.edges[0].source.0, "myelin://acme-eu/git/pr/repo:public");
}

// ===========================================================================
// 2.6 — the Git per-blob/ref replay grain (sub-artifact-granular)
// ===========================================================================

/// The Git replay scope is sub-artifact-granular (contract 2.6): a blob scope re-derives the line-range
/// anchor at BLOB grain; a PR scope the comment/thread anchors; a repo scope the whole repo. The
/// selector matches the grain Git's `replay` parses ([`myelin_git::replay::GitReplayKind`]).
#[test]
fn git_replay_scope_is_sub_artifact_granular() {
    assert_eq!(git_replay_scope(GitReplayGrain::Repo("core".into())), "repo:core");
    assert_eq!(
        git_replay_scope(GitReplayGrain::Blob { repo: "core".into(), oid: "abc".into() }),
        "blob:core/abc"
    );
    assert_eq!(
        git_replay_scope(GitReplayGrain::Pr { repo: "core".into(), number: 42 }),
        "pr:core/42"
    );
    // The grain is the one Git's producer owns (the token, never a literal).
    assert_eq!(GIT_OWNER_TOKEN, "git");
}

/// **REF-D4 (reindex-parity, DB-free CDC half) over a Git edge corpus.** A Git edge corpus is built
/// LIVE, the partition is WIPED, then rebuilt ONLY by re-driving the SAME upserts (the deterministic
/// edge_id + strip_sub roots — the production logic, no Git-DB backdoor) → byte-parity (cold == live).
/// The live-Postgres proof is the integration test; this is the in-memory model proof.
#[test]
fn ref_d4_git_edge_reindex_byte_parity() {
    let t = tenant();
    // A Git edge corpus: a PR links an issue, a commit links an issue, a line-range embeds a blob.
    let corpus = [
        ("myelin://acme-eu/git/pr/repo:9", "myelin://acme-eu/issue/issue/ENG-1", "links"),
        ("myelin://acme-eu/git/commit/repo:abc", "myelin://acme-eu/issue/issue/ENG-2", "links"),
        (
            "myelin://acme-eu/git/blob/repo:main:src%2Flib.rs#L1-L9",
            "myelin://acme-eu/git/blob/repo:main:src%2Flib.rs",
            "embeds",
        ),
    ];
    let build = || {
        let edges = EdgeProjection::new();
        for (source, target, rel) in corpus {
            let id = edge_id(&t, source, target, rel);
            edges.upsert(
                &t,
                &region(),
                EdgeRow {
                    edge_id: id.clone(),
                    source: ArtifactRef(source.into()),
                    source_root: strip_sub(&ArtifactRef(source.into())),
                    target: ArtifactRef(target.into()),
                    target_root: strip_sub(&ArtifactRef(target.into())),
                    rel: rel.into(),
                    rel_class: RelClass::Reference,
                    origin_event: format!("evt-{id}"),
                    origin_actor: "git-pseudonym".into(),
                    zookie: Some("zk-1".into()),
                    tombstoned: false,
                },
            );
        }
        edges
    };
    // LIVE build → its byte image.
    let live = build();
    let live_hash = live.parity_hash(&t, &region());
    // WIPE + rebuild from the SAME upserts (the reindex re-emit path) → byte-identical.
    live.wipe_partition(&t, &region());
    assert_eq!(live.live_count(&t, &region()), 0, "partition wiped");
    let cold = build();
    assert_eq!(
        cold.parity_hash(&t, &region()),
        live_hash,
        "cold == live (byte-identical Git-edge reindex parity)"
    );
}

// ----- a small resolve-service harness over the Git owner -----

/// Build a [`ResolveService`] over the Git owner (the engine is unchanged — the Git owner is the only
/// new wiring). [`GitOwner`] is `Clone` (Arc-shared interior), so a clone the service holds shares the
/// SAME recorded state the test arms. Uses the no-op cache read (REF-P12's live cache is orthogonal).
fn git_resolve_service(owner: &GitOwner) -> ResolveService {
    ResolveService::new(
        authz(),
        Arc::new(crate::resolve::NoOpCacheRead),
        Arc::new(owner.clone()),
        cell(),
    )
}
