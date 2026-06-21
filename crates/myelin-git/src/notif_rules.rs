//! # `notif_rules` — Git registers its Notif reasons + the watcher reverse index (GIT-P19 / P-263, M3)
//!
//! **Producer accretion (architecture `05-refined-shared-systems-architecture/notifications.md`
//! §3.1 / §3.5, C8).** This is the **Git half** of N-M3 "Producer accretion: Git + Knowledge
//! register their reasons + watchers". Git **registers** its `define_notif_rule` set and **declares**
//! its `watcher` ReBAC fragment; **Notif reads them, it never invents them**. The Knowledge half is
//! NOTIF-P20 (P-264); Issues / Chat / CI are M4 (NOTIF-P21 / P22 / P23); cross-cell is single-home
//! still (NOTIF-P24). **Named floors** (VISION §3) — see [the floors note](#named-floors).
//!
//! ## The inverse-signal property (EI-01 §1) — ZERO Notif code change
//! The load-bearing observation: Git registers its reasons + wires its watcher reverse index using
//! ONLY the **public, frozen** Notif seams —
//! [`myelin_notif::define_notif_rule`] (contract 7.6), [`myelin_notif::NotifRuleRegistry::register`],
//! and the [`myelin_notif::WatcherResolvePort`] trait (the read-fanout half of contract 4.3/4.10).
//! **No Notif enum variant was added, no Notif match arm, no Notif recompile** — the registration is
//! a *call into a data registry* and an *impl of a public trait*, both from THIS producer crate. If
//! accepting Git's set had required editing Notif, THIS MODULE COULD NOT COMPILE WITHOUT TOUCHING
//! `myelin-notif` — it does not (the seam is right; the compounding-payoff property is observable).
//! This mirrors the SAME accretion shape this crate already uses to register against Search
//! ([`crate::search_projection`], `declare_indexable`) and Refs ([`crate::subs`], `mint`).
//!
//! ## What Git registers (contract 7.6 — `review_requested` / `mentioned`)
//! Git's two notifiable Signal classes (architecture §1.3 Git **"Review requests"** view; §3.1 reason
//! table):
//! - **`review_requested`** — a `git.review.requested` event ([`crate::events::GIT_REVIEW_REQUESTED`])
//!   curated into a Signal: a reviewer was asked to review a PR. Reason
//!   [`myelin_notif::Reason::ReviewRequested`] → the `direct` ranking band (§3.1, `70/direct`).
//! - **`mentioned`** — a `mention(Principal)` structured node ([the frozen X-2 inline node]) inside a
//!   `git.comment.created` PR-comment body: a principal was @-mentioned in a PR thread. Reason
//!   [`myelin_notif::Reason::Mentioned`] → the `direct` band.
//!
//! Both are the **bounded DIRECT set** (write-fanout, §3.5) — a reviewer/mentionee is an explicit
//! target, not an ambient watcher. The `rule_key` each registers under is the curated Signal's
//! `rule_id` token (the `<rule>` segment of the `sig.<tenant>.<severity>.<rule>` subject the engine
//! publishes) — so the Notif router classifies a Git Signal BY its rule id through Git's registered
//! rule. These keys are the SAME ones the Git Signal-curation emitter (the GIT-P16 PR/review emit
//! follow-on) stamps; named as constants here so the producer + the router agree by NAME (X-5), never
//! a literal.
//!
//! ## What Git declares (contract 4.9 — the `watcher` ReBAC fragment, already frozen)
//! The `watcher` relation is declared on each watchable Git type in the frozen Git ReBAC namespace
//! fragment ([`crate::rebac_fragment`]) — `repo.watcher` and `pull_request.watcher`. This module does
//! NOT re-declare it (that fragment is frozen at GIT-P1); it **wires the read-fanout reverse index
//! over it**: [`GitWatcherIndex`] implements the frozen [`myelin_notif::WatcherResolvePort`] over
//! REAL Git PR/repo watcher tuples, so Notif's read-fanout ([`myelin_notif::read_fanout`]) materialises
//! a viewer's ambient Git slice over the **real watcher graph** — replacing the NOTIF-P13 synthetic
//! fixtures for Git subjects. The index answers the SAME `Filter{InRelation{watcher}}` push-down +
//! the SAME zookie watermark the production Identity reverse index serves (§3.5) — the JOIN lowering
//! and the held-not-leaked watermark gate are Notif's; Git only supplies the real watched-set + the
//! revision.
//!
//! ## NOTIF-D4 on a REAL Git subject (the leak gate, re-confirmed)
//! The Notif-side verification (`crates/myelin-notif/tests/` is Notif's; the Git-side re-confirmation
//! is `tests/drill_notif_d4_real_git_subject.rs` in THIS crate) drives [`myelin_notif::humanise`]
//! over a REAL Git private-repo subject (`myelin://<tenant>/git/pr/<n>` whose parent repo is private)
//! to a viewer lacking `pull` — and asserts the title NEVER appears (the humanised tombstone holds, 0
//! leak). That is the GIT-D8 cross-tenant-leak gate exercised through Notif's humanise path.
//!
//! ## <a name="named-floors"></a>Named floors (VISION §3)
//! - **Knowledge reasons + watchers → NOTIF-P20 (P-264).** Issues / Chat / CI → M4 (NOTIF-P21 / P22 /
//!   P23). Each accretes the SAME way (a `register` call + a `WatcherResolvePort` impl), no Notif edit.
//! - **Cross-cell is single-home (NOTIF-P24).** [`GitWatcherIndex`] resolves within ONE cell; the
//!   multi-cell ambient aggregation is N-M5.1. Named.
//! - **The LIVE watcher tuples + the production reverse index** are the Identity `list_subjects` /
//!   `authz_visible` reverse index served off the bus (the §3.5 per-tenant index). [`GitWatcherIndex`]
//!   is the in-process model of THAT index over Git's real watcher tuples (the watched-set is REAL —
//!   it is Git's PR/repo watcher graph — but the durable backing wires in with the Identity client in
//!   `serve`; the DECISION shape, one JOIN + the watermark, does not change). The Signal-curation
//!   EMITTER that turns a `git.review.requested` / `git.comment.created` into a curated Signal carrying
//!   the registered `rule_key` is the GIT-P16 emit follow-on (this module registers the rule the
//!   emitter's Signal classifies through; the emitter is not built here).

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

