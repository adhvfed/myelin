//! # `git_resolve` — Git `resolve(ref, viewer, Display)` for unfurls, wired through Notif's humanise
//! (the ONE templating surface; GIT-P31 / P-292, M3-G8)
//!
//! **The notifications + humanise half of M3-G8 (architecture
//! `04-subsystem-architectures/git-hosting/architecture/00-overview.md` §2 (D) — the notification
//! routing; `04-views-cli-and-api.md` — the humanised `summary` `(template_key, args)`, never a raw
//! string).** GIT-P19 (P-263) already shipped the [`crate::notif_rules`] *registration* half — Git's
//! [`define_notif_rule`](myelin_notif::define_notif_rule) set (contract 7.6) + the
//! [`GitWatcherIndex`](crate::notif_rules::GitWatcherIndex) read-fanout. What lands HERE is the
//! **resolve (5.2) for unfurls** bridge that the NOTIF-D4-class leak gate runs over: a REAL Git
//! `RefResolvePort` that drives Notif's [`humanise`](myelin_notif::humanise) (contract 7.3) — a
//! confidential PR/commit subject resolves to a **humanised tombstone, the TITLE NEVER LEAKS**.
//!
//! ## Why this exists — promoting the GIT-P19 test stand-in to a real producer-crate seam (EI-01 §7)
//! GIT-P19's NOTIF-D4 re-confirmation (`tests/drill_notif_d4_git_d8_real_git_subject.rs`) drove
//! humanise through a `GitRepoResolver` **defined inside the test file**, explicitly NAMED there as a
//! floor: *"The production resolve transport is the Refs chokepoint over the resilient client (a named
//! floor); the [`GitRepoResolver`] here stands in …"*. P-292 fills that floor with a REAL crate module:
//! [`GitRefResolver`] implements Notif's frozen [`RefResolvePort`] (contract 5.2 — the resolve seam
//! humanise consumes) by delegating to Git's REAL [`Projector::project`](crate::project::Projector)
//! (contract 5.6). So the humanise leak property is now exercised over Git's REAL per-viewer
//! permission-first projection logic — NOT a test-local approximation. The drill's stand-in is replaced
//! by this module (`tests/drill_notif_d4_humanise_resolve.rs`).
//!
//! ## The inverse-signal property (EI-01 §1) — ZERO Notif code change
//! Git supplies the resolve transport humanise binds its slots to using ONLY the **public, frozen**
//! Notif seam — the [`RefResolvePort`] trait (the read half of contract 5.2/7.3). No Notif enum
//! variant, no Notif match arm, no Notif recompile: humanise already calls `resolve_display` on a
//! `&dyn RefResolvePort`; Git hands it ONE more impl. The leak invariant is **structural** — a denied
//! ref maps to a [`RefResolution::Tombstone`], a type with **no `title` field** for a title to leak
//! into (NOTIF-D4 — threshold 0, never softened). This mirrors the SAME accretion shape this crate uses
//! for the rule set ([`crate::notif_rules`]), Search ([`crate::search_projection`]), and Refs
//! ([`crate::subs`]).
//!
//! ## The resolve → humanise → tombstone chain (contract 5.2 → 7.3, the leak gate)
//! 1. Notif's [`humanise`](myelin_notif::humanise) resolves EACH `ArtifactRef` slot per-viewer via
//!    `RefResolvePort::resolve_display` BEFORE formatting (so a denied slot never carries a title into
//!    the formatter).
//! 2. [`GitRefResolver::resolve_display`] calls Git's REAL [`Projector::project`](crate::project::Projector)
//!    — **permission FIRST** (`Id.check(viewer, view, repo->pull)`); a `Deny` / Id-hiccup / erased /
//!    restricted artifact returns a [`Projected::Tombstoned`](crate::project::Projected), built with NO
//!    field of the artifact read into it.
//! 3. This module maps that [`Projected`](crate::project::Projected) into Notif's
//!    [`RefResolution`]: `Visible(p)` → `Projection{title, icon}` (the ALLOWED branch — the title + a
//!    click-route link); `Tombstoned(t)` → `Tombstone{root, reason}` (the leak-free branch — Git's
//!    [`TombstoneReason`](crate::project::TombstoneReason) maps to Notif's PII-free
//!    [`TombstoneReason`](myelin_notif::TombstoneReason) which renders `a restricted pr`, NEVER the
//!    title).
//!
//! The OPAQUE root URN crosses into the tombstone (so Notif can render `a restricted <kind>` from the
//! URN structure) — the title/state/render-hint never do (they live only on the `Visible` branch).
//!
//! ## Review-requests are a FILTER over the ONE inbox (7.1), never a second store
//! Git's "Review requests" scoped view (architecture §1.3) is a `filter` over the ONE
//! [`list_inbox`](myelin_notif::list_inbox) by `reason = ReviewRequested` + `subject` prefix `git/` —
//! NEVER a second notification store (contract 7.1, C-9). [`git_review_requests_filter`] returns that
//! filter; the inbox itself is Notif's ONE store. This is asserted in the CDC + the unit tests.
//!
//! ## NOTIF-D4-class GATE (the dated green artifact; threshold 0, never softened)
//! A confidential PR/commit subject (a private-repo artifact) → for a viewer LACKING `pull` the
//! humanise render is a TOMBSTONE; the title appears EXACTLY ZERO times across every channel × every
//! Git reason template. Proven in `tests/drill_notif_d4_humanise_resolve.rs` over THIS real resolver.
//!
//! ## <a name="named-floors"></a>Named floors (VISION §3)
//! - **The Web UI + CLI/API surface (the notif inbox render + the PR context-pane unfurl)** is
//!   **GIT-P32** (the M3 band-exit aggregate). This module ships the resolve→humanise CONTRACT wiring;
//!   the browser-driven render of it is GIT-P32.
//! - **The live OLTP artifact store** the [`Projector`](crate::project::Projector) reads is the GIT-P20
//!   store-wiring floor (the SAME entity shapes the live store hydrates — the resolver is
//!   store-agnostic).
//! - **The production resolve transport** is the Refs resolve chokepoint over the substrate resilient
//!   client; [`GitRefResolver`] is the IN-PROCESS resolve seam (cell-local — contract 5.2 / OQ-I: git
//!   resolution is always cell-local) over Git's real Projector. The cross-cell single-home resolve is
//!   the named multi-cell floor (a viewer in cell A unfurling a PR homed in cell B has cell B run the
//!   projection; only the rendered projection crosses).

