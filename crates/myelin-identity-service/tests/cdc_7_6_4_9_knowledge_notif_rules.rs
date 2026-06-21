//! # The CDC pair for contracts 7.6 + 4.9 — **Knowledge's owned `define_notif_rule` set + the watcher
//! reverse index** (NOTIF-P20 / P-264, M3)
//!
//! **Contract-index rows.**
//! - **7.6** `define_notif_rule(reason, dedup_tpl, default_class)` — Signal class → inbox
//!   reason/priority; each subsystem registers its set. The Notif SEAM + the registration verb + the
//!   §3.1 ranking table is owned by Notif and frozen at NOTIF-P8 (`crates/myelin-notif/tests/
//!   cdc_7_6_notif_define_rule.rs`); THIS file pins the **Knowledge slice** — the freeze NOTIF-P20
//!   ships: the KN reason set (mentioned / comments / shared / watched).
//! - **4.9** the per-subsystem ReBAC namespace fragment — the `watcher` relation per watchable type.
//!   Knowledge declares `watcher` on `space` + `page` + `database_row` (frozen at P-249,
//!   `crates/myelin-identity-service/tests/cdc_4_9_knowledge_fragment.rs`); THIS file pins the
//!   **read-fanout slice** — Notif's read-fanout resolves a viewer's ambient KN slice over Knowledge's
//!   REAL watcher reverse index.
//!
//! (In contract-index terms the rule-set + fragment **PROVIDER**/producer is the subsystem — Knowledge
//! here — and the **CONSUMER** is Notif's registry + read-fanout; the two markers below carry the
//! provider+consumer pair for the coverage scanner. This mirrors the Git/Issues/Chat 7.6 slices already
//! shipped — Knowledge accretes the SAME way, ZERO Notif change; the SECOND producer no harder than the
//! first.)
//!
//! - the **PRODUCER** (the provider side) is **Knowledge declaring its reason set + wiring its watcher
//!   reverse index at build time** (the `knowledge_notif_rules` set
//!   plus [`myelin_identity_service::knowledge_rules::KnowledgeWatcherIndex`]) — the frozen Notif-owned
//!   [`myelin_notif::NotifRule`]s KN registers (each built via the frozen
//!   [`define_notif_rule`](myelin_notif::define_notif_rule) verb, so its `default_class` is RECONCILED
//!   against Notif's §3.1 table) + the REAL KN page/space/row watcher graph behind the frozen
//!   [`myelin_notif::WatcherResolvePort`]. The producer's promise: it registers exactly the KN reasons
//!   at their table-correct bands + serves ONLY the KN `watcher` relation, and NO second reason
//!   vocabulary / watcher-resolution path (EI-01 §7).
//! - the **CONSUMER** is **Notif admitting + routing + reading-fanout**: the
//!   [`NotifRuleRegistry`](myelin_notif::NotifRuleRegistry) `register`s each rule under its `rule_key`
//!   (the inverse-signal seam, ZERO Notif change) + `classify(rule_key, …)` routes a Signal through the
//!   registered KN rule, and [`read_fanout`](myelin_notif::read_fanout) lowers the KN index's
//!   `Filter{InRelation{watcher}}` push-down into the JOIN over Notif's own `subject_root` column.
//!
//! The two sides are pinned here so a drift on either (KN re-bands a reason / renames the watcher
//! relation; Notif renames a `NotifRule` field, changes the §3.1 table, or alters the read-fanout
//! lowering) fails this test in the same CI job. **The gate of NOTIF-P20 is the build-time registration
//! plus the real read-fanout** — Notif admits + routes KN's reasons and materialises a viewer's ambient
//! KN slice over the REAL watcher graph (replacing the NOTIF-P13 synthetic fixtures for KN subjects).
//! The KN Signal-curation EMITTER (a `knowledge.comment.created` → curated Signal) is the KN emit
//! follow-on.

use myelin_identity::{Consistency, ConsistencyMode, Principal, PrincipalId, PrincipalKind, Zookie};
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
    Consistency { at_least: Zookie(zk.into()), mode: ConsistencyMode::Strong }
}

// ===========================================================================
// 7.6 — PROVIDER (Knowledge declares its reason set) + CONSUMER (Notif admits + routes it)
// ===========================================================================

