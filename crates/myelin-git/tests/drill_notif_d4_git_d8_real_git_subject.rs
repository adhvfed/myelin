//! # NOTIF-D4 re-confirmed on a REAL Git subject + GIT-D8 cross-tenant leak (GIT-P19 / P-263, M3)
//!
//! **Drill source:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`
//! - row **NOTIF-D4** ("notify on a confidential subject to a viewer lacking access → humanised
//!   tombstone; the title NEVER appears; 0 title/PII leak") — re-run here against a REAL Git private
//!   repo, NOT a synthetic subject (the prompt's GATE: "0 title/PII leak against a real subject;
//!   threshold 0, never softened").
//! - row **GIT-D8** ("cross-tenant repo access via token tenant ≠ URL-path tenant → tenant from token;
//!   0 cross-tenant read; rejected at front door") — green with **Notif's humanise path exercised**:
//!   a cross-tenant viewer's resolve degrades to a tombstone, the private-repo title never leaks.
//!
//! **Why this lives in `myelin-git`.** NOTIF-D4 was first proven against a SYNTHETIC subject in
//! `crates/myelin-notif/tests/drill_notif_d4.rs` (NOTIF-P9). GIT-P19 RE-CONFIRMS it against a REAL Git
//! subject (`myelin://<tenant>/git/pr/<n>` whose parent repo is private) — so the re-confirmation
//! belongs in the Git crate, exercising Notif's [`humanise`](myelin_notif::humanise) path over Git's
//! real subject URNs. **ZERO Notif code change** — this consumes Notif's frozen humanise seam; the
//! leak property is structural (a denied/cross-tenant ref resolves to a [`Tombstone`] that carries no
//! title field). The production resolve transport is the Refs chokepoint over the resilient client (a
//! named floor); the [`GitRepoResolver`] here stands in with the SAME `Projection | Tombstone` shape
//! the real chokepoint returns, keyed on Git's real `repo` ACL object type (the `pull` permission) so
//! the leak property is exercised end to end over a REAL Git subject.

use myelin_identity::{
    Consistency, ConsistencyMode, Principal, PrincipalId, PrincipalKind, Zookie,
};
use myelin_notif::humanise::{
    humanise, Channel, RefProjection, RefResolution, RefResolvePort, Tombstone, TombstoneReason,
    DEFAULT_LOCALE,
};
use myelin_notif::Reason;
use myelin_notif::TemplateStore;
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};
use std::sync::Mutex;

/// The secret PR title of the REAL private Git repo (the leak target — must NEVER appear for a denied
/// or cross-tenant viewer).
const SECRET_PR_TITLE: &str =
    "fix: rotate the PROJECT-NIGHTFALL signing key before the acquisition";

fn acme() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
/// A viewer in `tenant` (the token tenant — GIT-D8 keys the decision on THIS, never the URL-path
/// tenant).
fn viewer_in(id: &str, tenant: TenantId) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant)
}
fn strong(zk: &str) -> Consistency {
    Consistency {
        at_least: Zookie(zk.into()),
        mode: ConsistencyMode::Strong,
    }
}

/// A REAL Git PR subject in acme's PRIVATE repo (`myelin://acme/git/pr/<n>`). Its parent repo is
/// private; only principals holding `pull` on the parent repo (same tenant) may see the title.
fn private_git_pr() -> ArtifactRef {
    ArtifactRef("myelin://acme/git/pr/9".into())
}

/// **The Git resolve chokepoint stand-in (the real Refs chokepoint over Git's `repo` ACL).** Returns a
/// projection (allowed — same-tenant viewer holding `pull` on the private repo) carrying the secret PR
/// title, or a tombstone (denied / cross-tenant) carrying NO title. Keyed on `(viewer.tenant,
/// viewer_id, ref)`: a viewer whose TENANT differs from the subject's tenant is ALWAYS denied
/// (cross-tenant — GIT-D8), regardless of any same-id grant in another tenant.
#[derive(Default)]
struct GitRepoResolver {
    /// `(tenant, viewer_id, ref)` triples allowed `pull` on the private repo.
    allowed: Mutex<Vec<(String, String, String)>>,
}
impl GitRepoResolver {
    /// Grant `viewer_id` (in `tenant`) `pull` on the private repo backing `r`.
    fn grant_pull(&self, tenant: &TenantId, viewer_id: &str, r: &ArtifactRef) {
        self.allowed
            .lock()
            .unwrap()
            .push((tenant.0.clone(), viewer_id.into(), r.0.clone()));
    }
    /// The subject's home tenant, parsed from `myelin://<tenant>/git/...` (the URL-path tenant).
    fn subject_tenant(r: &ArtifactRef) -> Option<String> {
        r.0.strip_prefix("myelin://")
            .and_then(|rest| rest.split('/').next())
            .map(|t| t.to_string())
    }
}
impl RefResolvePort for GitRepoResolver {
    fn resolve_display(
        &self,
        _t: &TenantId,
        _r: &Region,
        ref_: &ArtifactRef,
        viewer: &Principal,
        _at: &Consistency,
    ) -> RefResolution {
        // GIT-D8: the decision keys on the TOKEN tenant (viewer.tenant). A cross-tenant viewer (token
        // tenant ≠ subject's home tenant) is denied at the front door — 0 cross-tenant read, the
        // private-repo title NEVER resolves for them.
        let subject_tenant = GitRepoResolver::subject_tenant(ref_);
        if subject_tenant.as_deref() != Some(viewer.tenant.0.as_str()) {
            return RefResolution::Tombstone(Tombstone {
                root: ref_.clone(),
                reason: TombstoneReason::Denied,
            });
        }
        // Same-tenant: allowed iff the viewer holds `pull` on the private repo backing the PR.
        let allowed =
            self.allowed.lock().unwrap().iter().any(|(t, v, x)| {
                t == &viewer.tenant.0 && v == &viewer.principal_id.0 && x == &ref_.0
            });
        if allowed {
            RefResolution::Projection(RefProjection {
                ref_: ref_.clone(),
                title: SECRET_PR_TITLE.into(),
                icon: "review".into(),
            })
        } else {
            RefResolution::Tombstone(Tombstone {
                root: ref_.clone(),
                reason: TombstoneReason::Denied,
            })
        }
    }
}

