use myelin_harness::telemetry::{Predicate, SignalName, SignalSource};
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

const SECRET_PAGE_TITLE: &str =
    "Q3 layoffs: the PROJECT-NIGHTFALL severance list before the announcement";

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

fn confidential_kn_page() -> ArtifactRef {
    ArtifactRef("myelin://acme/knowledge/page/secret".into())
}

#[derive(Default)]
struct KnowledgePageResolver {
    allowed: Mutex<Vec<(String, String, String)>>,
    blocked: Mutex<Vec<(String, String, String)>>,
}
impl KnowledgePageResolver {
    fn grant_read(&self, tenant: &TenantId, viewer_id: &str, r: &ArtifactRef) {
        self.allowed
            .lock()
            .unwrap()
            .push((tenant.0.clone(), viewer_id.into(), r.0.clone()));
    }
    fn block(&self, tenant: &TenantId, viewer_id: &str, r: &ArtifactRef) {
        self.blocked
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
impl RefResolvePort for KnowledgePageResolver {
    fn resolve_display(
        &self,
        _t: &TenantId,
        _r: &Region,
        ref_: &ArtifactRef,
        viewer: &Principal,
        _at: &Consistency,
    ) -> RefResolution {
        let subject_tenant = KnowledgePageResolver::subject_tenant(ref_);
        if subject_tenant.as_deref() != Some(viewer.tenant.0.as_str()) {
            return RefResolution::Tombstone(Tombstone {
                root: ref_.clone(),
                reason: TombstoneReason::Denied,
            });
        }
        let is_blocked =
            self.blocked.lock().unwrap().iter().any(|(t, v, x)| {
                t == &viewer.tenant.0 && v == &viewer.principal_id.0 && x == &ref_.0
            });
        if is_blocked {
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
                title: SECRET_PAGE_TITLE.into(),
                icon: "page".into(),
            })
        } else {
            RefResolution::Tombstone(Tombstone {
                root: ref_.clone(),
                reason: TombstoneReason::Denied,
            })
        }
    }
}

const KN_REASONS: &[Reason] = &[
    Reason::Mentioned,
    Reason::Comments,
    Reason::Shared,
    Reason::Watched,
];

fn contains_leak(text: &str) -> bool {
    let lc = text.to_lowercase();
    text.contains(SECRET_PAGE_TITLE)
        || lc.contains("nightfall")
        || lc.contains("layoffs")
        || lc.contains("severance")
}

#[test]
fn notif_d4_zero_leak_on_real_confidential_kn_page() {
    let resolver = KnowledgePageResolver::default();
    let subject = confidential_kn_page();
    resolver.block(&acme(), "blocked-by-override", &subject);

    let templates = TemplateStore::with_platform_defaults();
    let denied = ["ex-contractor", "wrong-team-dev", "blocked-by-override"];

    let mut renders = 0u64;
    let mut leak_count = 0u64;
    let mut tombstone_present = 0u64;

    for v in denied {
        for &reason in KN_REASONS {
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
                if h.text.contains("a restricted page") {
                    tombstone_present += 1;
                }
                assert!(
                    h.links.is_empty(),
                    "a denied KN subject yields no click-route link"
                );
            }
        }
    }

    assert_eq!(
        leak_count, 0,
        "NOTIF-D4 (real KN subject): title-leak-count MUST be 0 over {renders} renders; never weakened"
    );
    assert_eq!(
        tombstone_present, renders,
        "every denied render shows the PII-free `a restricted page` tombstone (the embed degrades)"
    );
    eprintln!(
        "NOTIF-D4 GREEN on a REAL KN subject (2026-06-21): {renders} denied renders \
         (incl. a - direct_block override viewer), title-leak-count = {leak_count} (threshold 0), \
         tombstone = {tombstone_present}/{renders}"
    );
}

#[test]
fn notif_d4_permitted_kn_viewer_sees_the_page_title() {
    let resolver = KnowledgePageResolver::default();
    let subject = confidential_kn_page();
    resolver.grant_read(&acme(), "editor", &subject);
    let h = humanise(
        &resolver,
        &acme(),
        &region(),
        &TemplateStore::with_platform_defaults(),
        "shared",
        std::slice::from_ref(&subject),
        &viewer_in("editor", acme()),
        DEFAULT_LOCALE,
        &strong("zk-1"),
        Channel::Cli,
    );
    assert!(
        h.text.contains(SECRET_PAGE_TITLE),
        "the permitted editor sees the page title"
    );
    assert_eq!(
        h.links,
        vec![subject.0],
        "the allowed branch yields the click-route link"
    );
}

#[test]
fn kn_d13_cross_tenant_page_access_denied_via_humanise() {
    let mut signals = SignalSource::new();
    let resolver = KnowledgePageResolver::default();
    let subject = confidential_kn_page();
    resolver.grant_read(&acme(), "spy", &subject);

    let cross_tenant = viewer_in("spy", TenantId("evilcorp".into()));
    let mut leak = 0u64;
    let mut cross_tenant_reads: i64 = 0;
    for &reason in KN_REASONS {
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
            if h.links.contains(&subject.0) {
                cross_tenant_reads += 1;
            }
            assert!(
                h.text.contains("a restricted page"),
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
        "KN-D13: 0 cross-tenant leak - the token tenant decides, the title never crosses"
    );

    signals.set_scalar(SignalName::CrossTenantCount, cross_tenant_reads);
    signals
        .assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();

    eprintln!(
        "KN-D13 GREEN (2026-06-21): cross-tenant viewer, cross-tenant-leak-count = {leak} \
         (threshold 0), CrossTenantCount = {cross_tenant_reads}"
    );
}
