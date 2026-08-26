use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_notif::humanise::{
    humanise, Channel as HumaniseChannel, RefProjection, RefResolution, RefResolvePort, Tombstone,
    TombstoneReason, DEFAULT_LOCALE,
};
use myelin_notif::{reason_template_key, Reason, TemplateStore};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};
use std::sync::Mutex;

use myelin_identity::{Consistency, ConsistencyMode, Zookie};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn viewer(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
}
fn strong(zk: &str) -> Consistency {
    Consistency {
        at_least: Zookie(zk.into()),
        mode: ConsistencyMode::Strong,
    }
}

const SUBJECT_ID: &str = "u-erase";

fn subject_actor_ref() -> ArtifactRef {
    ArtifactRef(format!("myelin://acme/identity/principal/{SUBJECT_ID}"))
}

struct ErasingResolver {
    erased_refs: Mutex<Vec<String>>,
}
impl ErasingResolver {
    fn new() -> ErasingResolver {
        ErasingResolver {
            erased_refs: Mutex::new(Vec::new()),
        }
    }
    fn mark_erased(&self, r: &ArtifactRef) {
        self.erased_refs.lock().unwrap().push(r.0.clone());
    }
}
impl RefResolvePort for ErasingResolver {
    fn resolve_display(
        &self,
        _t: &TenantId,
        _r: &Region,
        ref_: &ArtifactRef,
        _viewer: &Principal,
        _at: &Consistency,
    ) -> RefResolution {
        if self
            .erased_refs
            .lock()
            .unwrap()
            .iter()
            .any(|x| x == &ref_.0)
        {
            return RefResolution::Tombstone(Tombstone {
                root: ref_.clone(),
                reason: TombstoneReason::Erased,
            });
        }
        RefResolution::Projection(RefProjection {
            ref_: ref_.clone(),
            title: "issue PROJ-1".into(),
            icon: "issue".into(),
        })
    }
}

#[test]
fn every_notification_appearance_of_an_erased_actor_renders_as_a_tombstone() {
    let resolver = ErasingResolver::new();
    resolver.mark_erased(&subject_actor_ref());
    let templates = TemplateStore::with_platform_defaults();

    let appearances = [Reason::Mentioned, Reason::Replied, Reason::Assigned];
    for reason in appearances {
        let key = reason_template_key(reason);
        let h = humanise(
            &resolver,
            &tenant(),
            &region(),
            &templates,
            key,
            std::slice::from_ref(&subject_actor_ref()),
            &viewer("u-bob"),
            DEFAULT_LOCALE,
            &strong("z1"),
            HumaniseChannel::Cli,
        );
        assert!(
            h.text.contains("[erased user]"),
            "every appearance of the erased subject humanises to [erased user] (reason={key}): got {:?}",
            h.text
        );
        assert!(
            h.links.is_empty(),
            "an erased subject yields no link (reason={key})"
        );
    }
}
