//! # The CDC pair for contracts 7.6 + 4.9 — **Git's owned `define_notif_rule` set + the watcher
//! reverse index** (GIT-P19 / P-263, M3)
//!
//! **Contract-index rows.**
//! - **7.6** `define_notif_rule(reason, dedup_tpl, default_class)` — Signal class → inbox
//!   reason/priority; each subsystem registers its set. The Notif SEAM + the registration verb + the
//!   §3.1 ranking table is owned by Notif and frozen at NOTIF-P8 (`crates/myelin-notif/tests/
//!   cdc_7_6_notif_define_rule.rs`); THIS file pins the **Git slice** — the freeze GIT-P19 ships: the
//!   Git reason set (review_requested / mentioned / watched).
//! - **4.9** the per-subsystem ReBAC namespace fragment — the `watcher` relation per watchable type.
//!   Git declares `watcher` on `repo` + `pull_request` (frozen at GIT-P1, `crates/myelin-git/tests/
//!   cdc_4_9_git_fragment.rs`); THIS file pins the **read-fanout slice** — Notif's read-fanout
//!   resolves a viewer's ambient Git slice over Git's REAL watcher reverse index.
//!
//! (In contract-index terms the rule-set + fragment **PROVIDER**/producer is the subsystem — Git here
//! — and the **CONSUMER** is Notif's registry + read-fanout; the two markers below carry the
//! provider+consumer pair for the coverage scanner. This mirrors the Issues/Chat 7.6 slices already
//! shipped — Git accretes the SAME way, ZERO Notif change.)
//!
//! - the **PRODUCER** (the provider side) is **Git declaring its reason set + wiring its watcher
//!   reverse index at build time** ([`myelin_git::notif_rules::git_notif_rules`] +
//!   [`myelin_git::notif_rules::GitWatcherIndex`]) — the frozen Notif-owned
//!   [`myelin_notif::NotifRule`]s Git registers (each built via the frozen
//!   [`define_notif_rule`](myelin_notif::define_notif_rule) verb, so its `default_class` is RECONCILED
//!   against Notif's §3.1 table) + the REAL Git PR/repo watcher graph behind the frozen
//!   [`myelin_notif::WatcherResolvePort`]. The producer's promise: it registers exactly the Git
//!   reasons at their table-correct bands + serves ONLY the Git `watcher` relation, and NO second
//!   reason vocabulary / watcher-resolution path (EI-01 §7).
//! - the **CONSUMER** is **Notif admitting + routing + reading-fanout**: the
//!   [`NotifRuleRegistry`](myelin_notif::NotifRuleRegistry) `register`s each rule under its `rule_key`
//!   (the inverse-signal seam, ZERO Notif change) + `classify(rule_key, …)` routes a Signal through
//!   the registered Git rule, and [`read_fanout`](myelin_notif::read_fanout) lowers the Git index's
//!   `Filter{InRelation{watcher}}` push-down into the JOIN over Notif's own `subject_root` column.
//!
//! The two sides are pinned here so a drift on either (Git re-bands a reason / renames the watcher
//! relation; Notif renames a `NotifRule` field, changes the §3.1 table, or alters the read-fanout
//! lowering) fails this test in the same CI job. **The gate of GIT-P19 is the build-time registration
//! plus the real read-fanout** — Notif admits + routes Git's reasons and materialises a viewer's
//! ambient Git slice over the REAL watcher graph (replacing the NOTIF-P13 synthetic fixtures for Git
//! subjects). The Signal-curation EMITTER (a `git.review.requested` → curated Signal) is the GIT-P16
//! follow-on.

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

// ===========================================================================
// 7.6 — PROVIDER (Git declares its reason set) + CONSUMER (Notif admits + routes it)
// ===========================================================================

/// **PROVIDER side (7.6) — Git declares its reason set at table-correct bands.** Git's three rules are
/// built through the FROZEN `define_notif_rule` verb; each `default_class` is reconciled against
/// Notif's §3.1 table (Git registers WHICH reason; Notif owns the band). A drift (Git re-bands a
/// reason) fails the verb's reconciliation and this test.
#[test]
fn provider_git_declares_review_mention_watched_at_table_bands() {
    let rules = git_notif_rules().expect("git's set is table-correct");
    let by_key: std::collections::BTreeMap<&str, &myelin_notif::NotifRule> =
        rules.iter().map(|(k, r)| (*k, r)).collect();

    // review_requested → direct (the §3.1 70/direct band).
    let rr = by_key[GIT_REVIEW_REQUESTED_RULE];
    assert_eq!(rr.reason, Reason::ReviewRequested);
    assert_eq!(rr.default_class, Class::Direct);

    // mentioned → direct.
    let mn = by_key[GIT_MENTIONED_RULE];
    assert_eq!(mn.reason, Reason::Mentioned);
    assert_eq!(mn.default_class, Class::Direct);

    // watched → watching (the ambient read-fanout reason; §3.1 35/watching).
    let wt = by_key[GIT_WATCHED_RULE];
    assert_eq!(wt.reason, Reason::Watched);
    assert_eq!(wt.default_class, Class::Watching);
}

