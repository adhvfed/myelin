use myelin_git::notif_rules::{
    git_notif_rules, git_watchable_object_types, register_git_notif_rules, GitWatcherIndex,
    GIT_MENTIONED_RULE, GIT_REVIEW_REQUESTED_RULE, GIT_WATCHED_RULE, GIT_WATCHER_RELATION,
};
use myelin_identity::{
    Consistency, ConsistencyMode, Principal, PrincipalId, PrincipalKind, Zookie,
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
fn provider_git_declares_review_mention_watched_at_table_bands() {
    let rules = git_notif_rules().expect("git's set is table-correct");
    let by_key: std::collections::BTreeMap<&str, &myelin_notif::NotifRule> =
        rules.iter().map(|(k, r)| (*k, r)).collect();

    let rr = by_key[GIT_REVIEW_REQUESTED_RULE];
    assert_eq!(rr.reason, Reason::ReviewRequested);
    assert_eq!(rr.default_class, Class::Direct);

    let mn = by_key[GIT_MENTIONED_RULE];
    assert_eq!(mn.reason, Reason::Mentioned);
    assert_eq!(mn.default_class, Class::Direct);

    let wt = by_key[GIT_WATCHED_RULE];
    assert_eq!(wt.reason, Reason::Watched);
    assert_eq!(wt.default_class, Class::Watching);
}

#[test]
fn consumer_notif_admits_and_routes_git_rules() {
    let mut reg = NotifRuleRegistry::platform_default();
    let before = reg.len();
    register_git_notif_rules(&mut reg).expect("Notif admits Git's set");
    assert_eq!(
        reg.len(),
        before + 3,
        "Notif admitted Git's three rules (zero Notif change)"
    );

    let pr = ArtifactRef("myelin://acme/git/pr/42".into());

    let c = reg.classify(GIT_REVIEW_REQUESTED_RULE, "psn:reviewer", &pr);
    assert_eq!(c.reason, Reason::ReviewRequested);
    assert_eq!(c.default_class, Class::Direct);
    assert!(c.from_registered_rule, "Git's rule took effect");
    assert_eq!(c.dedup_key, "git-review:myelin://acme/git/pr/42");

    let m = reg.classify(GIT_MENTIONED_RULE, "psn:bob", &pr);
    assert_eq!(m.reason, Reason::Mentioned);
    assert_eq!(m.dedup_key, "git-mention:psn:bob:myelin://acme/git/pr/42");
}

#[test]
fn provider_git_declares_watcher_on_watchable_types() {
    assert_eq!(GIT_WATCHER_RELATION, "watcher");
    assert_eq!(GIT_WATCHER_RELATION, myelin_notif::WATCHER_RELATION);
    assert_eq!(git_watchable_object_types(), ["repo", "pull_request"]);
    for frag in [
        myelin_git::rebac_fragment::repo_fragment(),
        myelin_git::rebac_fragment::pull_request_fragment(),
    ] {
        assert!(
            frag.relations.iter().any(|r| r.0 == "watcher"),
            "{} declares the watcher relation",
            frag.object_type.0
        );
    }
}

#[test]
fn consumer_notif_read_fanout_over_real_git_watchers() {
    let idx = GitWatcherIndex::new();
    let watched_pr = ArtifactRef("myelin://acme/git/pr/9".into());
    let unwatched_pr = ArtifactRef("myelin://acme/git/pr/10".into());

    let zk = idx.watch(&tenant(), "psn:alice", &watched_pr.0);

    let markers = AmbientMarkerStore::new();
    let origin = ArtifactRef("myelin://acme/bus/event/git-push".into());
    markers.record(&tenant(), &watched_pr, Reason::Watched, &origin);
    markers.record(&tenant(), &unwatched_pr, Reason::Watched, &origin);

    let alice = read_fanout(&viewer("psn:alice"), &markers, &idx, &strong(&zk.0))
        .expect("the real Git watcher index resolves");
    let alice_roots: Vec<&str> = alice.iter().map(|m| m.subject_root.as_str()).collect();
    assert!(
        alice_roots.contains(&watched_pr.0.as_str()),
        "alice reaches her watched PR-9"
    );
    assert!(
        !alice_roots.contains(&unwatched_pr.0.as_str()),
        "alice does not reach the unwatched PR-10"
    );

    let bob = read_fanout(&viewer("psn:bob"), &markers, &idx, &strong(&zk.0)).expect("resolves");
    assert!(
        bob.is_empty(),
        "a non-watcher reaches no ambient Git subject (held, not leaked)"
    );
}

#[test]
fn consumer_notif_read_fanout_reflects_a_revoked_git_watch() {
    let idx = GitWatcherIndex::new();
    let pr = ArtifactRef("myelin://acme/git/pr/9".into());
    idx.watch(&tenant(), "psn:alice", &pr.0);

    let markers = AmbientMarkerStore::new();
    markers.record(
        &tenant(),
        &pr,
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
    assert_eq!(before.len(), 1, "alice reaches the PR before revoke");

    let zk_after = idx.unwatch(&tenant(), "psn:alice", &pr.0);
    let after =
        read_fanout(&viewer("psn:alice"), &markers, &idx, &strong(&zk_after.0)).expect("resolves");
    assert!(
        after.is_empty(),
        "the just-revoked Git watch is reflected (held, not leaked)"
    );
}

#[test]
fn consumer_notif_holds_not_leaks_on_unavailable_git_index() {
    let idx = GitWatcherIndex::new();
    idx.watch(&tenant(), "psn:alice", "myelin://acme/git/pr/9");
    idx.set_unavailable(true);

    let markers = AmbientMarkerStore::new();
    markers.record(
        &tenant(),
        &ArtifactRef("myelin://acme/git/pr/9".into()),
        Reason::Watched,
        &ArtifactRef("myelin://acme/bus/event/e".into()),
    );

    let res = read_fanout(&viewer("psn:alice"), &markers, &idx, &strong("zk-1"));
    assert!(
        res.is_err(),
        "an unavailable Git index holds, never leaks the ambient slice"
    );
}