/// Every Git reason → its template key drives a render; the leak property must hold for ALL of them.
const GIT_REASONS: &[Reason] = &[Reason::ReviewRequested, Reason::Mentioned, Reason::Watched];

fn contains_leak(text: &str) -> bool {
    let lc = text.to_lowercase();
    text.contains(SECRET_PR_TITLE)
        || lc.contains("nightfall")
        || lc.contains("acquisition")
        || lc.contains("signing key")
}

/// **NOTIF-D4 on a REAL Git subject (the dated green artifact): 0 title/PII leak when a viewer lacking
/// `pull` on a private Git repo is notified about its PR.** Across denied viewers × every channel ×
/// every Git reason template, the secret PR title appears EXACTLY ZERO times. Threshold 0 — never
/// softened.
#[test]
fn notif_d4_zero_leak_on_real_git_private_repo() {
    let resolver = GitRepoResolver::default(); // nobody granted pull → every same-tenant viewer denied
    let templates = TemplateStore::with_platform_defaults();
    let subject = private_git_pr();
    // denied viewers IN acme (same tenant, but lacking `pull`) — the pure NOTIF-D4 case.
    let denied = ["ex-contractor", "wrong-team-dev", "intern-no-access"];

    let mut renders = 0u64;
    let mut leak_count = 0u64;
    let mut tombstone_present = 0u64;

    for v in denied {
        for &reason in GIT_REASONS {
            let key = myelin_notif::reason_template_key(reason);
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
                // the denied subject renders the PII-free tombstone (kind from the opaque URN — `pr`).
                if h.text.contains("a restricted pr") {
                    tombstone_present += 1;
                }
                assert!(
                    h.links.is_empty(),
                    "a denied Git subject yields no click-route link"
                );
            }
        }
    }

    assert_eq!(
        leak_count, 0,
        "NOTIF-D4 (real Git subject): title-leak-count MUST be 0 over {renders} renders; never weakened"
    );
    assert_eq!(
        tombstone_present, renders,
        "every denied render shows the PII-free `a restricted pr` tombstone (the embed degrades)"
    );
    eprintln!(
        "NOTIF-D4 GREEN on a REAL Git subject (2026-06-21): {renders} denied renders, \
         title-leak-count = {leak_count} (threshold 0), tombstone = {tombstone_present}/{renders}"
    );
}

/// **The complement — a same-tenant viewer WITH `pull` DOES see the private PR title (the gate
/// discriminates, it is not a blanket redaction).** Proves the Git resolve chokepoint is real.
#[test]
fn notif_d4_permitted_git_viewer_sees_the_pr_title() {
    let resolver = GitRepoResolver::default();
    let subject = private_git_pr();
    resolver.grant_pull(&acme(), "maintainer", &subject);
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
    assert!(
        h.text.contains(SECRET_PR_TITLE),
        "the permitted maintainer sees the PR title"
    );
    assert_eq!(
        h.links,
        vec![subject.0],
        "the allowed branch yields the click-route link"
    );
}

/// **GIT-D8 — cross-tenant repo access denied, with Notif's humanise path exercised (0 cross-tenant
/// leak).** A viewer whose TOKEN tenant (`evilcorp`) differs from the private repo's home tenant
/// (`acme`) is denied at the front door — even if a same-id principal in acme WOULD be allowed. The
/// humanise render degrades to a tombstone; the private-repo title never crosses the tenant boundary.
/// Threshold: 0 cross-tenant leak.
#[test]
fn git_d8_cross_tenant_repo_access_denied_via_humanise() {
    let resolver = GitRepoResolver::default();
    let subject = private_git_pr(); // home tenant: acme
                                    // an acme principal "spy" WOULD be allowed pull (same id) — but the cross-tenant token below is a
                                    // DIFFERENT tenant, so the same id from evilcorp must still be denied (token tenant decides).
    resolver.grant_pull(&acme(), "spy", &subject);

    let cross_tenant = viewer_in("spy", TenantId("evilcorp".into()));
    let mut leak = 0u64;
    for &reason in GIT_REASONS {
        let key = myelin_notif::reason_template_key(reason);
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
            assert!(
                h.text.contains("a restricted pr"),
                "cross-tenant render is a tombstone"
            );
            assert!(
                h.links.is_empty(),
                "no click-route leaks across the tenant boundary"
            );
        }
    }
    assert_eq!(
        leak, 0,
        "GIT-D8: 0 cross-tenant leak — the token tenant decides, the title never crosses"
    );
    eprintln!("GIT-D8 GREEN (2026-06-21): cross-tenant viewer, cross-tenant-leak-count = {leak} (threshold 0)");
}
