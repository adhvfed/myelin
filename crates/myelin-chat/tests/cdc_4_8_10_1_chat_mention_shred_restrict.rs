use myelin_chat::{
    agent_may_read, analytics_eligible, index_projection_if_allowed, notif_may_route,
    paragraph_body, render_mention, MentionRender, MentionResolver, ReadPath, RestrictionFlag,
    RestrictionGate, ERASED_USER,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::TenantId;
use std::collections::BTreeSet;

fn principal(id: &str) -> Principal {
    Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}
struct PseudonymMap {
    live: BTreeSet<String>,
}
impl PseudonymMap {
    fn with(ids: &[&str]) -> PseudonymMap {
        PseudonymMap {
            live: ids.iter().map(|s| s.to_string()).collect(),
        }
    }
    fn erase(&mut self, id: &str) {
        self.live.remove(id);
    }
}
impl MentionResolver for PseudonymMap {
    fn resolve_display_name(&self, mentioned: &Principal) -> Option<String> {
        self.live
            .contains(&mentioned.principal_id.0)
            .then(|| format!("@{}", mentioned.principal_id.0))
    }
}

#[test]
fn cdc_4_8_provider_consumer_mention_shreds_to_erased_user() {
    let mut map = PseudonymMap::with(&["psn:ada"]);
    let ada = principal("psn:ada");

    assert_eq!(
        render_mention(&ada, &map),
        MentionRender::Live("@psn:ada".into())
    );

    map.erase("psn:ada");

    let after = render_mention(&ada, &map);
    assert_eq!(after, MentionRender::Erased);
    assert_eq!(after.display(), ERASED_USER);
}

#[test]
fn cdc_4_8_mention_shred_is_free_body_never_rewritten() {
    let mut map = PseudonymMap::with(&["psn:ada", "psn:bo"]);
    let ada = principal("psn:ada");
    let bo = principal("psn:bo");

    map.erase("psn:ada");

    assert_eq!(render_mention(&ada, &map), MentionRender::Erased);
    assert_eq!(
        render_mention(&bo, &map),
        MentionRender::Live("@psn:bo".into())
    );
}

#[test]
fn cdc_10_1_provider_consumer_restrict_suppresses_every_read_path() {
    let restrictions = RestrictionFlag::new();
    let gate = RestrictionGate::new(restrictions.clone());
    let body = paragraph_body("a message", vec![]);

    assert!(index_projection_if_allowed(&gate, "psn:ada", &body, None).is_some());
    assert!(agent_may_read(&gate, "psn:ada"));
    assert!(notif_may_route(&gate, "psn:ada"));
    assert!(analytics_eligible(&gate, "psn:ada"));

    restrictions.set("psn:ada", true);

    assert!(
        index_projection_if_allowed(&gate, "psn:ada", &body, None).is_none(),
        "indexing suppressed"
    );
    assert!(!agent_may_read(&gate, "psn:ada"), "agent-use suppressed");
    assert!(
        !notif_may_route(&gate, "psn:ada"),
        "notif-routing suppressed"
    );
    assert!(
        !analytics_eligible(&gate, "psn:ada"),
        "analytics suppressed"
    );
    assert!(
        gate.suppressed_everywhere("psn:ada"),
        "the restricted subject is suppressed across ALL read paths (Art. 18 totality)"
    );

    restrictions.set("psn:ada", false);
    assert!(index_projection_if_allowed(&gate, "psn:ada", &body, None).is_some());
    assert!(!gate.suppressed_everywhere("psn:ada"));
}

#[test]
fn cdc_10_1_restriction_is_per_subject() {
    let restrictions = RestrictionFlag::new();
    let gate = RestrictionGate::new(restrictions.clone());
    restrictions.set("psn:ada", true);
    assert!(gate.suppressed_everywhere("psn:ada"));
    for path in ReadPath::ALL {
        assert!(
            gate.may_process("psn:bo", path),
            "{}: bo unaffected",
            path.label()
        );
    }
}