use myelin_identity::{Consistency, IdentityService, Principal, Zookie};
use myelin_notif::humanise::{
    RefProjection, RefResolution, RefResolvePort, Tombstone, TombstoneReason,
};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};

use crate::project::{Projected, Projector, TombstoneReason as GitTombstoneReason};

/// **The `filter` token Git's "Review requests" scoped view applies over the ONE inbox (contract 7.1).**
/// Review-requests are NOT a second store: the scoped view is `list_inbox(principal, filter =
/// reason:review_requested + subject-prefix git/)`. Named here so the view + the inbox agree on the
/// filter by NAME (X-5), never a literal.
pub const GIT_REVIEW_REQUESTS_FILTER_REASON: myelin_notif::Reason =
    myelin_notif::Reason::ReviewRequested;

/// The `subject` prefix the "Review requests" filter conjoins (so the scoped view shows ONLY git
/// review-requests, not every cross-subsystem review-request). The git subsystem token of the canonical
/// `myelin://<tenant>/git/...` URN.
pub const GIT_SUBJECT_PREFIX: &str = "git/";

/// **The "Review requests" scoped-view filter over the ONE inbox (contract 7.1 — a FILTER, never a
/// second store).** Returns the `(reason, subject_prefix)` pair the scoped view conjoins onto
/// [`list_inbox`](myelin_notif::list_inbox): `reason = ReviewRequested` AND the subject is a git
/// artifact (`.../git/...`). The inbox store is Notif's ONE store; this is purely a read-side filter
/// (C-9 — a scoped view is a `filter` over `reason`/`subject`, never a second store).
pub fn git_review_requests_filter() -> (myelin_notif::Reason, &'static str) {
    (GIT_REVIEW_REQUESTS_FILTER_REASON, GIT_SUBJECT_PREFIX)
}

