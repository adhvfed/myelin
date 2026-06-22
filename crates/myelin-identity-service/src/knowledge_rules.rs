//! # `knowledge_rules` — Knowledge registers its Notif reasons + the watcher reverse index
//! (NOTIF-P20 / P-264, M3)
//!
//! **Producer accretion (architecture `05-refined-shared-systems-architecture/notifications.md`
//! §3.1 / §3.5, C8).** This is the **Knowledge half** of N-M3 "Producer accretion: Git + Knowledge
//! register their reasons + watchers". Knowledge **registers** its `define_notif_rule` set and **wires
//! the read-fanout reverse index** over the `watcher` ReBAC fragment it ALREADY declares
//! ([`crate::knowledge_fragment`], on `space` / `page` / `database_row`); **Notif reads them, it never
//! invents them**. The Git half shipped first (NOTIF-P19 / P-263, [`myelin_git::notif_rules`]);
//! Issues / Chat / CI are M4 (NOTIF-P21 / P22 / P23); cross-cell is single-home still (NOTIF-P24).
//! **Named floors** (VISION §3) — see [the floors note](#named-floors).
//!
//! ## The inverse-signal property (EI-01 §1) — ZERO Notif code change, the SECOND producer no harder
//! The load-bearing observation, re-confirmed for the SECOND producer: Knowledge registers its reasons
//! and wires its watcher reverse index using ONLY the **public, frozen** Notif seams —
//! [`myelin_notif::define_notif_rule`] (contract 7.6), [`myelin_notif::NotifRuleRegistry::register`],
//! and the [`myelin_notif::WatcherResolvePort`] trait (the read-fanout half of contract 4.3/4.10). **No
//! Notif enum variant was added, no Notif match arm, no Notif recompile** — exactly as the Git half
//! (and Issues / Chat) already accreted. The second producer needed **no more change than the first**:
//! the same `register` call + the same `WatcherResolvePort` impl shape. If accepting Knowledge's set
//! had required editing Notif, THIS MODULE COULD NOT COMPILE WITHOUT TOUCHING `myelin-notif` — it does
//! not (the seam is right; the compounding-payoff property is observable, EI-01 closing).
//!
//! ## Why this lives HERE (not in `myelin-content`)
//! Same DAG discipline as [`crate::knowledge_fragment`] (and the same reason Search's KN index specs
//! live in `myelin-search`, not content): `myelin-content` is the Knowledge data-model LEAF, and
//! `myelin-notif` already **depends on** `myelin-content` (its humanise path resolves KN block
//! content) — so a `content → notif` edge would be a **cycle** (§2.9 acyclic DAG). Id already owns
//! Knowledge's compiled authz content (the `watcher` relation on `space` / `page` / `database_row`,
//! contract 4.9), so the Notif registration of Knowledge's reasons + the read-fanout over THAT watcher
//! relation belongs alongside it. The CDC test
//! (`tests/cdc_7_6_4_9_knowledge_notif_rules.rs`) pins the two sides agree by NAME (X-5).
//!
//! ## What Knowledge registers (contract 7.6 — `mentions` / `comments` / `shares` / `watched`)
//! Knowledge's four notifiable Signal classes (architecture §1.3 KN activity; §3.1 reason table):
//! - **`mentioned`** — a `mention(Principal)` STRUCTURED node ([the frozen X-2 inline node]) inside a
//!   KN page/comment body: a principal was @-mentioned. Reason [`Reason::Mentioned`] → the `direct`
//!   band (§3.1, `70/direct`). Notif reads the structured node, it never parses free text (§3.5 / AG-6).
//! - **`comments`** — a `knowledge.comment.created` event curated into a Signal: a new comment on a
//!   page/row the recipient participates in. Reason [`Reason::Comments`] → the `participating` band
//!   (§3.1, `55/participating`).
//! - **`shared`** — a `knowledge.page.shared` event: a page/space was shared WITH the recipient (a
//!   direct address). Reason [`Reason::Shared`] → the `direct` band.
//! - **`watched`** — the ambient read-fanout reason: any activity (edit / comment / move) on a watched
//!   page/space/database_row. Reason [`Reason::Watched`] → the `watching` band (§3.1, `35/watching`);
//!   resolved per-viewer over [`KnowledgeWatcherIndex`] at inbox open (the read-fanout, never a write
//!   per watcher).
//!
//! `mentioned` / `shared` are the **bounded DIRECT set** (write-fanout, §3.5 — an explicit target);
//! `comments` is participation; `watched` is the ambient set the read-fanout materialises per-viewer.
//! The `rule_key` each registers under is the curated Signal's `rule_id` token (the `<rule>` segment of
//! the `sig.<tenant>.<severity>.<rule>` subject the engine publishes) — named as constants here so the
//! KN Signal-curation emitter (the KN-side emit follow-on) and the router agree by NAME (X-5), never a
//! literal.
//!
//! ## What Knowledge declares (contract 4.9 — the `watcher` ReBAC fragment, already frozen)
//! The `watcher` relation is declared on each watchable KN type in [`crate::knowledge_fragment`] —
//! `space.watcher`, `page.watcher`, `database_row.watcher` (NOT `block`: a block inherits its page's
//! ACL, so a watcher fans out at page granularity). This module does NOT re-declare it (that fragment
//! is frozen at P-249); it **wires the read-fanout reverse index over it**: [`KnowledgeWatcherIndex`]
//! implements the frozen [`myelin_notif::WatcherResolvePort`] over REAL KN page/space/row watcher
//! tuples, so Notif's read-fanout ([`myelin_notif::read_fanout`]) materialises a viewer's ambient KN
//! slice over the **real watcher graph** — replacing the NOTIF-P13 synthetic fixtures for KN subjects.
//! The index answers the SAME `Filter{InRelation{watcher}}` push-down + the SAME zookie watermark the
//! production Identity reverse index serves (§3.5) — the JOIN lowering and the held-not-leaked
//! watermark gate are Notif's; Knowledge only supplies the real watched-set + the revision.
//!
//! ## NOTIF-D4 on a REAL KN subject (the leak gate, re-confirmed)
//! The Knowledge-side re-confirmation (`tests/drill_notif_d4_kn_d5_d13_real_kn_subject.rs`) drives
//! [`myelin_notif::humanise`] over a REAL KN confidential page subject
//! (`myelin://<tenant>/knowledge/page/<n>` blocked by the `- direct_block` page-tree override) to a
//! viewer lacking `read` — and asserts the title NEVER appears (the humanised tombstone holds, 0 leak).
//! That is the KN-D5 / KN-D13 confidential-page/row/field no-leak (incl. COUNT) exercised through
//! Notif's humanise path.
//!
//! ## <a name="named-floors"></a>Named floors (VISION §3)
//! - **Issues / Chat / CI reasons + watchers → M4 (NOTIF-P21 / P22 / P23).** Each accretes the SAME way
//!   (a `register` call + a `WatcherResolvePort` impl), no Notif edit.
//! - **Cross-cell is single-home (NOTIF-P24).** [`KnowledgeWatcherIndex`] resolves within ONE cell; the
//!   multi-cell ambient aggregation is N-M5.1. Named.
//! - **The LIVE watcher tuples + the production reverse index** are the Identity `list_subjects` /
//!   `authz_visible` reverse index served off the bus (the §3.5 per-tenant index).
//!   [`KnowledgeWatcherIndex`] is the in-process model of THAT index over Knowledge's real watcher
//!   tuples (the watched-set is REAL — it is KN's page/space/row watcher graph — but the durable
//!   backing wires in with the Identity client in `serve`; the DECISION shape, one JOIN + the
//!   watermark, does not change). The KN Signal-curation EMITTER that turns a
//!   `knowledge.comment.created` / `knowledge.page.shared` into a curated Signal carrying the registered
//!   `rule_key` is the KN emit follow-on (this module registers the rule the emitter's Signal
//!   classifies through; the emitter is not built here).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use myelin_identity::{
    AuthzError, Consistency, ListObjectsResult, ObjectType, Permission, Principal, RelName,
    Result as AuthzResult, SetExpr, Zookie,
};
use myelin_notif::{
    define_notif_rule, Class, DedupTpl, NotifRule, NotifRuleRegistry, Reason, RelationalLeaf,
    ReverseIndexAnswer, RevisionWatermark, WatcherResolvePort,
};
use myelin_tenancy::TenantId;