/// **CONSUMER side (7.6) — Notif's registry ADMITS + ROUTES Git's set (ZERO Notif change).** The
/// registry accepts Git's rules under their `rule_key`s and classifies a Git Signal through them. This
/// is the inverse-signal property: Git registered by CALLING the seam — no Notif enum/match/recompile.
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

    // a Git review-requested Signal routes through Git's registered rule → direct band, dedup key.
    let c = reg.classify(GIT_REVIEW_REQUESTED_RULE, "psn:reviewer", &pr);
    assert_eq!(c.reason, Reason::ReviewRequested);
    assert_eq!(c.default_class, Class::Direct);
    assert!(c.from_registered_rule, "Git's rule took effect");
    assert_eq!(c.dedup_key, "git-review:myelin://acme/git/pr/42");

    // a Git mention routes + collapses by (recipient, subject).
    let m = reg.classify(GIT_MENTIONED_RULE, "psn:bob", &pr);
    assert_eq!(m.reason, Reason::Mentioned);
    assert_eq!(m.dedup_key, "git-mention:psn:bob:myelin://acme/git/pr/42");
}

// ===========================================================================
// 4.9 — PROVIDER (Git declares the watcher relation + serves the index) +
//        CONSUMER (Notif's read-fanout lowers it into the JOIN over the real graph)
// ===========================================================================

/// **PROVIDER side (4.9) — Git declares the `watcher` relation on its watchable types + the read-fanout
/// index serves it.** The relation name the read-fanout JOIN resolves IS the name Git's frozen ReBAC
/// fragment declares (one name, X-5); the watchable types are `repo` + `pull_request`.
#[test]
fn provider_git_declares_watcher_on_watchable_types() {
    assert_eq!(GIT_WATCHER_RELATION, "watcher");
    assert_eq!(GIT_WATCHER_RELATION, myelin_notif::WATCHER_RELATION);
    assert_eq!(git_watchable_object_types(), ["repo", "pull_request"]);
    // the frozen fragment declares `watcher` on both watchable types (the producer half).
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

/// **CONSUMER side (4.9) — Notif's read-fanout materialises a viewer's ambient Git slice over Git's
/// REAL watcher graph.** The read-fanout lowers Git's `Filter{InRelation{watcher}}` push-down into the
/// JOIN over Notif's own `subject_root` column and projects the ONE coalesced marker per Git PR down to
/// the viewer's reachable set — REAL Git watchers, replacing the NOTIF-P13 synthetic fixtures.
#[test]
fn consumer_notif_read_fanout_over_real_git_watchers() {
    let idx = GitWatcherIndex::new();
    let watched_pr = ArtifactRef("myelin://acme/git/pr/9".into());
    let unwatched_pr = ArtifactRef("myelin://acme/git/pr/10".into());

    // alice watches PR-9 (a real pull_request.watcher tuple); bob watches nothing.
    let zk = idx.watch(&tenant(), "psn:alice", &watched_pr.0);

    // ONE coalesced ambient marker per Git PR (the read-fanout write side — zero per-watcher writes).
    let markers = AmbientMarkerStore::new();
    let origin = ArtifactRef("myelin://acme/bus/event/git-push".into());
    markers.record(&tenant(), &watched_pr, Reason::Watched, &origin);
    markers.record(&tenant(), &unwatched_pr, Reason::Watched, &origin);

    // alice's inbox open → she reaches her watched PR (the read-fanout JOIN over the real graph).
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

    // bob watches nothing → his ambient Git slice is empty (no leak of another's watched subject).
    let bob = read_fanout(&viewer("psn:bob"), &markers, &idx, &strong(&zk.0)).expect("resolves");
    assert!(
        bob.is_empty(),
        "a non-watcher reaches no ambient Git subject (held, not leaked)"
    );
}

/// **CONSUMER side (4.9 / 4.10) — a just-revoked Git watch is reflected (held, not leaked).** After
/// `unwatch`, a read at the NEW watermark drops the unwatched PR from alice's reachable set — the
/// watermark gate is Notif's, exercised over Git's real revision.
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

    // before revoke: alice reaches the PR.
    let before = read_fanout(
        &viewer("psn:alice"),
        &markers,
        &idx,
        &strong(&idx.current_zookie().0),
    )
    .expect("resolves");
    assert_eq!(before.len(), 1, "alice reaches the PR before revoke");

    // revoke alice's watch → a newer zookie (the watermark a strong read pins).
    let zk_after = idx.unwatch(&tenant(), "psn:alice", &pr.0);
    let after =
        read_fanout(&viewer("psn:alice"), &markers, &idx, &strong(&zk_after.0)).expect("resolves");
    assert!(
        after.is_empty(),
        "the just-revoked Git watch is reflected (held, not leaked)"
    );
}

/// **CONSUMER side — an unavailable Git index → held, not leaked (the ambient slice is withheld, never
/// widened).** An Identity hiccup on the Git reverse index returns the proven-empty ambient set, never
/// a leak.
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
    // held, not leaked: the read-fanout returns an Unavailable error (the proven set is withheld),
    // never a silent widen of the ambient Git slice.
    assert!(
        res.is_err(),
        "an unavailable Git index holds, never leaks the ambient slice"
    );
}