/// **`true` iff a `(reason, subject)` inbox row matches Git's "Review requests" scoped view** (the
/// filter predicate, contract 7.1). A row matches iff it is a `ReviewRequested` reason on a git subject.
/// This is the read-side predicate the scoped view applies — it does NOT move the row into a second
/// store (the row lives in the ONE inbox; the view just filters it in).
pub fn matches_review_requests_view(reason: myelin_notif::Reason, subject: &ArtifactRef) -> bool {
    reason == GIT_REVIEW_REQUESTS_FILTER_REASON && is_git_subject(subject)
}

/// `true` iff an `ArtifactRef` is a git subject (`myelin://<tenant>/git/...`). The git subsystem token
/// classification — never a content read.
fn is_git_subject(r: &ArtifactRef) -> bool {
    r.0.strip_prefix("myelin://")
        .and_then(|rest| rest.split('/').nth(1))
        .map(|subsystem| subsystem == "git")
        .unwrap_or(false)
}

/// **The REAL Git resolve seam for unfurls — `resolve(ref, viewer, Display)` over Git's `Projector`
/// (contract 5.2; the transport Notif's humanise binds its slots to).** Wraps Git's REAL
/// [`Projector`](crate::project::Projector) (contract 5.6 — permission-FIRST, per-viewer) and adapts its
/// [`Projected`](crate::project::Projected) into Notif's [`RefResolution`]. Notif holds this behind a
/// `&dyn RefResolvePort` (the same seam discipline as the rule registry — Notif holds the PORT, not the
/// full Git projector).
///
/// **Cell-local (contract 5.2 / OQ-I).** A git unfurl resolves the artifact in ITS home cell; the
/// cross-cell single-home resolve is the named multi-cell floor.
pub struct GitRefResolver<I: IdentityService> {
    /// Git's REAL per-viewer projector — the permission-first 5.6 projection the resolve adapts.
    projector: Projector<I>,
}

impl<I: IdentityService> GitRefResolver<I> {
    /// Compose the resolve seam over Git's REAL [`Projector`](crate::project::Projector).
    pub fn new(projector: Projector<I>) -> GitRefResolver<I> {
        GitRefResolver { projector }
    }

    /// A borrow of the underlying projector (for the front door / drills to seed its store or inspect).
    pub fn projector_mut(&mut self) -> &mut Projector<I> {
        &mut self.projector
    }

    /// Map Git's REAL [`Projected`](crate::project::Projected) into Notif's [`RefResolution`] (the leak
    /// invariant lives in the SHAPE — a tombstone has no title field). The ALLOWED branch carries the
    /// title + icon + the click-route ref; the denied/erased/restricted branch carries ONLY the opaque
    /// root + a PII-free reason (NEVER the title — it was never read on the deny path).
    ///
    /// A `ProjectError` (a malformed/non-git ref, a dangling ref, or the GIT-P24 blob floor) maps to a
    /// `RootGone` tombstone — the safe, non-leaking degrade (an un-projectable ref unfurls as a
    /// kind-shaped placeholder, never a panic, never a leak).
    fn to_resolution(reference: &ArtifactRef, projected: Projected) -> RefResolution {
        match projected {
            Projected::Visible(p) => RefResolution::Projection(RefProjection {
                ref_: reference.clone(),
                title: p.title,
                icon: p.icon,
            }),
            Projected::Tombstoned(t) => RefResolution::Tombstone(Tombstone {
                // The OPAQUE root crosses (so Notif renders `a restricted <kind>` from the URN); the
                // title/state never do (they live only on the Visible branch above).
                root: reference.clone(),
                reason: map_tombstone_reason(t.reason),
            }),
        }
    }
}