use crate::knowledge_fragment::object_types;

// ===========================================================================
// Contract 7.6 — Knowledge's define_notif_rule rule keys + the rule set (the registration)
// ===========================================================================

/// **The `rule_key` Knowledge's curated `mentioned` Signal carries** (the `<rule>` segment of the
/// engine subject). A `mention(Principal)` STRUCTURED node in a KN page/comment body — Notif reads the
/// structured node, never free text (§3.5 / AG-6). Named so the KN Signal-curation emitter and the
/// router agree by NAME (X-5).
pub const KN_MENTIONED_RULE: &str = "knowledge.mentioned";

/// **The `rule_key` Knowledge's curated `comments` Signal carries** — a `knowledge.comment.created` on
/// a page/row the recipient participates in. Classified through KN's registered `comments` rule (the
/// `participating` band).
pub const KN_COMMENTS_RULE: &str = "knowledge.comments";

/// **The `rule_key` Knowledge's curated `shared` Signal carries** — a `knowledge.page.shared` (a page/
/// space shared WITH the recipient, a direct address). Classified through KN's registered `shared` rule
/// (the `direct` band).
pub const KN_SHARED_RULE: &str = "knowledge.shared";

/// **The `rule_key` Knowledge's curated ambient `watched` Signal carries** (the read-fanout reason). An
/// ambient event (an edit / comment / move) on a watched page/space/database_row. Reason
/// [`Reason::Watched`] → the `watching` band; resolved per-viewer over [`KnowledgeWatcherIndex`] at
/// inbox open (the read-fanout, never a write per watcher).
pub const KN_WATCHED_RULE: &str = "knowledge.watched";