use crate::rebac_fragment::object_types;

// ===========================================================================
// Contract 7.6 — Git's define_notif_rule rule keys + the rule set (the registration)
// ===========================================================================

/// **The `rule_key` Git's curated `review_requested` Signal carries** (the `<rule>` segment of the
/// engine subject). The Notif router classifies a Git Signal under THIS key through Git's registered
/// [`NotifRule`]. Named so the Signal-curation emitter (GIT-P16) and the router agree by NAME (X-5).
pub const GIT_REVIEW_REQUESTED_RULE: &str = "git.review_requested";

/// **The `rule_key` Git's curated `mentioned`-in-a-PR Signal carries.** Classified through Git's
/// registered `mentioned` rule. A `mention(Principal)` STRUCTURED node in a PR-comment body — Notif
/// reads the structured node, it never parses free text (§3.5 / AG-6).
pub const GIT_MENTIONED_RULE: &str = "git.mentioned";

/// **The `rule_key` Git's curated ambient `watched`-PR Signal carries** (the read-fanout reason). An
/// ambient event (a push / comment / status) on a watched PR/repo. Reason
/// [`Reason::Watched`] → the `watching` band; resolved per-viewer over [`GitWatcherIndex`] at inbox
/// open (the read-fanout, never a write per watcher).
pub const GIT_WATCHED_RULE: &str = "git.watched";

