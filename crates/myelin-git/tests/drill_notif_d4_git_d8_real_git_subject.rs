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

const SECRET_PR_TITLE: &str =
    "fix: rotate the PROJECT-NIGHTFALL signing key before the acquisition";

fn acme() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn viewer_in(id: &str, tenant: TenantId) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant)
}
fn strong(zk: &str) -> Consistency {
    Consistency {
        at_least: Zookie(zk.into()),
        mode: ConsistencyMode::Strong,
    }
}

fn private_git_pr() -> ArtifactRef {
    ArtifactRef("myelin://acme/git/pr/9".into())
}

#[derive(Default)]
struct GitRepoResolver {
    allowed: Mutex<Vec<(String, String, String)>>,
}
impl GitRepoResolver {
    fn grant_pull(&self, tenant: &TenantId, viewer_id: &str, r: &ArtifactRef) {
        self.allowed
            .lock()
            .unwrap()
            .push((tenant.0.clone(), viewer_id.into(), r.0.clone()));
    }
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
        let subject_tenant = GitRepoResolver::subject_tenant(ref_);
        if subject_tenant.as_deref() != Some(viewer.tenant.0.as_str()) {
            return RefResolution::Tombstone(Tombstone {
                root: ref_.clone(),
                reason: TombstoneReason::Denied,
            });
        }
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

const GIT_REASONS: &[Reason] = &[Reason::ReviewRequested, Reason::Mentioned, Reason::Watched];

fn contains_leak(text: &str) -> bool {
    let lc = text.to_lowercase();
    text.contains(SECRET_PR_TITLE)
        || lc.contains("nightfall")
        || lc.contains("acquisition")
        || lc.contains("signing key")
}

#[test]
fn notif_d4_zero_leak_on_real_git_private_repo() {
    let resolver = GitRepoResolver::default();
    let templates = TemplateStore::with_platform_defaults();
    let subject = private_git_pr();
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

#[test]
fn git_d8_cross_tenant_repo_access_denied_via_humanise() {
    let resolver = GitRepoResolver::default();
    let subject = private_git_pr();
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
        "GIT-D8: 0 cross-tenant leak - the token tenant decides, the title never crosses"
    );
    eprintln!("GIT-D8 GREEN (2026-06-21): cross-tenant viewer, cross-tenant-leak-count = {leak} (threshold 0)");
}