/// **Knowledge's complete `define_notif_rule` set (contract 7.6) — the registration value KN hands
/// Notif.** Each entry is `(rule_key, NotifRule)`; the [`NotifRule`] is built through the FROZEN
/// [`define_notif_rule`] verb (which reconciles the `default_class` against Notif's ONE §3.1 ranking
/// table — KN registers WHICH reason its Signal is; Notif's table owns the band). The dedup template
/// collapses repeat KN activity on the same subject into ONE inbox row (§3.2):
/// - `mentioned` → `kn-mention:{recipient}:{subject}` (repeat mentions of a recipient on the same page
///   collapse to one row with `coalesce_count = N`).
/// - `comments`  → `kn-comments:{subject}` (a thread of comments on the same page coalesces to one row).
/// - `shared`    → `kn-shared:{recipient}:{subject}` (a re-share to the same recipient collapses).
/// - `watched`   → `kn-watched:{subject}` (ambient activity on a watched page coalesces; the per-viewer
///   materialisation is the read-fanout, not a per-watcher write).
///
/// Returns a `Result` because `define_notif_rule` is a TOTAL verb that REJECTS a class disagreeing with
/// the §3.1 table loudly (never silently mis-banded) — KN's set is table-correct by construction, so
/// this is `Ok` in prod; the error is surfaced (not `unwrap`ped) so a future band drift in KN's
/// registration fails LOUDLY at boot, not silently in the inbox.
pub fn knowledge_notif_rules(
) -> Result<Vec<(&'static str, NotifRule)>, myelin_notif::DefineRuleError> {
    Ok(vec![
        (
            KN_MENTIONED_RULE,
            define_notif_rule(
                Reason::Mentioned,
                DedupTpl("kn-mention:{recipient}:{subject}".into()),
                Class::Direct,
            )?,
        ),
        (
            KN_COMMENTS_RULE,
            define_notif_rule(
                Reason::Comments,
                DedupTpl("kn-comments:{subject}".into()),
                Class::Participating,
            )?,
        ),
        (
            KN_SHARED_RULE,
            define_notif_rule(
                Reason::Shared,
                DedupTpl("kn-shared:{recipient}:{subject}".into()),
                Class::Direct,
            )?,
        ),
        (
            KN_WATCHED_RULE,
            define_notif_rule(
                Reason::Watched,
                DedupTpl("kn-watched:{subject}".into()),
                Class::Watching,
            )?,
        ),
    ])
}