/// **PROVIDER side (7.6) — Knowledge declares its reason set at table-correct bands.** KN's four rules
/// are built through the FROZEN `define_notif_rule` verb; each `default_class` is reconciled against
/// Notif's §3.1 table (KN registers WHICH reason; Notif owns the band). A drift (KN re-bands a reason)
/// fails the verb's reconciliation and this test.
#[test]
fn provider_knowledge_declares_mention_comments_shared_watched_at_table_bands() {
    let rules = knowledge_notif_rules().expect("kn's set is table-correct");
    let by_key: std::collections::BTreeMap<&str, &myelin_notif::NotifRule> =
        rules.iter().map(|(k, r)| (*k, r)).collect();

    // mentioned → direct (the §3.1 70/direct band).
    let mn = by_key[KN_MENTIONED_RULE];
    assert_eq!(mn.reason, Reason::Mentioned);
    assert_eq!(mn.default_class, Class::Direct);

    // comments → participating (the §3.1 55/participating band).
    let cm = by_key[KN_COMMENTS_RULE];
    assert_eq!(cm.reason, Reason::Comments);
    assert_eq!(cm.default_class, Class::Participating);

    // shared → direct (a direct address of the recipient).
    let sh = by_key[KN_SHARED_RULE];
    assert_eq!(sh.reason, Reason::Shared);
    assert_eq!(sh.default_class, Class::Direct);

    // watched → watching (the ambient read-fanout reason; §3.1 35/watching).
    let wt = by_key[KN_WATCHED_RULE];
    assert_eq!(wt.reason, Reason::Watched);
    assert_eq!(wt.default_class, Class::Watching);
}

/// **CONSUMER side (7.6) — Notif's registry ADMITS + ROUTES Knowledge's set (ZERO Notif change).** The
/// registry accepts KN's rules under their `rule_key`s and classifies a KN Signal through them. This is
/// the inverse-signal property: Knowledge registered by CALLING the seam — no Notif enum/match/recompile
/// — and the SECOND producer was no harder than the first (the same `register` call as Git/Issues/Chat).
#[test]
fn consumer_notif_admits_and_routes_knowledge_rules() {
    let mut reg = NotifRuleRegistry::platform_default();
    let before = reg.len();
    register_knowledge_notif_rules(&mut reg).expect("Notif admits KN's set");
    assert_eq!(reg.len(), before + 4, "Notif admitted KN's four rules (zero Notif change)");

    let page = ArtifactRef("myelin://acme/knowledge/page/42".into());

    // a KN mention Signal routes through KN's registered rule → direct band, dedup key.
    let m = reg.classify(KN_MENTIONED_RULE, "psn:bob", &page);
    assert_eq!(m.reason, Reason::Mentioned);
    assert_eq!(m.default_class, Class::Direct);
    assert!(m.from_registered_rule, "KN's rule took effect");
    assert_eq!(m.dedup_key, "kn-mention:psn:bob:myelin://acme/knowledge/page/42");

    // a KN comments Signal routes into the participating band + collapses by subject.
    let c = reg.classify(KN_COMMENTS_RULE, "psn:alice", &page);
    assert_eq!(c.reason, Reason::Comments);
    assert_eq!(c.default_class, Class::Participating);
    assert_eq!(c.dedup_key, "kn-comments:myelin://acme/knowledge/page/42");

    // a KN shared Signal routes into the direct band + collapses by (recipient, subject).
    let s = reg.classify(KN_SHARED_RULE, "psn:carol", &page);
    assert_eq!(s.reason, Reason::Shared);
    assert_eq!(s.dedup_key, "kn-shared:psn:carol:myelin://acme/knowledge/page/42");
}

// ===========================================================================
// 4.9 — PROVIDER (Knowledge declares the watcher relation + serves the index) +
//        CONSUMER (Notif's read-fanout lowers it into the JOIN over the real graph)
// ===========================================================================

/// **PROVIDER side (4.9) — Knowledge declares the `watcher` relation on its watchable types + the
/// read-fanout index serves it.** The relation name the read-fanout JOIN resolves IS the name KN's
/// frozen ReBAC fragment declares (one name, X-5); the watchable types are `space` + `page` +
/// `database_row` (NOT `block`).
#[test]
fn provider_knowledge_declares_watcher_on_watchable_types() {
    assert_eq!(KN_WATCHER_RELATION, "watcher");
    assert_eq!(KN_WATCHER_RELATION, myelin_notif::WATCHER_RELATION);
    assert_eq!(knowledge_watchable_object_types(), ["space", "page", "database_row"]);
    // the frozen fragment declares `watcher` on the three watchable types (the producer half).
    use myelin_identity_service::knowledge_fragment;
    assert!(knowledge_fragment::space_fragment().is_watchable(), "space declares watcher");
    assert!(knowledge_fragment::page_fragment().is_watchable(), "page declares watcher");
    assert!(
        knowledge_fragment::database_row_fragment().is_watchable(),
        "database_row declares watcher"
    );
    // block is NOT independently watchable (it inherits its page's ACL; a watcher fans out at page
    // granularity).
    assert!(!knowledge_fragment::block_fragment().is_watchable(), "block is not watchable");
}

