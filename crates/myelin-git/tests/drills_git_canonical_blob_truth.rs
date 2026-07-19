//! **Git's canonical blob truth for a reindex — the replay source that omits deleted and restricted
//! blobs.**
//!
//! A rebuild replays owner truth, so Git has to be able to state what that truth IS. The emitter's
//! push path is incremental (it reports what CHANGED), which is the wrong shape for a cold rebuild:
//! a rebuild needs the full current set. Before this, Git's `ReindexSource` blob rows were populated
//! only by tests, so a production rebuild would have wiped the blob corpus and replayed nothing.
//!
//! What these drills pin:
//!
//! 1. enumeration yields the CANONICAL identity for every live blob on an indexed ref;
//! 2. a DELETED blob is absent — enumeration states what exists, so a rebuild cannot resurrect it;
//! 3. a RESTRICTED blob is absent, not body-suppressed — a suppressed row would still be queryable
//!    by its `path`/`blob_oid` facets;
//! 4. reloading REPLACES a ref's blob truth, so a blob deleted or restricted since the last load
//!    does not survive as a stale row and get replayed back in;
//! 5. a non-indexed ref has no truth to replay.

use std::sync::Arc;

use myelin_events::{
    Actor, EmitContextBase, MonotonicMinter, OutboxStore, Region, ReindexSource, SnapshotScope,
    TenantId, Timestamp,
};
use myelin_git::code_projection::{
    Blob, CodeProjectionCursor, CodeProjectionEmitter, NoRestrictions, RestrictionPolicy, Tree,
};
use myelin_git::replay::GitReindexSource;
use myelin_identity::{Principal, PrincipalId, PrincipalKind};

const REPO: &str = "core";
const MAIN: &str = "refs/heads/main";

fn ctx() -> EmitContextBase {
    let tenant = TenantId("acme".into());
    EmitContextBase {
        tenant: tenant.clone(),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("platform".into()),
            PrincipalKind::Service,
            tenant,
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-07-19T00:00:00Z".into()),
        recorded_at: Timestamp("2026-07-19T00:00:00Z".into()),
        caused_by: None,
    }
}

/// A restriction policy suppressing one exact path.
struct RestrictPath(&'static str);
impl RestrictionPolicy for RestrictPath {
    fn is_restricted(&self, _repo: &str, path: &str) -> bool {
        path == self.0
    }
}

fn tree() -> Tree {
    Tree::empty()
        .with("src/charge.rs", Blob::new("o-charge", b"fn charge() {}".to_vec()))
        .with("src/refund.rs", Blob::new("o-refund", b"fn refund() {}".to_vec()))
        .with(
            "src/secret.rs",
            Blob::new("o-secret", b"const KEY = \"top-secret\";".to_vec()),
        )
}

fn emitter<'a, R: RestrictionPolicy>(
    outbox: &'a OutboxStore,
    cursor: &'a CodeProjectionCursor,
    restriction: &'a R,
) -> CodeProjectionEmitter<'a, R> {
    CodeProjectionEmitter::new(
        REPO,
        "main",
        ctx(),
        outbox,
        Arc::new(MonotonicMinter::new()),
        cursor,
        restriction,
    )
}

/// **Enumeration yields the CANONICAL identity of every live, unrestricted blob — and omits the
/// restricted one entirely.**
#[test]
fn enumeration_yields_canonical_identities_and_omits_restricted() {
    let outbox = OutboxStore::new();
    let cursor = CodeProjectionCursor::new();
    let restriction = RestrictPath("src/secret.rs");
    let e = emitter(&outbox, &cursor, &restriction);

    let truth = e.enumerate_canonical_truth(MAIN, &tree()).unwrap();

    let refs: Vec<&str> = truth.iter().map(|t| t.artifact_ref.0.as_str()).collect();
    assert_eq!(
        refs,
        vec![
            "myelin://acme/git/blob/core:refs%2Fheads%2Fmain:src%2Fcharge%2Ers",
            "myelin://acme/git/blob/core:refs%2Fheads%2Fmain:src%2Frefund%2Ers",
        ],
        "canonical percent-encoded identities, restricted path omitted entirely"
    );
    // Every identity is canonical — a rebuild from these rows cannot reintroduce a legacy id.
    for t in &truth {
        assert!(
            myelin_search::canonical::is_canonical_blob_id(&t.artifact_ref.0),
            "not canonical: {}",
            t.artifact_ref.0
        );
        assert!(!myelin_search::canonical::is_legacy_blob_id(&t.artifact_ref.0));
    }
    // The restricted blob leaks neither its path nor its body into the enumeration.
    let rendered = format!("{truth:?}");
    assert!(
        !rendered.contains("secret") && !rendered.contains("top-secret"),
        "the restricted blob must not appear at all: {rendered}"
    );
}