/// **Git's complete `define_notif_rule` set (contract 7.6) — the registration value Git hands Notif.**
/// Each entry is `(rule_key, NotifRule)`; the [`NotifRule`] is built through the FROZEN
/// [`define_notif_rule`] verb (which reconciles the `default_class` against Notif's ONE §3.1 ranking
/// table — Git registers WHICH reason its Signal is; Notif's table owns the band). The dedup template
/// collapses repeat Git activity on the same subject into ONE inbox row (§3.2):
/// - `review_requested` → `git-review:{subject}` (re-requests on the same PR collapse).
/// - `mentioned`        → `git-mention:{recipient}:{subject}` (repeat mentions of a recipient on the
///   same PR thread collapse to one row with `coalesce_count = N`).
/// - `watched`          → `git-watched:{subject}` (ambient activity on a watched PR coalesces; the
///   per-viewer materialisation is the read-fanout, not a per-watcher write).
///
/// Returns a `Result` because `define_notif_rule` is a TOTAL verb that REJECTS a class disagreeing
/// with the §3.1 table loudly (never silently mis-banded) — Git's set is table-correct by
/// construction, so this is `Ok` in prod; the error is surfaced (not `unwrap`ped) so a future band
/// drift in Git's registration fails LOUDLY at boot, not silently in the inbox.
pub fn git_notif_rules(
) -> Result<Vec<(&'static str, NotifRule)>, myelin_notif::DefineRuleError> {
    Ok(vec![
        (
            GIT_REVIEW_REQUESTED_RULE,
            define_notif_rule(
                Reason::ReviewRequested,
                DedupTpl("git-review:{subject}".into()),
                Class::Direct,
            )?,
        ),
        (
            GIT_MENTIONED_RULE,
            define_notif_rule(
                Reason::Mentioned,
                DedupTpl("git-mention:{recipient}:{subject}".into()),
                Class::Direct,
            )?,
        ),
        (
            GIT_WATCHED_RULE,
            define_notif_rule(
                Reason::Watched,
                DedupTpl("git-watched:{subject}".into()),
                Class::Watching,
            )?,
        ),
    ])
}

/// **Register Git's `define_notif_rule` set into a [`NotifRuleRegistry`] (the inverse-signal seam,
/// EI-01 §1).** A `serve` boot path that wires the Notif router holds the ONE registry; Git (and
/// every other producer subsystem) calls THIS to register its set — a data insertion, ZERO Notif code
/// change. Returns `&mut` for fluent chaining with the other producers' registrations. Last-write-wins
/// on a re-registration (idempotent on a reconnect — the rule set is declarative). Surfaces a
/// [`myelin_notif::DefineRuleError`] if Git's set ever drifts off the §3.1 table (fails loudly).
pub fn register_git_notif_rules(
    registry: &mut NotifRuleRegistry,
) -> Result<&mut NotifRuleRegistry, myelin_notif::DefineRuleError> {
    for (key, rule) in git_notif_rules()? {
        registry.register(key, rule);
    }
    Ok(registry)
}

// ===========================================================================
// Contract 4.9 — the watcher relation (frozen in rebac_fragment) + the read-fanout reverse index
// ===========================================================================

/// **The frozen `watcher` relation NAME Git declares on each watchable type** (contract 4.9, C8). The
/// SAME constant Notif reads ([`myelin_notif::WATCHER_RELATION`]); re-exported here so the Git
/// producer + the Notif consumer agree by NAME (X-5), and the read-fanout JOIN resolves the SAME
/// relation Git's [`crate::rebac_fragment`] declares on `repo` + `pull_request`. Notif does not invent
/// it; Git declares it; this asserts the two halves use ONE name.
pub const GIT_WATCHER_RELATION: &str = myelin_notif::WATCHER_RELATION;

/// **The watchable Git object types that declare the `watcher` relation** (contract 4.9 — `repo` +
/// `pull_request`). These are the §5.2 watchable types Notif's read-fanout resolves a viewer's
/// ambient Git slice over; named here from the frozen [`crate::rebac_fragment::object_types`] so the
/// read-fanout index and the ReBAC fragment agree on WHICH types are watchable.
pub fn git_watchable_object_types() -> [&'static str; 2] {
    [object_types::REPO, object_types::PULL_REQUEST]
}