/// **Register Knowledge's `define_notif_rule` set into a [`NotifRuleRegistry`] (the inverse-signal
/// seam, EI-01 §1).** A `serve` boot path that wires the Notif router holds the ONE registry; Knowledge
/// (and every other producer subsystem) calls THIS to register its set — a data insertion, ZERO Notif
/// code change. Returns `&mut` for fluent chaining with the other producers' registrations (e.g.
/// alongside `myelin_git::notif_rules::register_git_notif_rules`). Last-write-wins on a re-registration
/// (idempotent on a reconnect — the rule set is declarative). Surfaces a
/// [`myelin_notif::DefineRuleError`] if KN's set ever drifts off the §3.1 table (fails loudly).
pub fn register_knowledge_notif_rules(
    registry: &mut NotifRuleRegistry,
) -> Result<&mut NotifRuleRegistry, myelin_notif::DefineRuleError> {
    for (key, rule) in knowledge_notif_rules()? {
        registry.register(key, rule);
    }
    Ok(registry)
}

// ===========================================================================
// Contract 4.9 — the watcher relation (frozen in knowledge_fragment) + the read-fanout reverse index
// ===========================================================================

/// **The frozen `watcher` relation NAME Knowledge declares on each watchable type** (contract 4.9, C8).
/// The SAME constant Notif reads ([`myelin_notif::WATCHER_RELATION`]); re-exported here so the Knowledge
/// producer + the Notif consumer agree by NAME (X-5), and the read-fanout JOIN resolves the SAME
/// relation [`crate::knowledge_fragment`] declares on `space` / `page` / `database_row`. Notif does not
/// invent it; Knowledge declares it; this asserts the two halves use ONE name.
pub const KN_WATCHER_RELATION: &str = myelin_notif::WATCHER_RELATION;

/// **The watchable Knowledge object types that declare the `watcher` relation** (contract 4.9 —
/// `space`, `page`, `database_row`; NOT `block`, which inherits its page's ACL so a watcher fans out at
/// page granularity). These are the watchable types Notif's read-fanout resolves a viewer's ambient KN slice
/// over; named here from the frozen [`crate::knowledge_fragment::object_types`] so the read-fanout index
/// and the ReBAC fragment agree on WHICH types are watchable.
pub fn knowledge_watchable_object_types() -> [&'static str; 3] {
    [
        object_types::SPACE,
        object_types::PAGE,
        object_types::DATABASE_ROW,
    ]
}

/// **A REAL Knowledge watcher reverse index — the read-fanout over real page/space/row watcher tuples
/// (contract 4.3 / 4.10, §3.5).** The in-process model of the per-tenant `authz_visible` reverse index
/// for the KN `watcher` relation: per `(tenant, principal)` it holds the set of KN `subject_root`s that
/// principal WATCHES (a `page.watcher` / `space.watcher` / `database_row.watcher` tuple) + the current
/// monotone revision. It implements the FROZEN [`WatcherResolvePort`] so Notif's
/// [`myelin_notif::read_fanout`] materialises a viewer's ambient KN slice over THIS real graph —
/// replacing the NOTIF-P13 synthetic fixtures for KN subjects.
///
/// **Knowledge supplies the real watched-set + the revision; Notif owns the JOIN lowering + the
/// watermark gate.** A `watch`/`unwatch` bumps the revision (a newer zookie) so a just-revoked KN watch
/// is reflected by a read at the new watermark (held, not leaked — the watermark gate is Notif's, in
/// `read_fanout`). The durable backing is the Identity reverse index off the bus (the named floor); the
/// watched-set itself is REAL (Knowledge's page/space/row watcher tuples). This is the SAME shape Git's
/// [`myelin_git::notif_rules::GitWatcherIndex`] wires — the second producer is no harder than the first.
#[derive(Clone, Default)]
pub struct KnowledgeWatcherIndex {
    inner: Arc<Mutex<KnowledgeWatcherState>>,
}

#[derive(Default)]
struct KnowledgeWatcherState {
    /// Per-`(tenant, principal)`: the KN subject_roots that principal watches (real watcher tuples).
    watches: BTreeMap<(String, String), BTreeSet<String>>,
    /// The monotone revision (bumped on every watch/unwatch — the zookie watermark source).
    revision: u64,
    /// If set, the index reports UNAVAILABLE (an Identity hiccup — held-not-leaked is exercised).
    unavailable: bool,
}

impl KnowledgeWatcherIndex {
    /// A fresh, empty Knowledge watcher reverse index at revision 0.
    pub fn new() -> KnowledgeWatcherIndex {
        KnowledgeWatcherIndex::default()
    }

