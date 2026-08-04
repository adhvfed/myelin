use myelin_identity::{
    Consistency, ConsistencyMode, Principal, PrincipalId, PrincipalKind, Zookie,
};
use myelin_identity_service::knowledge_rules::{
    knowledge_notif_rules, knowledge_watchable_object_types, register_knowledge_notif_rules,
    KnowledgeWatcherIndex, KN_COMMENTS_RULE, KN_MENTIONED_RULE, KN_SHARED_RULE, KN_WATCHED_RULE,
    KN_WATCHER_RELATION,
};
use myelin_notif::read_fanout::{read_fanout, AmbientMarkerStore};
use myelin_notif::{Class, NotifRuleRegistry, Reason};
use myelin_refs::ArtifactRef;
use myelin_tenancy::TenantId;

fn tenant() -> TenantId {
    TenantId("acme".into())
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

#[test]
fn provider_knowledge_declares_mention_comments_shared_watched_at_table_bands() {
    let rules = knowledge_notif_rules().expect("kn's set is table-correct");
    let by_key: std::collections::BTreeMap<&str, &myelin_notif::NotifRule> =
        rules.iter().map(|(k, r)| (*k, r)).collect();

    let mn = by_key[KN_MENTIONED_RULE];
    assert_eq!(mn.reason, Reason::Mentioned);
    assert_eq!(mn.default_class, Class::Direct);

    let cm = by_key[KN_COMMENTS_RULE];
    assert_eq!(cm.reason, Reason::Comments);
    assert_eq!(cm.default_class, Class::Participating);

    let sh = by_key[KN_SHARED_RULE];
    assert_eq!(sh.reason, Reason::Shared);
    assert_eq!(sh.default_class, Class::Direct);

    let wt = by_key[KN_WATCHED_RULE];
    assert_eq!(wt.reason, Reason::Watched);
    assert_eq!(wt.default_class, Class::Watching);
}

#[test]
fn consumer_notif_admits_and_routes_knowledge_rules() {
    let mut reg = NotifRuleRegistry::platform_default();
    let before = reg.len();
    register_knowledge_notif_rules(&mut reg).expect("Notif admits KN's set");
    assert_eq!(
        reg.len(),
        before + 4,
        "Notif admitted KN's four rules (zero Notif change)"
    );

    let page = ArtifactRef("myelin://acme/knowledge/page/42".into());

    let m = reg.classify(KN_MENTIONED_RULE, "psn:bob", &page);
    assert_eq!(m.reason, Reason::Mentioned);
    assert_eq!(m.default_class, Class::Direct);
    assert!(m.from_registered_rule, "KN's rule took effect");
    assert_eq!(
        m.dedup_key,
        "kn-mention:psn:bob:myelin://acme/knowledge/page/42"
    );

    let c = reg.classify(KN_COMMENTS_RULE, "psn:alice", &page);
    assert_eq!(c.reason, Reason::Comments);
    assert_eq!(c.default_class, Class::Participating);
    assert_eq!(c.dedup_key, "kn-comments:myelin://acme/knowledge/page/42");

    let s = reg.classify(KN_SHARED_RULE, "psn:carol", &page);
    assert_eq!(s.reason, Reason::Shared);
    assert_eq!(
        s.dedup_key,
        "kn-shared:psn:carol:myelin://acme/knowledge/page/42"
    );
}

#[test]
fn provider_knowledge_declares_watcher_on_watchable_types() {
    assert_eq!(KN_WATCHER_RELATION, "watcher");
    assert_eq!(KN_WATCHER_RELATION, myelin_notif::WATCHER_RELATION);
    assert_eq!(
        knowledge_watchable_object_types(),
        ["space", "page", "database_row"]
    );
    use myelin_identity_service::knowledge_fragment;
    assert!(
        knowledge_fragment::space_fragment().is_watchable(),
        "space declares watcher"
    );
    assert!(
        knowledge_fragment::page_fragment().is_watchable(),
        "page declares watcher"
    );
    assert!(
        knowledge_fragment::database_row_fragment().is_watchable(),
        "database_row declares watcher"
    );
    assert!(
        !knowledge_fragment::block_fragment().is_watchable(),
        "block is not watchable"
    );
}

#[test]
fn consumer_notif_read_fanout_over_real_knowledge_watchers() {
    let idx = KnowledgeWatcherIndex::new();
    let watched_page = ArtifactRef("myelin://acme/knowledge/page/9".into());
    let unwatched_page = ArtifactRef("myelin://acme/knowledge/page/10".into());

    let zk = idx.watch(&tenant(), "psn:alice", &watched_page.0);

    let markers = AmbientMarkerStore::new();
    let origin = ArtifactRef("myelin://acme/bus/event/kn-edit".into());
    markers.record(&tenant(), &watched_page, Reason::Watched, &origin);
    markers.record(&tenant(), &unwatched_page, Reason::Watched, &origin);

    let alice = read_fanout(&viewer("psn:alice"), &markers, &idx, &strong(&zk.0))
        .expect("the real KN watcher index resolves");
    let alice_roots: Vec<&str> = alice.iter().map(|m| m.subject_root.as_str()).collect();
    assert!(
        alice_roots.contains(&watched_page.0.as_str()),
        "alice reaches her watched page-9"
    );
    assert!(
        !alice_roots.contains(&unwatched_page.0.as_str()),
        "alice does not reach the unwatched page-10"
    );

    let bob = read_fanout(&viewer("psn:bob"), &markers, &idx, &strong(&zk.0)).expect("resolves");
    assert!(
        bob.is_empty(),
        "a non-watcher reaches no ambient KN subject (held, not leaked)"
    );
}

#[test]
fn consumer_notif_read_fanout_reflects_a_revoked_knowledge_watch() {
    let idx = KnowledgeWatcherIndex::new();
    let page = ArtifactRef("myelin://acme/knowledge/page/9".into());
    idx.watch(&tenant(), "psn:alice", &page.0);

    let markers = AmbientMarkerStore::new();
    markers.record(
        &tenant(),
        &page,
        Reason::Watched,
        &ArtifactRef("myelin://acme/bus/event/e".into()),
    );

    let before = read_fanout(
        &viewer("psn:alice"),
        &markers,
        &idx,
        &strong(&idx.current_zookie().0),
    )
    .expect("resolves");
    assert_eq!(before.len(), 1, "alice reaches the page before revoke");

    let zk_after = idx.unwatch(&tenant(), "psn:alice", &page.0);
    let after =
        read_fanout(&viewer("psn:alice"), &markers, &idx, &strong(&zk_after.0)).expect("resolves");
    assert!(
        after.is_empty(),
        "the just-revoked KN watch is reflected (held, not leaked)"
    );
}

#[test]
fn consumer_notif_holds_not_leaks_on_unavailable_knowledge_index() {
    let idx = KnowledgeWatcherIndex::new();
    idx.watch(&tenant(), "psn:alice", "myelin://acme/knowledge/page/9");
    idx.set_unavailable(true);

    let markers = AmbientMarkerStore::new();
    markers.record(
        &tenant(),
        &ArtifactRef("myelin://acme/knowledge/page/9".into()),
        Reason::Watched,
        &ArtifactRef("myelin://acme/bus/event/e".into()),
    );

    let res = read_fanout(&viewer("psn:alice"), &markers, &idx, &strong("zk-1"));
    assert!(
        res.is_err(),
        "an unavailable KN index holds, never leaks the ambient slice"
    );
}