/// **A REAL Git watcher reverse index — the read-fanout over real PR/repo watcher tuples (contract
/// 4.3 / 4.10, §3.5).** The in-process model of the per-tenant `authz_visible` reverse index for the
/// Git `watcher` relation: per `(tenant, principal)` it holds the set of Git `subject_root`s that
/// principal WATCHES (a `pull_request.watcher` / `repo.watcher` tuple) + the current monotone
/// revision. It implements the FROZEN [`WatcherResolvePort`] so Notif's [`myelin_notif::read_fanout`]
/// materialises a viewer's ambient Git slice over THIS real graph — replacing the NOTIF-P13 synthetic
/// fixtures for Git subjects.
///
/// **Git supplies the real watched-set + the revision; Notif owns the JOIN lowering + the watermark
/// gate.** A `watch`/`unwatch` bumps the revision (a newer zookie) so a just-revoked Git watch is
/// reflected by a read at the new watermark (held, not leaked — the watermark gate is Notif's, in
/// `read_fanout`). The durable backing is the Identity reverse index off the bus (the named floor);
/// the watched-set itself is REAL (Git's PR/repo watcher tuples).
#[derive(Clone, Default)]
pub struct GitWatcherIndex {
    inner: Arc<Mutex<GitWatcherState>>,
}

#[derive(Default)]
struct GitWatcherState {
    /// Per-`(tenant, principal)`: the Git subject_roots that principal watches (real watcher tuples).
    watches: BTreeMap<(String, String), BTreeSet<String>>,
    /// The monotone revision (bumped on every watch/unwatch — the zookie watermark source).
    revision: u64,
    /// If set, the index reports UNAVAILABLE (an Identity hiccup — held-not-leaked is exercised).
    unavailable: bool,
}

impl GitWatcherIndex {
    /// A fresh, empty Git watcher reverse index at revision 0.
    pub fn new() -> GitWatcherIndex {
        GitWatcherIndex::default()
    }

    /// **`watch` — `principal` now watches the Git `subject_root` (a real `pull_request.watcher` /
    /// `repo.watcher` tuple write; bumps the revision).** The new revision is the watermark a
    /// subsequent strong read pins (a fresh watch reflected at-or-after it). Returns the new zookie
    /// (the `zk-<rev>` form Notif's read-fanout passes back as the watermark).
    pub fn watch(
        &self,
        tenant: &TenantId,
        principal: &str,
        subject_root: &str,
    ) -> Zookie {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.revision += 1;
        g.watches
            .entry((tenant.0.clone(), principal.to_string()))
            .or_default()
            .insert(subject_root.to_string());
        Zookie(format!("zk-{}", g.revision))
    }

    /// **`unwatch` — revoke `principal`'s watch on the Git `subject_root` (bumps the revision).** A
    /// read at the NEW watermark reflects the revocation: Notif's read-fanout JOINs the reverse index
    /// at-or-after the new revision, so the unwatched subject_root is absent from the reachable set
    /// (held, not leaked). Returns the new zookie (the watermark a strong read must honour).
    pub fn unwatch(
        &self,
        tenant: &TenantId,
        principal: &str,
        subject_root: &str,
    ) -> Zookie {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.revision += 1;
        if let Some(set) = g.watches.get_mut(&(tenant.0.clone(), principal.to_string())) {
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
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).unavailable = on;
    }
}

