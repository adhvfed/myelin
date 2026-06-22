//! # NOTIF-D4-class on a REAL Git subject, through the REAL `GitRefResolver` → humanise chain
//! (GIT-P31 / P-292, M3-G8)
//!
//! **Drill source:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`
//! - row **NOTIF-D4-class** ("notify on a confidential subject to a viewer lacking access → humanised
//!   tombstone; the title NEVER appears; 0 confidential titles in delivered notifications"). The
//!   prompt's GATE: *"a confidential PR/commit subject → humanised TOMBSTONE; the title NEVER leaks in
//!   the notification (0 confidential titles in delivered notifications)."* Threshold 0, never softened.
//!
//! **What this drill PROVES that the GIT-P19 drill did not (the floor it fills).** GIT-P19's NOTIF-D4
//! re-confirmation (`drill_notif_d4_git_d8_real_git_subject.rs`) drove humanise through a
//! `GitRepoResolver` **defined inside the test file** — explicitly NAMED there as a floor ("the
//! production resolve transport is the Refs chokepoint … the [`GitRepoResolver`] here stands in"). THIS
//! drill drives the SAME NOTIF-D4-class leak gate through the REAL crate seam
//! [`myelin_git::git_resolve::GitRefResolver`] (P-292), which delegates to Git's REAL
//! [`Projector::project`](myelin_git::project::Projector) (contract 5.6 — permission FIRST). So the leak
//! property is now exercised over Git's REAL per-viewer permission logic — not a test approximation.
//!
//! **ZERO Notif code change.** This consumes Notif's frozen [`humanise`](myelin_notif::humanise) +
//! [`RefResolvePort`](myelin_notif::RefResolvePort) seams; the resolver is an impl handed in. The leak
//! invariant is structural: a denied ref maps to a [`Tombstone`](myelin_notif::Tombstone) with no
//! `title` field.

use myelin_git::git_resolve::GitRefResolver;
use myelin_git::body::Body;
use myelin_git::check_status::GateOutcome;
use myelin_git::lifecycle::PullRequest;
use myelin_git::project::{git_pr_ref, ArtifactStore, Projector};
use myelin_identity::{
    AuthzError, CaveatContext, Consistency, ConsistencyMode, Credential, Decision, IdentityService,
    ListObjectsResult, ObjectId, ObjectType, Permission, Principal, PrincipalId, PrincipalKind,
    Result as IdResult, RewriteTrace, SubjectTree, TupleDelta, Zookie,
};
use myelin_notif::humanise::{humanise, Channel, TemplateStore, DEFAULT_LOCALE};
use myelin_notif::{reason_template_key, Reason};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};
use std::collections::HashSet;

/// The secret PR title of the REAL private Git repo — must NEVER appear for a denied viewer.
const SECRET_PR_TITLE: &str = "fix: rotate the PROJECT-NIGHTFALL signing key before the acquisition";

fn acme() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn viewer_in(id: &str, tenant: TenantId) -> Principal {
    Principal::new(
        tenant,
        region(),
        PrincipalId(id.into()),
        PrincipalKind::Human,
        myelin_identity::DataRole::Controller,
        myelin_identity::PrincipalStatus::Active,
    )
}
fn strong(zk: &str) -> Consistency {
    Consistency { at_least: Zookie(zk.into()), mode: ConsistencyMode::Strong }
}

/// A REAL private PR in acme's repo. Only principals holding `view` (→ `repo->pull`) may see the title.
fn private_pr() -> (ArtifactRef, PullRequest) {
    let pr_ref = git_pr_ref("acme", "repo7", 9);
    let mut pr = PullRequest::open(9, "refs/heads/main", "refs/heads/feature", "psn:alice", false);
    pr.body = Body::new(SECRET_PR_TITLE, vec![]);
    (pr_ref, pr)
}