/// **A DELETED blob is absent from the truth — enumeration states what exists.**
#[test]
fn a_deleted_blob_is_absent_from_the_truth() {
    let outbox = OutboxStore::new();
    let cursor = CodeProjectionCursor::new();
    let r = NoRestrictions;
    let e = emitter(&outbox, &cursor, &r);

    let after_delete = Tree::empty().with(
        "src/charge.rs",
        Blob::new("o-charge", b"fn charge() {}".to_vec()),
    );
    let truth = e.enumerate_canonical_truth(MAIN, &after_delete).unwrap();
    assert_eq!(truth.len(), 1);
    assert!(
        !truth.iter().any(|t| t.path.contains("refund")),
        "a blob deleted from the ref has no truth to replay"
    );
}

/// **A non-indexed ref has no truth to replay.**
#[test]
fn a_non_indexed_ref_has_no_truth() {
    let outbox = OutboxStore::new();
    let cursor = CodeProjectionCursor::new();
    let r = NoRestrictions;
    let e = emitter(&outbox, &cursor, &r);
    assert!(e
        .enumerate_canonical_truth("refs/heads/scratch", &tree())
        .unwrap()
        .is_empty());
}

/// **Reloading REPLACES a ref's blob truth, so a since-deleted or since-restricted blob does not
/// survive as a stale row and get replayed back into the index.**
///
/// This is the resurrection path a merge-instead-of-replace would open, and it is the reason the
/// reload drops the ref's rows before inserting.
#[test]
fn reloading_replaces_the_refs_blob_truth_rather_than_merging() {
    let outbox = OutboxStore::new();
    let cursor = CodeProjectionCursor::new();
    let mut source = GitReindexSource::new();

    // Load 1: three blobs, nothing restricted.
    {
        let r = NoRestrictions;
        let e = emitter(&outbox, &cursor, &r);
        let truth = e.enumerate_canonical_truth(MAIN, &tree()).unwrap();
        assert_eq!(truth.len(), 3);
        source.load_canonical_blob_truth(REPO, MAIN, &truth, 1);
    }
    let scope = SnapshotScope::new("git", "blob:all");
    assert_eq!(source.replay(&scope, None).len(), 3, "three blobs replay");

    // Load 2: one blob deleted from the ref, one newly restricted.
    {
        let restriction = RestrictPath("src/secret.rs");
        let e = emitter(&outbox, &cursor, &restriction);
        let after = Tree::empty().with(
            "src/charge.rs",
            Blob::new("o-charge", b"fn charge() {}".to_vec()),
        );
        let truth = e.enumerate_canonical_truth(MAIN, &after).unwrap();
        assert_eq!(truth.len(), 1, "only the surviving unrestricted blob");
        source.load_canonical_blob_truth(REPO, MAIN, &truth, 2);
    }

    let drafts = source.replay(&scope, None);
    assert_eq!(
        drafts.len(),
        1,
        "the deleted and restricted blobs do NOT survive the reload: {:?}",
        drafts.iter().map(|d| &d.subject.0).collect::<Vec<_>>()
    );
    assert!(drafts[0].subject.0.contains("charge"));
    // Neither the deleted nor the restricted identity is replayed.
    for gone in ["refund", "secret"] {
        assert!(
            !drafts.iter().any(|d| d.subject.0.contains(gone)),
            "`{gone}` was replayed back into the index"
        );
    }

    // A reload of ONE (repo, ref) leaves another repo's blob truth alone.
    {
        let r = NoRestrictions;
        let other = CodeProjectionEmitter::new(
            "other-repo",
            "main",
            ctx(),
            &outbox,
            Arc::new(MonotonicMinter::new()),
            &cursor,
            &r,
        );
        let truth = other
            .enumerate_canonical_truth(MAIN, &Tree::empty().with("a.rs", Blob::new("o-a", b"x".to_vec())))
            .unwrap();
        source.load_canonical_blob_truth("other-repo", MAIN, &truth, 1);
    }
    let drafts = source.replay(&scope, None);
    assert_eq!(drafts.len(), 2, "both repos' blob truth coexists");
}