impl WatcherResolvePort for GitWatcherIndex {
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
                "git watcher reverse index unavailable (held, not leaked)".into(),
            ));
        }
        // The S8 PUSHED-DOWN path: the read-fanout lowers the watcher relation via the JOIN (the
        // density path Notif exercises) — return the Filter{InRelation{watcher}} the production
        // reverse index returns, stamped with the current revision's zookie. Notif's read-fanout
        // lowers the SetExpr; this index resolves the relational leaf below.
        Ok(ListObjectsResult::Filter {
            set_expr: SetExpr::InRelation {
                relation: RelName(GIT_WATCHER_RELATION.into()),
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
                "git watcher reverse index unavailable (held, not leaked)".into(),
            ));
        }
        // Only the Git `watcher` relation is served by THIS index (a different relation → empty —
        // never a widen). Both the InRelation + TupleSet forms resolve to the same real watched set.
        let watched = match leaf {
            RelationalLeaf::InRelation { relation, .. } if relation.0 == GIT_WATCHER_RELATION => g
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
        // below the required watermark (the held-not-leaked gate is Notif's, not Git's).
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

    /// **Git's `define_notif_rule` set is built through the FROZEN verb + reconciles against Notif's
    /// §3.1 table (the registration is table-correct).** Each rule's `default_class` is EXACTLY the
    /// band Notif's ranking table assigns the reason — Git registers the reason, Notif owns the band.
    #[test]
    fn git_rules_are_table_correct_review_mention_watched() {
        let rules = git_notif_rules().expect("git's set is table-correct by construction");
        // exactly the three Git Signal classes, keyed by the Git rule_id tokens.
        let keys: Vec<&str> = rules.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            keys,
            vec![GIT_REVIEW_REQUESTED_RULE, GIT_MENTIONED_RULE, GIT_WATCHED_RULE]
        );
        for (key, rule) in &rules {
            // the registered default_class is EXACTLY the §3.1 ranking-table band for the reason.
            assert_eq!(
                rule.default_class,
                reason_base_class(rule.reason).1,
                "rule `{key}` must register the §3.1 band for its reason"
            );
        }
        // the reasons are the frozen Git pair + the ambient read-fanout reason.
        assert_eq!(rules[0].1.reason, Reason::ReviewRequested);
        assert_eq!(rules[0].1.default_class, Class::Direct);
        assert_eq!(rules[1].1.reason, Reason::Mentioned);
        assert_eq!(rules[1].1.default_class, Class::Direct);
        assert_eq!(rules[2].1.reason, Reason::Watched);
        assert_eq!(rules[2].1.default_class, Class::Watching);
    }

    /// **THE INVERSE-SIGNAL PROPERTY (EI-01 §1): Git registers + the router classifies a Git Signal —
    /// with ZERO Notif code change.** Git uses ONLY the public seam; the platform-default registry
    /// accretes Git's set with no Notif edit, and a Signal carrying a Git rule_key classifies through
    /// Git's rule. If accepting Git's registration required a Notif change, this test could not compile
    /// without editing `myelin-notif` — it does not.
    #[test]
    fn git_registers_with_zero_notif_change() {
        let mut reg = NotifRuleRegistry::platform_default();
        let before = reg.len();
        register_git_notif_rules(&mut reg).expect("git's set registers");
        assert_eq!(reg.len(), before + 3, "git's three rules accreted (no Notif enum/match edit)");

        // the router classifies a Git review-requested Signal through Git's registered rule.
        let subject = myelin_refs::ArtifactRef("myelin://acme/git/pr/9".into());
        let c = reg.classify(GIT_REVIEW_REQUESTED_RULE, "psn:reviewer", &subject);
        assert_eq!(c.reason, Reason::ReviewRequested);
        assert_eq!(c.default_class, Class::Direct);
        assert!(c.from_registered_rule, "the Git registration took effect (0 Notif change)");
        assert_eq!(c.dedup_key, "git-review:myelin://acme/git/pr/9");

        // a Git mention Signal classifies + collapses by (recipient, subject).
        let m = reg.classify(GIT_MENTIONED_RULE, "psn:bob", &subject);
        assert_eq!(m.reason, Reason::Mentioned);
        assert_eq!(m.dedup_key, "git-mention:psn:bob:myelin://acme/git/pr/9");
    }

    /// **Re-registration is idempotent (last-write-wins) — a reconnecting Git declaring its set does
    /// not double it.** The router seam is declarative; registering twice keeps three rules.
    #[test]
    fn git_re_registration_is_idempotent() {
        let mut reg = NotifRuleRegistry::new();
        register_git_notif_rules(&mut reg).unwrap();
        register_git_notif_rules(&mut reg).unwrap();
        assert_eq!(reg.len(), 3, "re-registering Git's set keeps three rules (idempotent)");
    }

    /// **The watcher relation Git wires the reverse index over IS the frozen relation Git's ReBAC
    /// fragment declares (one name, X-5).** The read-fanout JOIN resolves `watcher`; the fragment
    /// declares `watcher` on `repo` + `pull_request`; this asserts they are the SAME name.
    #[test]
    fn git_watcher_relation_matches_the_frozen_fragment() {
        assert_eq!(GIT_WATCHER_RELATION, "watcher");
        // the read-fanout consumer reads the SAME name.
        assert_eq!(GIT_WATCHER_RELATION, myelin_notif::WATCHER_RELATION);
        // the watchable types are exactly the fragment's watchable Git types.
        assert_eq!(git_watchable_object_types(), ["repo", "pull_request"]);
        // and the fragment really declares `watcher` on both (the producer half).
        let repo_rels: Vec<String> = crate::rebac_fragment::repo_fragment()
            .relations
            .iter()
            .map(|r| r.0.clone())
            .collect();
        assert!(repo_rels.contains(&"watcher".to_string()));
        let pr_rels: Vec<String> = crate::rebac_fragment::pull_request_fragment()
            .relations
            .iter()
            .map(|r| r.0.clone())
            .collect();
        assert!(pr_rels.contains(&"watcher".to_string()));
    }

    fn viewer(id: &str) -> Principal {
        Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
    }

    /// **The Git watcher index serves the read-fanout push-down + resolves the real watched set.** A
    /// principal who watches a Git PR gets that subject_root back through the `InRelation{watcher}`
    /// leaf; a principal who watches nothing gets the empty set (never a widen).
    #[test]
    fn git_watcher_index_resolves_real_watched_prs() {
        let idx = GitWatcherIndex::new();
        let pr = "myelin://acme/git/pr/9";
        idx.watch(&tenant(), "psn:alice", pr);

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

        // resolving the watcher leaf returns alice's REAL watched PR.
        let leaf = RelationalLeaf::InRelation {
            relation: RelName("watcher".into()),
            via_column: myelin_notif::subject_root_col(),
        };
        let answer = idx
            .resolve_relation(&viewer("psn:alice"), &leaf, RevisionWatermark(0))
            .expect("available");
        assert!(answer.subject_roots.contains(pr), "alice watches the PR");

        // a principal who watches nothing → empty (never a widen).
        let none = idx
            .resolve_relation(&viewer("psn:nobody"), &leaf, RevisionWatermark(0))
            .expect("available");
        assert!(none.subject_roots.is_empty(), "a non-watcher reaches nothing");
    }

    /// **A different relation than `watcher` resolves to the empty set (never a widen).** The Git
    /// index serves ONLY the Git watcher relation; a stray relation leaf is deny-by-default.
    #[test]
    fn git_watcher_index_only_serves_the_watcher_relation() {
        let idx = GitWatcherIndex::new();
        idx.watch(&tenant(), "psn:alice", "myelin://acme/git/pr/9");
        let other = RelationalLeaf::InRelation {
            relation: RelName("reviewer".into()),
            via_column: myelin_notif::subject_root_col(),
        };
        let answer = idx
            .resolve_relation(&viewer("psn:alice"), &other, RevisionWatermark(0))
            .expect("available");
        assert!(answer.subject_roots.is_empty(), "a non-watcher relation reaches nothing (no widen)");
    }

    fn strong(zk: &str) -> Consistency {
        Consistency {
            at_least: Zookie(zk.into()),
            mode: myelin_identity::ConsistencyMode::Strong,
        }
    }
}
