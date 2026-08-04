use myelin_identity::{Consistency, IdentityService, Principal, Zookie};
use myelin_notif::humanise::{
    RefProjection, RefResolution, RefResolvePort, Tombstone, TombstoneReason,
};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};

use crate::refs_glue::{Projected, Projector, TombstoneReason as KnTombstoneReason};

pub struct KnowledgeRefResolver<I: IdentityService> {
    projector: Projector<I>,
}

impl<I: IdentityService> KnowledgeRefResolver<I> {
    pub fn new(projector: Projector<I>) -> KnowledgeRefResolver<I> {
        KnowledgeRefResolver { projector }
    }

    pub fn projector_mut(&mut self) -> &mut Projector<I> {
        &mut self.projector
    }

    fn to_resolution(reference: &ArtifactRef, projected: Projected) -> RefResolution {
        match projected {
            Projected::Visible(p) => RefResolution::Projection(RefProjection {
                ref_: reference.clone(),
                title: p.title,
                icon: p.icon,
            }),
            Projected::Tombstoned(t) => RefResolution::Tombstone(Tombstone {
                root: t.root,
                reason: map_tombstone_reason(t.reason),
            }),
        }
    }
}

fn map_tombstone_reason(reason: KnTombstoneReason) -> TombstoneReason {
    match reason {
        KnTombstoneReason::Denied => TombstoneReason::Denied,
        KnTombstoneReason::RootGone => TombstoneReason::RootGone,
        KnTombstoneReason::SubGone => TombstoneReason::SubGone,
        KnTombstoneReason::Erased => TombstoneReason::Erased,
    }
}