/// A deterministic Id: a `(viewer.tenant, view@object)` allow-list. GIT-D8: the decision keys on the
/// TOKEN tenant (viewer.tenant) — a cross-tenant viewer is denied even if a same-id grant exists in
/// another tenant.
struct StubId {
    allow: HashSet<(String, String)>,
}
impl StubId {
    fn new() -> Self {
        Self { allow: HashSet::new() }
    }
    fn grant(mut self, tenant: &TenantId, object: &ArtifactRef) -> Self {
        self.allow.insert((tenant.0.clone(), format!("view@{}", object.0)));
        self
    }
}
impl IdentityService for StubId {
    fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn check(
        &self,
        s: &Principal,
        permission: &Permission,
        object: &ArtifactRef,
        _at: &Consistency,
        _caveat: Option<&CaveatContext>,
    ) -> IdResult<Decision> {
        let key = (s.tenant.0.clone(), format!("{}@{}", permission.0, object.0));
        Ok(if self.allow.contains(&key) { Decision::Allow } else { Decision::Deny })
    }
    fn list_objects(
        &self,
        _s: &Principal,
        _p: &Permission,
        _t: &ObjectType,
        _at: &Consistency,
    ) -> IdResult<ListObjectsResult> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn list_subjects(&self, _o: &ObjectId, _p: &Permission, _at: &Consistency) -> IdResult<SubjectTree> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn explain(
        &self,
        _s: &Principal,
        _p: &Permission,
        _o: &ObjectId,
        _at: &Consistency,
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

fn resolver(id: StubId) -> GitRefResolver<StubId> {
    let (pr_ref, pr) = private_pr();
    let mut store = ArtifactStore::new();
    store.put_pr(&pr_ref, pr, GateOutcome::AllRequiredGreen, 0, 0);
    GitRefResolver::new(Projector::new(id, store))
}

const GIT_REASONS: &[Reason] = &[Reason::ReviewRequested, Reason::Mentioned, Reason::Watched];

fn contains_leak(text: &str) -> bool {
    let lc = text.to_lowercase();
    text.contains(SECRET_PR_TITLE)
        || lc.contains("nightfall")
        || lc.contains("acquisition")
        || lc.contains("signing key")
}

/// **NOTIF-D4-class (the dated green artifact): 0 title leak through the REAL `GitRefResolver` →
/// humanise chain.** A confidential PR review-requested to viewers WITHOUT `pull` → across every viewer
/// × every channel × every Git reason, the secret title appears EXACTLY ZERO times. Threshold 0.
#[test]
fn notif_d4_zero_leak_through_real_git_resolver() {
    let resolver = resolver(StubId::new()); // nobody granted → every viewer denied
    let templates = TemplateStore::with_platform_defaults();
    let (subject, _) = private_pr();
    let denied = ["ex-contractor", "wrong-team-dev", "intern-no-access"];

    let mut renders = 0u64;
    let mut leak_count = 0u64;
    let mut tombstone_present = 0u64;

    for v in denied {
        for &reason in GIT_REASONS {
            let key = reason_template_key(reason);
            for channel in [Channel::Cli, Channel::Email, Channel::Markdown] {
                let h = humanise(
                    &resolver,
                    &acme(),
                    &region(),
                    &templates,
                    key,
                    std::slice::from_ref(&subject),
                    &viewer_in(v, acme()),
                    DEFAULT_LOCALE,
                    &strong("zk-1"),
                    channel,
                );
                renders += 1;
                if contains_leak(&h.text) {
                    leak_count += 1;
                }
                if h.text.contains("a restricted pr") {
                    tombstone_present += 1;
                }
                assert!(h.links.is_empty(), "a denied Git subject yields no click-route link");
            }
        }
    }

    assert_eq!(
        leak_count, 0,
        "NOTIF-D4-class (real GitRefResolver): title-leak-count MUST be 0 over {renders} renders; never weakened"
    );
    assert_eq!(
        tombstone_present, renders,
        "every denied render shows the PII-free `a restricted pr` tombstone (the embed degrades)"
    );
    eprintln!(
        "NOTIF-D4-class GREEN through the REAL GitRefResolver (2026-06-22): {renders} denied renders, \
         title-leak-count = {leak_count} (threshold 0), tombstone = {tombstone_present}/{renders}"
    );
}

/// **The complement — a viewer WITH `pull` sees the PR title through the real resolver (the gate
/// discriminates, not a blanket redaction).**
#[test]
fn notif_d4_permitted_viewer_sees_the_title() {
    let (subject, _) = private_pr();
    let resolver = resolver(StubId::new().grant(&acme(), &subject));
    let h = humanise(
        &resolver,
        &acme(),
        &region(),
        &TemplateStore::with_platform_defaults(),
        "review_requested",
        std::slice::from_ref(&subject),
        &viewer_in("maintainer", acme()),
        DEFAULT_LOCALE,
        &strong("zk-1"),
        Channel::Cli,
    );
    assert!(h.text.contains(SECRET_PR_TITLE), "the permitted maintainer sees the PR title");
    assert_eq!(h.links, vec![subject.0], "the allowed branch yields the click-route link");
}

/// **GIT-D8 — cross-tenant unfurl denied through the real resolver (0 cross-tenant leak).** A viewer
/// whose TOKEN tenant (`evilcorp`) differs from the subject's home tenant (`acme`) is denied — even if a
/// same-id principal in acme would be allowed. The humanise render degrades to a tombstone; the
/// private-repo title never crosses the tenant boundary. Threshold 0.
#[test]
fn git_d8_cross_tenant_unfurl_denied_through_real_resolver() {
    let (subject, _) = private_pr(); // home tenant: acme
    // an acme "spy" WOULD be allowed — but the cross-tenant token below is a DIFFERENT tenant.
    let resolver = resolver(StubId::new().grant(&acme(), &subject));
    let cross_tenant = viewer_in("spy", TenantId("evilcorp".into()));

    let mut leak = 0u64;
    for &reason in GIT_REASONS {
        let key = reason_template_key(reason);
        for channel in [Channel::Cli, Channel::Email, Channel::Markdown] {
            let h = humanise(
                &resolver,
                &acme(),
                &region(),
                &TemplateStore::with_platform_defaults(),
                key,
                std::slice::from_ref(&subject),
                &cross_tenant,
                DEFAULT_LOCALE,
                &strong("zk-1"),
                channel,
            );
            if contains_leak(&h.text) {
                leak += 1;
            }
            assert!(h.text.contains("a restricted pr"), "cross-tenant render is a tombstone");
            assert!(h.links.is_empty(), "no click-route leaks across the tenant boundary");
        }
    }
    assert_eq!(leak, 0, "GIT-D8: 0 cross-tenant leak — the token tenant decides, the title never crosses");
    eprintln!("GIT-D8 GREEN through the real resolver (2026-06-22): cross-tenant-leak-count = {leak} (threshold 0)");
}
