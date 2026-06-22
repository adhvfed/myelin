//! # CDC — contract 5.1 (`ArtifactRef` id grammar, git's canonical keys) + 5.2/5.6
//! (`project(ref, viewer)` for git artifacts) — GIT-P18 / P-279
//!
//! Rows 5.1 / 5.2 / 5.6 are the seam between:
//! - the **PROVIDER** (git) that MINTS its stable canonical `ArtifactRef` keys (`pr/<repo>:<n>`,
//!   `commit/<repo>:<sha>`, contract 5.1 / REF-3) and IMPLEMENTS `project(ref, viewer)` — the ONLY way
//!   git's artifacts are read (per-viewer permission-checked; a denied viewer gets a tombstone, 5.2/5.6);
//! - the **CONSUMER** (Refs/Search/Notif) that reads a git artifact ONLY through `project(ref, viewer)`
//!   (no cross-DB) and gets a `Projection` (authorized) or a `Tombstone` (denied/erased) — never a raw
//!   title for a viewer it may not show it to.
//!
//! The PROVIDER's promise (asserted on the provider side): every minted key round-trips byte-identical
//! through the one Refs codec (0 ungrammatical keys; the `#n` display is render-time only, 0 stored
//! display keys); and `project` is permission-FIRST (deny ⇒ tombstone with NO artifact field read).
//!
//! The CONSUMER's promise (asserted on the consumer side): a downstream reader that holds only a
//! canonical `ArtifactRef` + a viewer `Principal` gets back exactly the §3 `{title, state, icon}`
//! projection IFF the viewer is authorized, and a content-free tombstone otherwise — so a 0-leak in
//! `project` is a 0-leak for EVERY consumer (the M3-G5/M5 leak-drill feed, GIT-D11 / SRCH-D1/D3).
//!
//! FLOORS named: the `blob`/`#L<a>-L<b>` content-anchored sub-projection is GIT-P24; the live OLTP
//! store is GIT-P20; cross-cell projection is single-home (the multi-cell floor). The general Refs
//! `resolve(ref, viewer, mode)` 4-step tombstone LADDER (over ALL subsystems) is the P-159 Refs
//! resolver; THIS pair ships the GIT-OWNED `project()` half rows 5.2/5.6 assign to git.

use myelin_git::body::Body;
use myelin_git::check_status::GateOutcome;
use myelin_git::lifecycle::PullRequest;
use myelin_git::project::{
    git_commit_ref, git_pr_ref, ArtifactStore, CommitMeta, Projected, Projector,
};
use myelin_identity::{
    AuthzError, CaveatContext, Consistency, Credential, DataRole, Decision, IdentityService,
    ListObjectsResult, ObjectId, ObjectType, Permission, Principal, PrincipalId, PrincipalKind,
    PrincipalStatus, Result as IdResult, RewriteTrace, SubjectTree, TupleDelta, Zookie,
};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};
use std::collections::HashSet;

// ════════════════════════════════════════════════════════════════════════════════════════════════
// PROVIDER side (git): the canonical-key id grammar (5.1) + permission-first project() (5.2/5.6).
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **PROVIDER side of 5.1** — git mints its stable canonical keys; each round-trips byte-identical
/// through the ONE Refs codec, and the `#n` display is render-time only (0 stored display keys).
fn provider_canonical_keys() -> Vec<(ArtifactRef, &'static str)> {
    vec![
        (
            git_pr_ref("acme", "payments", 1421),
            "myelin://acme/git/pr/payments:1421",
        ),
        (
            git_commit_ref("acme", "payments", "blake3:deadbeefcafe"),
            "myelin://acme/git/commit/payments:blake3:deadbeefcafe",
        ),
    ]
}

#[test]
fn provider_mints_canonical_keys_that_round_trip_and_have_no_stored_display_key() {
    for (key, expect) in provider_canonical_keys() {
        // the canonical stored key round-trips byte-identical.
        assert_eq!(myelin_refs::format(&key), expect);
        assert_eq!(myelin_refs::parse(expect).unwrap(), key);
        // the render-time display (`#n`) is NEVER a stored scope (REF-3).
        if let Some(disp) = myelin_git::project::display_key(&key) {
            assert!(
                myelin_refs::parse(&disp).is_err(),
                "the display key `{disp}` must NOT re-parse to a scope (0 stored display keys)"
            );
        }
    }
}