impl<I: IdentityService + Send + Sync> RefResolvePort for KnowledgeRefResolver<I> {
    fn resolve_display(
        &self,
        _tenant: &TenantId,
        _region: &Region,
        ref_: &ArtifactRef,
        viewer: &Principal,
        at: &Consistency,
    ) -> RefResolution {
        let zookie: Zookie = at.at_least.clone();
        match self.projector.project(ref_, viewer, zookie) {
            Ok(projected) => Self::to_resolution(ref_, projected),
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
    use crate::refs_glue::{PageMeta, PageStore, Projector};
    use myelin_identity::{
        AuthzError, CaveatContext, ConsistencyMode, Credential, Decision, ListObjectsResult,
        ObjectId, ObjectType, Permission, PrincipalId, PrincipalKind, Result as IdResult,
        RewriteTrace, SubjectTree, TupleDelta,
    };
    use myelin_notif::humanise::{humanise, Channel, TemplateStore, DEFAULT_LOCALE};
    use myelin_notif::reason_template_key;
    use myelin_notif::Reason;
    use std::collections::HashSet;

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

    const SECRET_TITLE: &str =
        "Incident runbook: rotate the PROJECT-NIGHTFALL key before acquisition";

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
    fn secret_page() -> ArtifactRef {
        ArtifactRef("myelin://acme/knowledge/page/7c2".into())
    }

    fn confidential_page_resolver(grant_read: bool) -> (KnowledgeRefResolver<StubId>, ArtifactRef) {
        let page = secret_page();
        let mut store = PageStore::new();
        store.put_root(
            &page,
            PageMeta {
                title: SECRET_TITLE.into(),
                state: "published".into(),
            },
        );
        let id = if grant_read {
            StubId::new().allow_read(&page)
        } else {
            StubId::new()
        };
        (KnowledgeRefResolver::new(Projector::new(id, store)), page)
    }

    #[test]
    fn denied_viewer_resolves_to_a_tombstone_no_title() {
        let (resolver, page) = confidential_page_resolver(false);
        let r = resolver.resolve_display(
            &acme(),
            &region(),
            &page,
            &viewer("ex-contractor"),
            &strong("zk-1"),
        );
        match r {
            RefResolution::Tombstone(t) => {
                assert_eq!(
                    t.root, page,
                    "the opaque root crosses (for `a restricted page`)"
                );
                assert_eq!(t.reason, TombstoneReason::Denied);
            }
            RefResolution::Projection(_) => {
                panic!("a denied viewer must NOT get a projection (leak!)")
            }
        }
    }

    #[test]
    fn permitted_viewer_resolves_to_a_projection_with_the_title() {
        let (resolver, page) = confidential_page_resolver(true);
        let r = resolver.resolve_display(
            &acme(),
            &region(),
            &page,
            &viewer("maintainer"),
            &strong("zk-1"),
        );
        match r {
            RefResolution::Projection(p) => {
                assert_eq!(p.ref_, page);
                assert_eq!(
                    p.title, SECRET_TITLE,
                    "the permitted viewer sees the page title"
                );
                assert_eq!(p.icon, "page");
            }
            RefResolution::Tombstone(_) => panic!("the permitted viewer must see the projection"),
        }
    }

    #[test]
    fn notif_d4_zero_title_leak_through_humanise() {
        let (resolver, page) = confidential_page_resolver(false);
        let templates = TemplateStore::with_platform_defaults();
        let reasons = [
            Reason::Mentioned,
            Reason::Comments,
            Reason::Shared,
            Reason::Watched,
        ];
        let mut renders = 0u64;
        let mut leaks = 0u64;
        let mut tombstones = 0u64;
        for &reason in &reasons {
            let key = reason_template_key(reason);
            for channel in [Channel::Cli, Channel::Email, Channel::Markdown] {
                let h = humanise(
                    &resolver,
                    &acme(),
                    &region(),
                    &templates,
                    key,
                    std::slice::from_ref(&page),
                    &viewer("ex-contractor"),
                    DEFAULT_LOCALE,
                    &strong("zk-1"),
                    channel,
                );
                renders += 1;
                if h.text.contains(SECRET_TITLE) || h.text.to_lowercase().contains("nightfall") {
                    leaks += 1;
                }
                if h.text.contains("a restricted page") {
                    tombstones += 1;
                }
                assert!(
                    h.links.is_empty(),
                    "a denied KN unfurl yields no click-route link"
                );
            }
        }
        assert_eq!(
            leaks, 0,
            "NOTIF-D4-class: 0 title leak over {renders} denied KN renders (threshold 0)"
        );
        assert_eq!(
            tombstones, renders,
            "every denied render shows the PII-free `a restricted page` tombstone (the embed degrades)"
        );
        eprintln!(
            "NOTIF-D4 GREEN through KN resolve→humanise (2026-06-22): {renders} denied renders, \
             title-leak-count = {leaks} (threshold 0), tombstone = {tombstones}/{renders}"
        );
    }

    #[test]
    fn tombstone_reason_mapping_is_total_and_pii_free() {
        assert_eq!(
            map_tombstone_reason(KnTombstoneReason::Denied),
            TombstoneReason::Denied
        );
        assert_eq!(
            map_tombstone_reason(KnTombstoneReason::RootGone),
            TombstoneReason::RootGone
        );
        assert_eq!(
            map_tombstone_reason(KnTombstoneReason::SubGone),
            TombstoneReason::SubGone
        );
        assert_eq!(
            map_tombstone_reason(KnTombstoneReason::Erased),
            TombstoneReason::Erased
        );
    }

    #[test]
    fn an_erased_kn_subject_humanises_to_the_erased_display() {
        let page = secret_page();
        let mut store = PageStore::new();
        store.put_root(
            &page,
            PageMeta {
                title: SECRET_TITLE.into(),
                state: "published".into(),
            },
        );
        store.mark_erased(&page);
        let resolver =
            KnowledgeRefResolver::new(Projector::new(StubId::new().allow_read(&page), store));
        let h = humanise(
            &resolver,
            &acme(),
            &region(),
            &TemplateStore::with_platform_defaults(),
            reason_template_key(Reason::Mentioned),
            std::slice::from_ref(&page),
            &viewer("any"),
            DEFAULT_LOCALE,
            &strong("zk-1"),
            Channel::Cli,
        );
        assert!(
            h.text.contains("[erased user]"),
            "an erased KN subject renders the erased display"
        );
        assert!(
            !h.text.contains(SECRET_TITLE),
            "the erased subject's title never leaks"
        );
    }

    #[test]
    fn a_non_kn_ref_degrades_to_a_non_leaking_tombstone() {
        let (resolver, _) = confidential_page_resolver(false);
        let not_kn = ArtifactRef("myelin://acme/git/pr/9".into());
        let r = resolver.resolve_display(
            &acme(),
            &region(),
            &not_kn,
            &viewer("alice"),
            &strong("zk-1"),
        );
        match r {
            RefResolution::Tombstone(t) => assert_eq!(t.reason, TombstoneReason::RootGone),
            RefResolution::Projection(_) => panic!("a non-KN ref must not project a title"),
        }
    }
}
