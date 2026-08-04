use std::collections::HashSet;
use std::sync::Mutex;

use myelin_identity::{
    AuthzError, CaveatContext, Consistency, ConsistencyMode, Credential, Decision, IdentityService,
    ListObjectsResult, ObjectId, ObjectType, Permission, Principal, PrincipalId, PrincipalKind,
    Result as IdResult, RewriteTrace, SubjectTree, TupleDelta, Zookie,
};
use myelin_knowledge::refs_glue::{PageMeta, PageStore, Projector};
use myelin_knowledge::KnowledgeRefResolver;
use myelin_notif::humanise::{
    humanise, Channel, RefResolution, RefResolvePort, TemplateStore, DEFAULT_LOCALE,
};
use myelin_notif::{reason_template_key, Reason};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};

const SECRET_PAGE_TITLE: &str =
    "Incident PROJECT-NIGHTFALL: signing-key rotation before the acquisition";

const KN_REASONS: &[Reason] = &[
    Reason::Mentioned,
    Reason::Comments,
    Reason::Shared,
    Reason::Watched,
];

fn acme() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn viewer(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, acme())
}
fn strong(zk: &str) -> Consistency {
    Consistency {
        at_least: Zookie(zk.into()),
        mode: ConsistencyMode::Strong,
    }
}
fn confidential_page() -> ArtifactRef {
    ArtifactRef("myelin://acme/knowledge/page/runbook9".into())
}

fn contains_leak(text: &str) -> bool {
    let lc = text.to_lowercase();
    text.contains(SECRET_PAGE_TITLE)
        || lc.contains("nightfall")
        || lc.contains("acquisition")
        || lc.contains("signing-key")
}

struct MutableId {
    allow: Mutex<HashSet<String>>,
}
impl MutableId {
    fn new() -> Self {
        Self {
            allow: Mutex::new(HashSet::new()),
        }
    }
    fn grant_read(&self, object: &ArtifactRef) {
        self.allow
            .lock()
            .unwrap()
            .insert(format!("read@{}", object.0));
    }
    fn revoke_read(&self, object: &ArtifactRef) {
        self.allow
            .lock()
            .unwrap()
            .remove(&format!("read@{}", object.0));
    }
}
impl IdentityService for MutableId {
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
        Ok(if self.allow.lock().unwrap().contains(&key) {
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

fn resolver_with(id: MutableId) -> KnowledgeRefResolver<MutableId> {
    let mut store = PageStore::new();
    store.put_root(
        &confidential_page(),
        PageMeta {
            title: SECRET_PAGE_TITLE.into(),
            state: "published".into(),
        },
    );
    KnowledgeRefResolver::new(Projector::new(id, store))
}

#[test]
fn notif_d4_zero_leak_on_real_kn_confidential_page() {
    let resolver = resolver_with(MutableId::new());
    let templates = TemplateStore::with_platform_defaults();
    let subject = confidential_page();
    let denied = ["ex-contractor", "wrong-space-member", "intern-no-access"];

    let mut renders = 0u64;
    let mut leak_count = 0u64;
    let mut tombstone_present = 0u64;

    for v in denied {
        for &reason in KN_REASONS {
            let key = reason_template_key(reason);
            for channel in [Channel::Cli, Channel::Email, Channel::Markdown] {
                let h = humanise(
                    &resolver,
                    &acme(),
                    &region(),
                    &templates,
                    key,
                    std::slice::from_ref(&subject),
                    &viewer(v),
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
        "NOTIF-D4 GREEN on a REAL KN subject (2026-06-22): {renders} denied renders, \
         title-leak-count = {leak_count} (threshold 0), tombstone = {tombstone_present}/{renders}"
    );
}

#[test]
fn notif_d4_permitted_kn_viewer_sees_the_page_title() {
    let id = MutableId::new();
    id.grant_read(&confidential_page());
    let resolver = resolver_with(id);
    let h = humanise(
        &resolver,
        &acme(),
        &region(),
        &TemplateStore::with_platform_defaults(),
        reason_template_key(Reason::Mentioned),
        std::slice::from_ref(&confidential_page()),
        &viewer("maintainer"),
        DEFAULT_LOCALE,
        &strong("zk-1"),
        Channel::Cli,
    );
    assert!(
        h.text.contains(SECRET_PAGE_TITLE),
        "the permitted maintainer sees the page title"
    );
    assert_eq!(
        h.links,
        vec![confidential_page().0],
        "the allowed branch yields the click-route link"
    );
}

#[test]
fn notif_d4_revoke_flips_title_to_tombstone_no_rewrite() {
    let id = MutableId::new();
    id.grant_read(&confidential_page());
    let resolver = resolver_with(id);
    let subject = confidential_page();

    match resolver.resolve_display(
        &acme(),
        &region(),
        &subject,
        &viewer("alice"),
        &strong("zk-1"),
    ) {
        RefResolution::Projection(p) => assert_eq!(p.title, SECRET_PAGE_TITLE),
        RefResolution::Tombstone(_) => panic!("alice has read → must project the title"),
    }

    let revoked_id = MutableId::new();
    revoked_id.grant_read(&subject);
    revoked_id.revoke_read(&subject);
    let revoked = resolver_with(revoked_id);
    let h = humanise(
        &revoked,
        &acme(),
        &region(),
        &TemplateStore::with_platform_defaults(),
        reason_template_key(Reason::Mentioned),
        std::slice::from_ref(&subject),
        &viewer("alice"),
        DEFAULT_LOCALE,
        &strong("zk-2"),
        Channel::Cli,
    );
    assert!(
        h.text.contains("a restricted page"),
        "after revoke the slot is a tombstone"
    );
    assert!(
        !contains_leak(&h.text),
        "the title never leaks after revoke (0 leak, no re-write)"
    );
    assert!(
        h.links.is_empty(),
        "a revoked subject yields no click-route link"
    );
}
