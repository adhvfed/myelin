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
use std::collections::HashSet;

const SECRET_TITLE: &str = "Q3 layoffs runbook (confidential)";

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

struct StubId {
    allow: HashSet<String>,
}
impl StubId {
    fn new() -> Self {
        Self {
            allow: HashSet::new(),
        }
    }
    fn allow_read(mut self, object: &ArtifactRef) -> Self {
        self.allow.insert(format!("read@{}", object.0));
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

fn page() -> ArtifactRef {
    ArtifactRef("myelin://acme/knowledge/page/runbook7".into())
}

fn resolver(grant_read: bool) -> KnowledgeRefResolver<StubId> {
    let mut store = PageStore::new();
    store.put_root(
        &page(),
        PageMeta {
            title: SECRET_TITLE.into(),
            state: "published".into(),
        },
    );
    let id = if grant_read {
        StubId::new().allow_read(&page())
    } else {
        StubId::new()
    };
    KnowledgeRefResolver::new(Projector::new(id, store))
}

#[test]
fn provider_knowledge_resolve_is_projection_or_tombstone_permission_first() {
    let allowed = resolver(true);
    match allowed.resolve_display(
        &acme(),
        &region(),
        &page(),
        &viewer("editor"),
        &strong("zk-1"),
    ) {
        RefResolution::Projection(p) => {
            assert_eq!(p.ref_, page());
            assert_eq!(p.title, SECRET_TITLE);
            assert_eq!(p.icon, "page", "the page Display projection icon");
        }
        RefResolution::Tombstone(_) => panic!("an allowed viewer must project the title"),
    }
    let denied = resolver(false);
    match denied.resolve_display(
        &acme(),
        &region(),
        &page(),
        &viewer("contractor"),
        &strong("zk-1"),
    ) {
        RefResolution::Tombstone(t) => assert_eq!(t.root, page()),
        RefResolution::Projection(_) => panic!("a denied viewer must NOT project a title (leak!)"),
    }
}

#[test]
fn consumer_humanise_renders_the_kn_display_projection_per_viewer() {
    let templates = TemplateStore::with_platform_defaults();
    let key = reason_template_key(Reason::Mentioned);

    let allowed = resolver(true);
    let h = humanise(
        &allowed,
        &acme(),
        &region(),
        &templates,
        key,
        std::slice::from_ref(&page()),
        &viewer("editor"),
        DEFAULT_LOCALE,
        &strong("zk-1"),
        Channel::Cli,
    );
    assert!(
        h.text.contains(SECRET_TITLE),
        "the allowed viewer's humanised string shows the title"
    );
    assert_eq!(
        h.links,
        vec![page().0],
        "the allowed branch carries the click-route link"
    );

    let denied = resolver(false);
    let h = humanise(
        &denied,
        &acme(),
        &region(),
        &templates,
        key,
        std::slice::from_ref(&page()),
        &viewer("contractor"),
        DEFAULT_LOCALE,
        &strong("zk-1"),
        Channel::Cli,
    );
    assert!(
        h.text.contains("a restricted page"),
        "the denied viewer sees the PII-free tombstone"
    );
    assert!(
        !h.text.contains(SECRET_TITLE),
        "the title never leaks to the denied viewer"
    );
    assert!(
        h.links.is_empty(),
        "a denied subject yields no click-route link"
    );
}

#[test]
fn the_kn_reason_vocabulary_agrees_between_7_6_registration_and_7_3_render() {
    use myelin_identity_service::knowledge_rules::knowledge_notif_rules;
    let rules = knowledge_notif_rules().expect("kn's set is table-correct");
    let templates = TemplateStore::with_platform_defaults();
    for (key, rule) in &rules {
        let tkey = reason_template_key(rule.reason);
        assert!(
            templates.lookup(&acme().0, tkey, DEFAULT_LOCALE).is_some(),
            "rule `{key}` reason {:?} (template key `{tkey}`) has a platform humanise template",
            rule.reason
        );
    }
    let reasons: Vec<Reason> = rules.iter().map(|(_, r)| r.reason).collect();
    assert_eq!(
        reasons,
        vec![
            Reason::Mentioned,
            Reason::Comments,
            Reason::Shared,
            Reason::Watched
        ],
        "the KN reason set the 7.3 render covers"
    );
}

#[test]
fn notif_d4_zero_title_leak_over_every_kn_reason_and_channel() {
    let denied = resolver(false);
    let templates = TemplateStore::with_platform_defaults();
    let reasons = [
        Reason::Mentioned,
        Reason::Comments,
        Reason::Shared,
        Reason::Watched,
    ];
    let mut renders = 0u64;
    let mut leaks = 0u64;
    for &reason in &reasons {
        for channel in [Channel::Cli, Channel::Email, Channel::Markdown] {
            let h = humanise(
                &denied,
                &acme(),
                &region(),
                &templates,
                reason_template_key(reason),
                std::slice::from_ref(&page()),
                &viewer("contractor"),
                DEFAULT_LOCALE,
                &strong("zk-1"),
                channel,
            );
            renders += 1;
            if h.text.contains(SECRET_TITLE) || h.text.to_lowercase().contains("layoffs") {
                leaks += 1;
            }
            assert!(
                h.text.contains("a restricted page"),
                "every denied render is a tombstone"
            );
        }
    }
    assert_eq!(
        leaks, 0,
        "NOTIF-D4 (KN slice): 0 title leak over {renders} denied renders (threshold 0)"
    );
    eprintln!("NOTIF-D4 KN 7.3 slice GREEN (2026-06-22): {renders} renders, leak-count = {leaks}");
}