/// Map Git's [`TombstoneReason`](crate::project::TombstoneReason) onto Notif's PII-free
/// [`TombstoneReason`](myelin_notif::TombstoneReason). Both are STRUCTURED enums (never free text);
/// each renders a fixed, content-free display (`a restricted pr` / `[erased user]`). The mapping is
/// total — a future Git reason MUST be mapped here (the compiler enforces it), never silently widened.
fn map_tombstone_reason(reason: GitTombstoneReason) -> TombstoneReason {
    match reason {
        // Denied / restricted → the leak-free `a restricted <kind>` (the NOTIF-D4 chokepoint).
        GitTombstoneReason::Unauthorized => TombstoneReason::Denied,
        GitTombstoneReason::Restricted => TombstoneReason::Denied,
        // Erased → the canonical `[erased user]` / erased display (EI-04 §1).
        GitTombstoneReason::Erased => TombstoneReason::Erased,
        // A content-anchored line-range gone → the parent shows; the embed degrades to SubGone (the
        // anchored sub is gone but the root is not).
        GitTombstoneReason::ContentGone => TombstoneReason::SubGone,
    }
}

impl<I: IdentityService + Send + Sync> RefResolvePort for GitRefResolver<I> {
    /// **`resolve(ref, viewer, Display)` (contract 5.2) — over Git's REAL permission-first projector.**
    /// Per-viewer, permission-checked: a denied/erased/restricted ref returns a
    /// [`RefResolution::Tombstone`] (NEVER a title); an allowed ref returns a
    /// [`RefResolution::Projection`]. The `tenant`/`region` are carried for the cell-local resolve
    /// (contract 5.2 / OQ-I); the per-viewer decision keys on `viewer` (the TOKEN tenant — GIT-D8).
    fn resolve_display(
        &self,
        _tenant: &TenantId,
        _region: &Region,
        ref_: &ArtifactRef,
        viewer: &Principal,
        at: &Consistency,
    ) -> RefResolution {
        // The projector takes a read-consistency fence; humanise passes a strong zookie for a
        // security-sensitive render. A `ProjectError` degrades to a non-leaking `RootGone` tombstone.
        let zookie: Zookie = at.at_least.clone();
        match self.projector.project(ref_, viewer, zookie) {
            Ok(projected) => Self::to_resolution(ref_, projected),
            // A malformed / non-git / dangling / blob-floor ref → a safe, non-leaking placeholder
            // (never a panic, never a leak; the unfurl shows `a restricted <kind>` from the URN).
            Err(_) => RefResolution::Tombstone(Tombstone {
                root: ref_.clone(),
                reason: TombstoneReason::RootGone,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::Body;
    use crate::check_status::GateOutcome;
    use crate::lifecycle::PullRequest;
    use crate::project::{git_pr_ref, ArtifactStore};
    use myelin_identity::{
        AuthzError, CaveatContext, ConsistencyMode, Credential, Decision, ListObjectsResult,
        ObjectId, ObjectType, Permission, PrincipalId, PrincipalKind, Result as IdResult,
        RewriteTrace, SubjectTree, TupleDelta,
    };
    use myelin_notif::humanise::{humanise, Channel, TemplateStore, DEFAULT_LOCALE};
    use myelin_notif::reason_template_key;
    use myelin_notif::Reason;
    use std::collections::HashSet;

    // ── a deterministic Id stub: a `view@object` allow-list (absent ⇒ Deny, fail-closed). ──
    struct StubId {
        allow: HashSet<String>,
    }
    impl StubId {
        fn new() -> Self {
            Self {
                allow: HashSet::new(),
            }
        }
        fn allow_view(mut self, object: &ArtifactRef) -> Self {
            self.allow.insert(format!("view@{}", object.0));
            self
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
            _caveat: Option<&CaveatContext>,
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
            _at: &Consistency,
        ) -> IdResult<ListObjectsResult> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn list_subjects(
            &self,
            _o: &ObjectId,
            _p: &Permission,
            _at: &Consistency,
        ) -> IdResult<SubjectTree> {
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

    const SECRET_TITLE: &str = "rotate the PROJECT-NIGHTFALL signing key before the acquisition";

    fn acme() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }
    fn viewer(id: &str) -> Principal {
        Principal::new(
            acme(),
            region(),
            PrincipalId(id.into()),
            PrincipalKind::Human,
            myelin_identity::DataRole::Controller,
            myelin_identity::PrincipalStatus::Active,
        )
    }
    fn strong(zk: &str) -> Consistency {
        Consistency {
            at_least: Zookie(zk.into()),
            mode: ConsistencyMode::Strong,
        }
    }
    fn secret_pr() -> PullRequest {
        let mut pr = PullRequest::open(
            9,
            "refs/heads/main",
            "refs/heads/feature",
            "psn:alice",
            false,
        );
        pr.body = Body::new(SECRET_TITLE, vec![]);
        pr
    }

    /// A resolver over a private PR; `grant_view` grants `view` on the PR (so the projector allows it).
    /// Returns `(resolver, pr_ref)`.
    fn private_pr_resolver(grant_view: bool) -> (GitRefResolver<StubId>, ArtifactRef) {
        let pr_ref = git_pr_ref("acme", "repo7", 9);
        let mut store = ArtifactStore::new();
        store.put_pr(&pr_ref, secret_pr(), GateOutcome::AllRequiredGreen, 0, 0);
        let id = if grant_view {
            StubId::new().allow_view(&pr_ref)
        } else {
            StubId::new()
        };
        (GitRefResolver::new(Projector::new(id, store)), pr_ref)
    }

    /// **A denied viewer's resolve is a TOMBSTONE carrying NO title (the structural leak invariant).**
    /// The resolver delegates to Git's permission-first projector; a viewer lacking `pull` gets a
    /// `RefResolution::Tombstone` — a type with no `title` field for the secret to leak into.
    #[test]
    fn denied_viewer_resolves_to_a_tombstone_no_title() {
        let (resolver, pr_ref) = private_pr_resolver(false); // nobody granted
        let r = resolver.resolve_display(
            &acme(),
            &region(),
            &pr_ref,
            &viewer("ex-contractor"),
            &strong("zk-1"),
        );
        match r {
            RefResolution::Tombstone(t) => {
                assert_eq!(
                    t.root, pr_ref,
                    "the opaque root crosses (for `a restricted pr`)"
                );
                assert_eq!(t.reason, TombstoneReason::Denied);
            }
            RefResolution::Projection(_) => {
                panic!("a denied viewer must NOT get a projection (leak!)")
            }
        }
    }

    /// **A permitted viewer's resolve IS a projection carrying the title + a click-route (the gate
    /// discriminates — it is not a blanket redaction).** Proves the resolver is REAL, over Git's
    /// projector.
    #[test]
    fn permitted_viewer_resolves_to_a_projection_with_the_title() {
        let (resolver, pr_ref) = private_pr_resolver(true);
        let r = resolver.resolve_display(
            &acme(),
            &region(),
            &pr_ref,
            &viewer("maintainer"),
            &strong("zk-1"),
        );
        match r {
            RefResolution::Projection(p) => {
                assert_eq!(p.ref_, pr_ref);
                assert_eq!(
                    p.title, SECRET_TITLE,
                    "the permitted viewer sees the PR title"
                );
                assert_eq!(p.icon, "pr");
            }
            RefResolution::Tombstone(_) => panic!("the permitted viewer must see the projection"),
        }
    }

    /// **NOTIF-D4-class (the leak gate, unit slice): a confidential PR humanised for a DENIED viewer
    /// across every channel × every Git reason → 0 title leak.** The resolver feeds Notif's REAL
    /// humanise; the title appears EXACTLY ZERO times. Threshold 0, never softened.
    #[test]
    fn notif_d4_zero_title_leak_through_humanise() {
        let (resolver, pr_ref) = private_pr_resolver(false); // denied
        let templates = TemplateStore::with_platform_defaults();
        let reasons = [Reason::ReviewRequested, Reason::Mentioned, Reason::Watched];
        let mut renders = 0u64;
        let mut leaks = 0u64;
        for &reason in &reasons {
            let key = reason_template_key(reason);
            for channel in [Channel::Cli, Channel::Email, Channel::Markdown] {
                let h = humanise(
                    &resolver,
                    &acme(),
                    &region(),
                    &templates,
                    key,
                    std::slice::from_ref(&pr_ref),
                    &viewer("ex-contractor"),
                    DEFAULT_LOCALE,
                    &strong("zk-1"),
                    channel,
                );
                renders += 1;
                if h.text.contains(SECRET_TITLE) || h.text.to_lowercase().contains("nightfall") {
                    leaks += 1;
                }
                assert!(
                    h.text.contains("a restricted pr"),
                    "the denied render is a tombstone"
                );
                assert!(
                    h.links.is_empty(),
                    "a denied unfurl yields no click-route link"
                );
            }
        }
        assert_eq!(
            leaks, 0,
            "NOTIF-D4-class: 0 title leak over {renders} denied renders (threshold 0)"
        );
    }

    /// **The Git tombstone-reason mapping is total + PII-free** (every Git reason maps to a Notif
    /// content-free reason; never a free-text leak).
    #[test]
    fn tombstone_reason_mapping_is_total_and_pii_free() {
        assert_eq!(
            map_tombstone_reason(GitTombstoneReason::Unauthorized),
            TombstoneReason::Denied
        );
        assert_eq!(
            map_tombstone_reason(GitTombstoneReason::Restricted),
            TombstoneReason::Denied
        );
        assert_eq!(
            map_tombstone_reason(GitTombstoneReason::Erased),
            TombstoneReason::Erased
        );
        assert_eq!(
            map_tombstone_reason(GitTombstoneReason::ContentGone),
            TombstoneReason::SubGone
        );
    }

    /// **A malformed / non-git ref degrades to a non-leaking `RootGone` tombstone (never a panic).**
    #[test]
    fn an_unprojectable_ref_degrades_to_a_non_leaking_tombstone() {
        let (resolver, _) = private_pr_resolver(false);
        let not_git = ArtifactRef("myelin://acme/issue/issue/ENG-1".into());
        let r = resolver.resolve_display(
            &acme(),
            &region(),
            &not_git,
            &viewer("alice"),
            &strong("zk-1"),
        );
        match r {
            RefResolution::Tombstone(t) => assert_eq!(t.reason, TombstoneReason::RootGone),
            RefResolution::Projection(_) => panic!("a non-git ref must not project a title"),
        }
    }

    /// **Review-requests are a FILTER over the ONE inbox (7.1), never a second store.** The scoped-view
    /// predicate matches a `ReviewRequested` git subject and rejects a non-review / non-git row.
    #[test]
    fn review_requests_is_a_filter_over_the_one_inbox() {
        let (reason, prefix) = git_review_requests_filter();
        assert_eq!(reason, Reason::ReviewRequested);
        assert_eq!(prefix, "git/");

        let git_pr = git_pr_ref("acme", "repo7", 9);
        // a review-request ON a git PR matches the scoped view.
        assert!(matches_review_requests_view(
            Reason::ReviewRequested,
            &git_pr
        ));
        // a MENTION on a git PR does NOT (the filter is review-requests only).
        assert!(!matches_review_requests_view(Reason::Mentioned, &git_pr));
        // a review-request on a NON-git subject does NOT (the prefix conjunct rejects it).
        let issue = ArtifactRef("myelin://acme/issue/issue/ENG-1".into());
        assert!(!matches_review_requests_view(
            Reason::ReviewRequested,
            &issue
        ));
    }
}