    /// **`watch` — `principal` now watches the KN `subject_root` (a real `page.watcher` /
    /// `space.watcher` / `database_row.watcher` tuple write; bumps the revision).** The new revision is
    /// the watermark a subsequent strong read pins (a fresh watch reflected at-or-after it). Returns the
    /// new zookie (the `zk-<rev>` form Notif's read-fanout passes back as the watermark).
    pub fn watch(&self, tenant: &TenantId, principal: &str, subject_root: &str) -> Zookie {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.revision += 1;
        g.watches
            .entry((tenant.0.clone(), principal.to_string()))
            .or_default()
            .insert(subject_root.to_string());
        Zookie(format!("zk-{}", g.revision))
    }

    /// **`unwatch` — revoke `principal`'s watch on the KN `subject_root` (bumps the revision).** A read
    /// at the NEW watermark reflects the revocation: Notif's read-fanout JOINs the reverse index
    /// at-or-after the new revision, so the unwatched subject_root is absent from the reachable set
    /// (held, not leaked). Returns the new zookie (the watermark a strong read must honour).
    pub fn unwatch(&self, tenant: &TenantId, principal: &str, subject_root: &str) -> Zookie {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.revision += 1;
        if let Some(set) = g
            .watches
            .get_mut(&(tenant.0.clone(), principal.to_string()))
        {
            set.remove(subject_root);
        }
        Zookie(format!("zk-{}", g.revision))
    }

    /// The current monotone revision as a zookie (`zk-<rev>`) — the watermark source for a read.
    pub fn current_zookie(&self) -> Zookie {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        Zookie(format!("zk-{}", g.revision))
    }

    /// Make the index report UNAVAILABLE (an Identity hiccup) — exercises Notif's held-not-leaked path.
    pub fn set_unavailable(&self, on: bool) {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .unavailable = on;
    }
}

impl WatcherResolvePort for KnowledgeWatcherIndex {
    fn list_objects(
        &self,
        _subject: &Principal,
        _permission: &Permission,
        _ty: &ObjectType,
        _at: &Consistency,
    ) -> AuthzResult<ListObjectsResult> {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if g.unavailable {
            return Err(AuthzError::Unavailable(
                "knowledge watcher reverse index unavailable (held, not leaked)".into(),
            ));
        }
        // The S8 PUSHED-DOWN path: the read-fanout lowers the watcher relation via the JOIN (the
        // density path Notif exercises) — return the Filter{InRelation{watcher}} the production reverse
        // index returns, stamped with the current revision's zookie. Notif's read-fanout lowers the
        // SetExpr; this index resolves the relational leaf below.
        Ok(ListObjectsResult::Filter {
            set_expr: SetExpr::InRelation {
                relation: RelName(KN_WATCHER_RELATION.into()),
                via_column: myelin_notif::subject_root_col(),
            },
            zookie: Zookie(format!("zk-{}", g.revision)),
        })
    }

    fn resolve_relation(
        &self,
        subject: &Principal,
        leaf: &RelationalLeaf,
        _required: RevisionWatermark,
    ) -> AuthzResult<ReverseIndexAnswer> {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if g.unavailable {
            return Err(AuthzError::Unavailable(
                "knowledge watcher reverse index unavailable (held, not leaked)".into(),
            ));
        }
        // Only the KN `watcher` relation is served by THIS index (a different relation → empty — never
        // a widen). Both the InRelation + TupleSet forms resolve to the same real watched set.
        let watched = match leaf {
            RelationalLeaf::InRelation { relation, .. } if relation.0 == KN_WATCHER_RELATION => g
                .watches
                .get(&(subject.tenant.0.clone(), subject.principal_id.0.clone()))
                .cloned()
                .unwrap_or_default(),
            RelationalLeaf::TupleSet { .. } => g
                .watches
                .get(&(subject.tenant.0.clone(), subject.principal_id.0.clone()))
                .cloned()
                .unwrap_or_default(),
            _ => BTreeSet::new(),
        };
        // The index serves at the CURRENT revision; Notif's `resolve_leaf` rejects a served revision
        // below the required watermark (the held-not-leaked gate is Notif's, not Knowledge's).
        Ok(ReverseIndexAnswer {
            subject_roots: watched,
            revision: RevisionWatermark(g.revision),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalId, PrincipalKind};
    use myelin_notif::reason_base_class;

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }

    /// **Knowledge's `define_notif_rule` set is built through the FROZEN verb + reconciles against
    /// Notif's §3.1 table (the registration is table-correct).** Each rule's `default_class` is EXACTLY
    /// the band Notif's ranking table assigns the reason — Knowledge registers the reason, Notif owns
    /// the band.
    #[test]
    fn knowledge_rules_are_table_correct_mention_comments_shared_watched() {
        let rules = knowledge_notif_rules().expect("kn's set is table-correct by construction");
        // exactly the four KN Signal classes, keyed by the KN rule_id tokens.
        let keys: Vec<&str> = rules.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            keys,
            vec![
                KN_MENTIONED_RULE,
                KN_COMMENTS_RULE,
                KN_SHARED_RULE,
                KN_WATCHED_RULE
            ]
        );
        for (key, rule) in &rules {
            // the registered default_class is EXACTLY the §3.1 ranking-table band for the reason.
            assert_eq!(
                rule.default_class,
                reason_base_class(rule.reason).1,
                "rule `{key}` must register the §3.1 band for its reason"
            );
        }
        // the reasons are the frozen KN set + their §3.1 bands.
        assert_eq!(rules[0].1.reason, Reason::Mentioned);
        assert_eq!(rules[0].1.default_class, Class::Direct);
        assert_eq!(rules[1].1.reason, Reason::Comments);
        assert_eq!(rules[1].1.default_class, Class::Participating);
        assert_eq!(rules[2].1.reason, Reason::Shared);
        assert_eq!(rules[2].1.default_class, Class::Direct);
        assert_eq!(rules[3].1.reason, Reason::Watched);
        assert_eq!(rules[3].1.default_class, Class::Watching);
    }