// ── a deterministic Id over a `view@object` allow-list (the provider's permission source) ──
struct StubId {
    allow: HashSet<String>,
}
impl StubId {
    fn allowing(objects: &[&ArtifactRef]) -> Self {
        Self {
            allow: objects.iter().map(|o| format!("view@{}", o.0)).collect(),
        }
    }
    fn denying_all() -> Self {
        Self {
            allow: HashSet::new(),
        }
    }
}
impl IdentityService for StubId {
    fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn check(
        &self,
        _s: &Principal,
        permission: &Permission,
        object: &ArtifactRef,
        _at: &Consistency,
        _c: Option<&CaveatContext>,
    ) -> IdResult<Decision> {
        let key = format!("{}@{}", permission.0, object.0);
        Ok(if self.allow.contains(&key) {
            Decision::Allow
        } else {
            Decision::Deny
        })
    }
    fn list_objects(
        &self,
        _s: &Principal,
        _p: &Permission,
        _t: &ObjectType,
        _a: &Consistency,
    ) -> IdResult<ListObjectsResult> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn list_subjects(
        &self,
        _o: &ObjectId,
        _p: &Permission,
        _a: &Consistency,
    ) -> IdResult<SubjectTree> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn explain(
        &self,
        _s: &Principal,
        _p: &Permission,
        _o: &ObjectId,
        _a: &Consistency,
    ) -> IdResult<RewriteTrace> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn delegation(
        &self,
        _a: &Principal,
        _t: &Principal,
    ) -> IdResult<myelin_identity::EffectivePolicy> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn write_tuples(
        &self,
        _d: &[TupleDelta],
        _p: Option<&myelin_identity::Precondition>,
    ) -> IdResult<Zookie> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn mint_run_token(
        &self,
        _a: &PrincipalId,
        _r: &myelin_identity::RunId,
        _d: &myelin_identity::DelegationCaveats,
        _t: &myelin_identity::FailStaticBound,
    ) -> IdResult<myelin_identity::RunToken> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn revoke(&self, _t: &myelin_identity::RevokeTarget) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn resolve_pseudonym(&self, _s: &PrincipalId, _t: &TenantId) -> IdResult<String> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn erase(&self, _s: &PrincipalId) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn admit_fragment(
        &self,
        _f: &myelin_identity::NamespaceFragment,
    ) -> IdResult<myelin_identity::FragmentAdmit> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
}

fn viewer(id: &str) -> Principal {
    Principal::new(
        TenantId("acme".into()),
        Region("fr-par".into()),
        PrincipalId(id.into()),
        PrincipalKind::Human,
        DataRole::Controller,
        PrincipalStatus::Active,
    )
}

/// Build a projector seeded with one PR + one commit, with `alice` authorized to view both.
fn seeded_projector(authorized: bool) -> (Projector<StubId>, ArtifactRef, ArtifactRef) {
    let pr_ref = git_pr_ref("acme", "payments", 1421);
    let commit_ref = git_commit_ref("acme", "payments", "blake3:deadbeefcafe");
    let mut store = ArtifactStore::new();
    let mut pr = PullRequest::open(
        1421,
        "refs/heads/main",
        "refs/heads/feature",
        "psn:alice",
        false,
    );
    pr.body = Body::new("Harden the retry path", vec![]);
    store.put_pr(&pr_ref, pr, GateOutcome::AllRequiredGreen, 1, 1);
    store.put_commit(
        &commit_ref,
        CommitMeta {
            subject: "Fix the leak".into(),
            verified: true,
        },
    );
    let id = if authorized {
        StubId::allowing(&[&pr_ref, &commit_ref])
    } else {
        StubId::denying_all()
    };
    (Projector::new(id, store), pr_ref, commit_ref)
}

#[test]
fn provider_project_is_permission_first_deny_yields_a_tombstone_with_no_title() {
    // PROVIDER: a denied viewer's projection is a tombstone that never read the title (0 leak).
    let (projector, pr_ref, _commit) = seeded_projector(/*authorized*/ false);
    let got = projector
        .project(&pr_ref, &viewer("mallory"), Zookie("z".into()))
        .unwrap();
    assert!(got.is_tombstone());
    assert_eq!(
        got.title(),
        None,
        "the provider's deny path never reads the title (0 leak)"
    );
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// CONSUMER side (Refs/Search/Notif): read a git artifact ONLY through project(ref, viewer).
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// A downstream **CONSUMER** (modelled — Refs/Search/Notif) reads a git artifact through the ONLY
/// allowed path: `project(ref, viewer)`. It holds the canonical `ArtifactRef` + the viewer `Principal`
/// and consumes whatever `project` returns — never reaching into git's DB. The consumer's promise: it
/// renders the `{title, state, icon}` for an authorized viewer and a content-free "(not available)" for
/// a tombstone — it has no other way to learn the title (so a 0-leak in `project` is a 0-leak here).
fn consumer_render(projector: &Projector<StubId>, r: &ArtifactRef, v: &Principal) -> String {
    match projector.project(r, v, Zookie("z".into())) {
        Ok(Projected::Visible(p)) => format!("{}|{}|{}", p.icon, p.state, p.title),
        Ok(Projected::Tombstoned(t)) => t.display_text().to_string(),
        Err(e) => format!("ERR:{e}"),
    }
}

#[test]
fn consumer_reads_the_projection_for_an_authorized_viewer() {
    let (projector, pr_ref, commit_ref) = seeded_projector(/*authorized*/ true);
    // the consumer gets the §3 {icon, state, title} projection — the ONLY git-artifact read path.
    assert_eq!(
        consumer_render(&projector, &pr_ref, &viewer("alice")),
        "pr|open|Harden the retry path"
    );
    assert_eq!(
        consumer_render(&projector, &commit_ref, &viewer("alice")),
        "commit|verified|deadbee Fix the leak"
    );
}

#[test]
fn consumer_gets_a_content_free_tombstone_for_an_unauthorized_viewer() {
    let (projector, pr_ref, _commit) = seeded_projector(/*authorized*/ false);
    // the consumer holds the canonical ref + the viewer; the ONLY thing it can learn is "(not available)".
    let rendered = consumer_render(&projector, &pr_ref, &viewer("mallory"));
    assert_eq!(
        rendered, "(not available)",
        "0 leak — the consumer never sees the title"
    );
    assert!(
        !rendered.contains("Harden"),
        "the title must NOT appear anywhere in a consumer's tombstone render"
    );
}