/// **CONSUMER side (4.9) — Notif's read-fanout materialises a viewer's ambient KN slice over Knowledge's
/// REAL watcher graph.** The read-fanout lowers KN's `Filter{InRelation{watcher}}` push-down into the
/// JOIN over Notif's own `subject_root` column and projects the ONE coalesced marker per KN page down to
/// the viewer's reachable set — REAL KN watchers, replacing the NOTIF-P13 synthetic fixtures.
#[test]
fn consumer_notif_read_fanout_over_real_knowledge_watchers() {
    let idx = KnowledgeWatcherIndex::new();
    let watched_page = ArtifactRef("myelin://acme/knowledge/page/9".into());
    let unwatched_page = ArtifactRef("myelin://acme/knowledge/page/10".into());

    // alice watches page-9 (a real page.watcher tuple); bob watches nothing.
    let zk = idx.watch(&tenant(), "psn:alice", &watched_page.0);

    // ONE coalesced ambient marker per KN page (the read-fanout write side — zero per-watcher writes).
    let markers = AmbientMarkerStore::new();
    let origin = ArtifactRef("myelin://acme/bus/event/kn-edit".into());
    markers.record(&tenant(), &watched_page, Reason::Watched, &origin);
    markers.record(&tenant(), &unwatched_page, Reason::Watched, &origin);

    // alice's inbox open → she reaches her watched page (the read-fanout JOIN over the real graph).
    let alice = read_fanout(&viewer("psn:alice"), &markers, &idx, &strong(&zk.0))
        .expect("the real KN watcher index resolves");
    let alice_roots: Vec<&str> = alice.iter().map(|m| m.subject_root.as_str()).collect();
    assert!(alice_roots.contains(&watched_page.0.as_str()), "alice reaches her watched page-9");
    assert!(
        !alice_roots.contains(&unwatched_page.0.as_str()),
        "alice does not reach the unwatched page-10"
    );

    // bob watches nothing → his ambient KN slice is empty (no leak of another's watched subject).
    let bob = read_fanout(&viewer("psn:bob"), &markers, &idx, &strong(&zk.0)).expect("resolves");
    assert!(bob.is_empty(), "a non-watcher reaches no ambient KN subject (held, not leaked)");
}

/// **CONSUMER side (4.9 / 4.10) — a just-revoked KN watch is reflected (held, not leaked).** After
/// `unwatch`, a read at the NEW watermark drops the unwatched page from alice's reachable set — the
/// watermark gate is Notif's, exercised over KN's real revision.
#[test]
fn consumer_notif_read_fanout_reflects_a_revoked_knowledge_watch() {
    let idx = KnowledgeWatcherIndex::new();
    let page = ArtifactRef("myelin://acme/knowledge/page/9".into());
    idx.watch(&tenant(), "psn:alice", &page.0);

    let markers = AmbientMarkerStore::new();
    markers.record(&tenant(), &page, Reason::Watched, &ArtifactRef("myelin://acme/bus/event/e".into()));

    // before revoke: alice reaches the page.
    let before = read_fanout(&viewer("psn:alice"), &markers, &idx, &strong(&idx.current_zookie().0))
        .expect("resolves");
    assert_eq!(before.len(), 1, "alice reaches the page before revoke");

    // revoke alice's watch → a newer zookie (the watermark a strong read pins).
    let zk_after = idx.unwatch(&tenant(), "psn:alice", &page.0);
    let after = read_fanout(&viewer("psn:alice"), &markers, &idx, &strong(&zk_after.0))
        .expect("resolves");
    assert!(after.is_empty(), "the just-revoked KN watch is reflected (held, not leaked)");
}

/// **CONSUMER side — an unavailable KN index → held, not leaked (the ambient slice is withheld, never
/// widened).** An Identity hiccup on the KN reverse index returns the proven-empty ambient set, never a
/// leak.
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
    // held, not leaked: the read-fanout returns an Unavailable error (the proven set is withheld),
    // never a silent widen of the ambient KN slice.
    assert!(res.is_err(), "an unavailable KN index holds, never leaks the ambient slice");
}