    /// **THE INVERSE-SIGNAL PROPERTY (EI-01 §1): Knowledge registers + the router classifies a KN Signal
    /// — with ZERO Notif code change, the SECOND producer no harder than the first.** Knowledge uses
    /// ONLY the public seam; the platform-default registry accretes KN's set with no Notif edit, and a
    /// Signal carrying a KN rule_key classifies through KN's rule. If accepting KN's registration
    /// required a Notif change, this test could not compile without editing `myelin-notif` — it does not.
    #[test]
    fn knowledge_registers_with_zero_notif_change() {
        let mut reg = NotifRuleRegistry::platform_default();
        let before = reg.len();
        register_knowledge_notif_rules(&mut reg).expect("kn's set registers");
        assert_eq!(
            reg.len(),
            before + 4,
            "kn's four rules accreted (no Notif enum/match edit)"
        );

        // the router classifies a KN mention Signal through KN's registered rule + collapses by
        // (recipient, subject).
        let subject = myelin_refs::ArtifactRef("myelin://acme/knowledge/page/9".into());
        let m = reg.classify(KN_MENTIONED_RULE, "psn:bob", &subject);
        assert_eq!(m.reason, Reason::Mentioned);
        assert_eq!(m.default_class, Class::Direct);
        assert!(
            m.from_registered_rule,
            "the KN registration took effect (0 Notif change)"
        );
        assert_eq!(
            m.dedup_key,
            "kn-mention:psn:bob:myelin://acme/knowledge/page/9"
        );

        // a KN comments Signal classifies into the participating band + collapses by subject.
        let c = reg.classify(KN_COMMENTS_RULE, "psn:alice", &subject);
        assert_eq!(c.reason, Reason::Comments);
        assert_eq!(c.default_class, Class::Participating);
        assert_eq!(c.dedup_key, "kn-comments:myelin://acme/knowledge/page/9");

        // a KN watched Signal classifies into the ambient watching band (the read-fanout reason).
        let w = reg.classify(KN_WATCHED_RULE, "psn:carol", &subject);
        assert_eq!(w.reason, Reason::Watched);
        assert_eq!(w.default_class, Class::Watching);
    }

    /// **Re-registration is idempotent (last-write-wins) — a reconnecting Knowledge declaring its set
    /// does not double it.** The router seam is declarative; registering twice keeps four rules.
    #[test]
    fn knowledge_re_registration_is_idempotent() {
        let mut reg = NotifRuleRegistry::new();
        register_knowledge_notif_rules(&mut reg).unwrap();
        register_knowledge_notif_rules(&mut reg).unwrap();
        assert_eq!(
            reg.len(),
            4,
            "re-registering KN's set keeps four rules (idempotent)"
        );
    }

    /// **The watcher relation Knowledge wires the reverse index over IS the frozen relation KN's ReBAC
    /// fragment declares (one name, X-5).** The read-fanout JOIN resolves `watcher`; the fragment
    /// declares `watcher` on `space` / `page` / `database_row`; this asserts they are the SAME name.
    #[test]
    fn knowledge_watcher_relation_matches_the_frozen_fragment() {
        assert_eq!(KN_WATCHER_RELATION, "watcher");
        // the read-fanout consumer reads the SAME name.
        assert_eq!(KN_WATCHER_RELATION, myelin_notif::WATCHER_RELATION);
        // the watchable types are exactly the fragment's watchable KN types.
        assert_eq!(
            knowledge_watchable_object_types(),
            ["space", "page", "database_row"]
        );
        // and the fragment really declares `watcher` on all three (the producer half); block is NOT.
        assert!(
            crate::knowledge_fragment::space_fragment().is_watchable(),
            "space is watchable"
        );
        assert!(
            crate::knowledge_fragment::page_fragment().is_watchable(),
            "page is watchable"
        );
        assert!(
            crate::knowledge_fragment::database_row_fragment().is_watchable(),
            "database_row is watchable"
        );
        assert!(
            !crate::knowledge_fragment::block_fragment().is_watchable(),
            "block is NOT independently watchable (it inherits its page's ACL)"
        );
    }

    fn viewer(id: &str) -> Principal {
        Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
    }

    /// **The KN watcher index serves the read-fanout push-down + resolves the real watched set.** A
    /// principal who watches a KN page gets that subject_root back through the `InRelation{watcher}`
    /// leaf; a principal who watches nothing gets the empty set (never a widen).
    #[test]
    fn knowledge_watcher_index_resolves_real_watched_pages() {
        let idx = KnowledgeWatcherIndex::new();
        let page = "myelin://acme/knowledge/page/9";
        idx.watch(&tenant(), "psn:alice", page);

        // the list_objects push-down returns the Filter{InRelation{watcher}} the read-fanout lowers.
        let result = idx
            .list_objects(
                &viewer("psn:alice"),
                &Permission(myelin_notif::WATCH_PERMISSION.into()),
                &ObjectType(myelin_notif::SUBJECT_ROOT_TYPE.into()),
                &strong("zk-1"),
            )
            .expect("the index is available");
        match result {
            ListObjectsResult::Filter { set_expr, .. } => assert_eq!(
                set_expr,
                SetExpr::InRelation {
                    relation: RelName("watcher".into()),
                    via_column: myelin_notif::subject_root_col(),
                }
            ),
            other => panic!("expected the pushed-down Filter, got {other:?}"),
        }

        // resolving the watcher leaf returns alice's REAL watched page.
        let leaf = RelationalLeaf::InRelation {
            relation: RelName("watcher".into()),
            via_column: myelin_notif::subject_root_col(),
        };
        let answer = idx
            .resolve_relation(&viewer("psn:alice"), &leaf, RevisionWatermark(0))
            .expect("available");
        assert!(
            answer.subject_roots.contains(page),
            "alice watches the page"
        );

        // a principal who watches nothing → empty (never a widen).
        let none = idx
            .resolve_relation(&viewer("psn:nobody"), &leaf, RevisionWatermark(0))
            .expect("available");
        assert!(
            none.subject_roots.is_empty(),
            "a non-watcher reaches nothing"
        );
    }

    /// **A different relation than `watcher` resolves to the empty set (never a widen).** The KN index
    /// serves ONLY the KN watcher relation; a stray relation leaf is deny-by-default.
    #[test]
    fn knowledge_watcher_index_only_serves_the_watcher_relation() {
        let idx = KnowledgeWatcherIndex::new();
        idx.watch(&tenant(), "psn:alice", "myelin://acme/knowledge/page/9");
        let other = RelationalLeaf::InRelation {
            relation: RelName("direct_reader".into()),
            via_column: myelin_notif::subject_root_col(),
        };
        let answer = idx
            .resolve_relation(&viewer("psn:alice"), &other, RevisionWatermark(0))
            .expect("available");
        assert!(
            answer.subject_roots.is_empty(),
            "a non-watcher relation reaches nothing (no widen)"
        );
    }

    fn strong(zk: &str) -> Consistency {
        Consistency {
            at_least: Zookie(zk.into()),
            mode: myelin_identity::ConsistencyMode::Strong,
        }
    }
}
