//! # `sla_escalation` — Issues passes its REAL SLA escalation chain + wires the watcher read-fanout
//! (NOTIF-P21 / P-342, M4)
//!
//! **Consumer accretion (architecture
//! `05-refined-shared-systems-architecture/notifications.md` §2.4 / §3.7, C3).** This is the **Issues
//! half** of N-M4 "Consumer accretion: Issues SLA/escalation + …". Issues **passes its REAL SLA
//! escalation chain definition** to the FROZEN NOTIF-P14 durable-workflow machinery (replacing the
//! Notif-defined [`EscalationPolicy::test_chain`] floor), **wires its `watcher` read-fanout reverse
//! index** (the 4.9 relation its frozen ReBAC fragment declares), and — together with the FULL reason
//! set in [`crate::declares`] — makes Issues **"My Work"** a real filtered view (the C-9 invariant: a
//! FILTER over the ONE inbox, never a second store). Chat is NOTIF-P22, CI is NOTIF-P23; cross-cell is
//! single-home still (NOTIF-P24); surge/erasure hardening is NOTIF-P25/P26/P27. **Named floors** below.
//!
//! ## The inverse-signal property (EI-01 §1) — ZERO Notif code change
//! Issues passes its chain + wires its watcher index using ONLY the **public, frozen** Notif seams —
//! [`EscalationPolicy`] / [`EscalationStep`] / [`EscalationTarget`] (the frozen C3 chain shape,
//! contract 7.5) and the [`WatcherResolvePort`] trait (the read-fanout half of contract 4.3/4.10).
//! **No Notif enum variant was added, no Notif match arm, no Notif recompile** — the chain is a *value
//! Issues constructs* and hands the engine, and the watcher index is an *impl of a public trait*, both
//! from THIS consumer crate. If passing Issues' real chain had required editing Notif, THIS MODULE
//! COULD NOT COMPILE WITHOUT TOUCHING `myelin-notif` — it does not (the seam is right; registration did
//! not get harder than the prior consumers' — the inverse-signal). This mirrors the SAME accretion
//! shape Git ([`myelin_git::notif_rules`]) and Knowledge
//! ([`myelin_identity_service::knowledge_rules`]) already use to wire their watcher indexes.
//!
//! ## What Issues passes — the REAL SLA escalation chain (contract 7.5 / §2.4)
//! [`issue_sla_escalation_policy`] is the chain DEFINITION Issues' SLA policy hands Notif's
//! [`EscalationEngine::page`] when an SLA breach fires ([`crate::events::SLA_BREACHED`]). It is the
//! frozen §2.4 shape — an ordered list of steps, each `page → oncall_now(schedule) → notify(critical,
//! pierces quiet-hours) → escalate-after-timer(ack_window)` — realised over Issues' REAL escalation
//! tiers (the assignee's team on-call first, then the project on-call lead, then the org incident lead),
//! with the chain looping `repeat` times before exhaustion. **Notif owns the POLICY evaluation + the
//! durable timer wheel; Issues owns only the chain DEFINITION** (which tiers, which ack windows). The
//! chain carries no PII (targets are opaque schedule/principal selectors, channels are channel kinds).
//!
//! ## What Issues wires — the `watcher` read-fanout reverse index (contract 4.9, frozen at ISS-P01)
//! The `watcher` relation is already declared on the `issue` type in Issues' frozen ReBAC namespace
//! fragment ([`crate::rebac_fragment`], 4.9, C8) — this module does NOT re-declare it; it **wires the
//! read-fanout reverse index over it**: [`IssueWatcherIndex`] implements the frozen
//! [`WatcherResolvePort`] over REAL Issues watcher tuples, so Notif's read-fanout
//! ([`myelin_notif::read_fanout`]) materialises a viewer's ambient Issues slice (the `watched`
//! reason rows in "My Work") over the **real watcher graph** — replacing the NOTIF-P13 synthetic
//! fixtures for Issues subjects. Issues supplies the real watched-set + the revision; Notif owns the
//! JOIN lowering + the held-not-leaked watermark gate.
//!
//! ## ISS-D6 — an SLA breach starts the escalation chain (the drill)
//! The drill (`tests/drill_iss_d6_sla_escalation.rs`) drives a REAL SLA breach through Issues' REAL
//! chain on Notif's durable wheel: the chain STARTS (the first page reaches the on-call AT FIRE TIME),
//! WALKS per the frozen shape across a kill mid-`ack_window` (the durable timer resumes), pages the
//! next tier EXACTLY ONCE (0 missed, 0 duplicate — inherits NOTIF-D7's exactly-once property under
//! Issues' real chain), and an ack HALTS it (idempotent — one `notif.escalation.acked` event).
//!
//! ## Named floors (VISION §3)
//! - **Chat reasons + explicit-first + HITL cards → NOTIF-P22 (P-343); CI status-summary reasons →
//!   NOTIF-P23 (P-344).** Each accretes the SAME way (a chain/rule definition + an optional
//!   `WatcherResolvePort` impl), no Notif edit.
//! - **Cross-cell is single-home (NOTIF-P24).** [`IssueWatcherIndex`] resolves within ONE cell; the
//!   multi-cell ambient aggregation is N-M5.1. Named.
//! - **Surge / erasure hardening → NOTIF-P25/P26/P27.** Named.
//! - **The LIVE durable wheel** the chain arms its `ack_window` timers on is the `myelin-flow`
//!   minute-bucket wheel (P-FLOW-09/P-FLOW-13) behind Notif's [`DurableWheel`] seam — the SAME named
//!   floor NOTIF-P14 carries; here the in-memory model proves the chain-walk POLICY over Issues' real
//!   chain. The **live watcher tuples + the production reverse index** are the Identity
//!   `list_subjects` / `authz_visible` reverse index off the bus; [`IssueWatcherIndex`] is the
//!   in-process model of THAT index over Issues' real watcher tuples (the watched-set is REAL).
//! - **The Signal-curation EMITTER** that turns an `issue.sla.breached` into a curated Signal carrying
//!   the registered `rule_key` + STARTS the escalation `page` is the ISS-P22 "My Work" wiring follow-on
//!   (this module supplies the chain the emitter starts; the live signal route is not built here).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use myelin_identity::{
    AuthzError, Consistency, ListObjectsResult, ObjectType, Permission, Principal, RelName,
    Result as AuthzResult, SetExpr, Zookie,
};
use myelin_notif::{
    EscalationPolicy, EscalationStep, EscalationTarget, PrefChannel as EscalationChannel,
    RelationalLeaf, ReverseIndexAnswer, RevisionWatermark, WatcherResolvePort,
};
use myelin_tenancy::TenantId;

use crate::rebac_fragment::object_types;

// ===========================================================================
// Contract 7.5 / §2.4 — Issues' REAL SLA escalation chain (the definition Issues passes Notif)
// ===========================================================================

/// **The opaque on-call schedule selector for the assignee's TEAM tier** (the first escalation tier —
/// the issue's owning team on-call). Resolved through [`myelin_notif::oncall_now`] AT FIRE TIME, so
/// the page reaches whoever is on call for that team WHEN the breach escalates, not who was on call
/// when the SLA policy was authored. A selector, not PII (the roster is the on-call schedule's).
pub const SLA_TEAM_ONCALL_SCHEDULE: &str = "issue-sla-team-oncall";

/// **The opaque on-call schedule selector for the PROJECT lead tier** (the second escalation tier —
/// the project's on-call lead, escalated to if the team tier does not ack within its window).
pub const SLA_PROJECT_ONCALL_SCHEDULE: &str = "issue-sla-project-oncall";

/// **The opaque on-call schedule selector for the ORG incident-lead tier** (the third / final
/// escalation tier — the org incident lead, escalated to if the project tier does not ack).
pub const SLA_ORG_ONCALL_SCHEDULE: &str = "issue-sla-org-oncall";

/// **The stable policy id of Issues' REAL SLA escalation chain** (the `escalation_policy` row id Notif
/// keys an `escalation_run` to). Distinct from the Notif-defined `esc-test-chain` floor — this is the
/// real Issues policy that replaces it at NOTIF-P21.
pub const SLA_ESCALATION_POLICY_ID: &str = "issue-sla-escalation";

/// **Issues' REAL SLA escalation chain definition (contract 7.5 / §2.4 — the C3 frozen shape).** The
/// ordered three-tier chain Issues' SLA policy passes to Notif's [`EscalationEngine::page`] when an
/// SLA breach fires: the assignee's **team on-call** first → escalate after `ack_window_minutes` to
/// the **project lead** → escalate again to the **org incident lead**; the whole chain loops `repeat`
/// times before exhaustion. Each step is the frozen `page → oncall_now → notify(critical) →
/// escalate-after-timer` shape; every escalation step is `Class::Critical` (an on-call page pierces
/// quiet-hours — you cannot silence a breached SLA). `repeat` is clamped to `≥ 1` by the engine
/// ([`EscalationPolicy::step_at`]); pass `repeat = 1` for a single walk, `2+` to re-page the chain.
///
/// **Notif owns the POLICY evaluation + the durability; Issues owns only this DEFINITION.** The
/// `ack_window_minutes` is the durable-timer wait Notif arms on the `myelin-flow` wheel between tiers
/// (a real durable timer, not an in-process sleep). The chain carries no PII — the tiers are opaque
/// schedule selectors resolved at fire time, the channels are channel kinds.
pub fn issue_sla_escalation_policy(ack_window_minutes: u32, repeat: u32) -> EscalationPolicy {
    let channels = vec![EscalationChannel::InApp, EscalationChannel::WebPush];
    EscalationPolicy {
        policy_id: SLA_ESCALATION_POLICY_ID.to_string(),
        steps: vec![
            // Tier 1: the assignee's TEAM on-call (resolved at fire time).
            EscalationStep {
                target: EscalationTarget::Schedule(SLA_TEAM_ONCALL_SCHEDULE.to_string()),
                channels: channels.clone(),
                ack_window_minutes,
            },
            // Tier 2: the PROJECT lead on-call (escalated to if tier 1 does not ack).
            EscalationStep {
                target: EscalationTarget::Schedule(SLA_PROJECT_ONCALL_SCHEDULE.to_string()),
                channels: channels.clone(),
                ack_window_minutes,
            },
            // Tier 3: the ORG incident lead on-call (the final tier).
            EscalationStep {
                target: EscalationTarget::Schedule(SLA_ORG_ONCALL_SCHEDULE.to_string()),
                channels,
                ack_window_minutes,
            },
        ],
        repeat: repeat.max(1),
    }
}

// ===========================================================================
// Contract 4.9 — the watcher relation (frozen in rebac_fragment) + the read-fanout reverse index
// ===========================================================================

/// **The frozen `watcher` relation NAME Issues declares on the watchable `issue` type** (contract 4.9,
/// C8). The SAME constant Notif reads ([`myelin_notif::WATCHER_RELATION`]); re-exported here so the
/// Issues producer + the Notif consumer agree by NAME (X-5), and the read-fanout JOIN resolves the
/// SAME relation Issues' [`crate::rebac_fragment`] declares on `issue`. Notif does not invent it;
/// Issues declares it; this asserts the two halves use ONE name.
pub const ISSUE_WATCHER_RELATION: &str = myelin_notif::WATCHER_RELATION;

/// **The watchable Issues object type that declares the `watcher` relation** (contract 4.9 — `issue`).
/// The §1.3 "My Work" `watched` reason rows are resolved per-viewer over this type's read-fanout;
/// named here from the frozen [`object_types::ISSUE`] so the read-fanout index and the ReBAC fragment
/// agree on WHICH type is watchable.
pub fn issue_watchable_object_type() -> &'static str {
    object_types::ISSUE
}

/// **A REAL Issues watcher reverse index — the read-fanout over real issue watcher tuples (contract
/// 4.3 / 4.10, §3.5).** The in-process model of the per-tenant `authz_visible` reverse index for the
/// Issues `watcher` relation: per `(tenant, principal)` it holds the set of Issue `subject_root`s that
/// principal WATCHES (an `issue.watcher` tuple) + the current monotone revision. It implements the
/// FROZEN [`WatcherResolvePort`] so Notif's [`myelin_notif::read_fanout`] materialises a viewer's
/// ambient Issues slice (the `watched` reason rows in "My Work") over THIS real graph — replacing the
/// NOTIF-P13 synthetic fixtures for Issues subjects.
///
/// **Issues supplies the real watched-set + the revision; Notif owns the JOIN lowering + the watermark
/// gate.** A `watch`/`unwatch` bumps the revision (a newer zookie) so a just-revoked Issue watch is
/// reflected by a read at the new watermark (held, not leaked — the watermark gate is Notif's, in
/// `read_fanout`). The durable backing is the Identity reverse index off the bus (the named floor);
/// the watched-set itself is REAL (Issues' issue watcher tuples). This is the SAME shape Git's
/// [`myelin_git::notif_rules::GitWatcherIndex`] and Knowledge's `KnowledgeWatcherIndex` wire — the
/// third consumer is no harder than the first (the inverse-signal).
#[derive(Clone, Default)]
pub struct IssueWatcherIndex {
    inner: Arc<Mutex<IssueWatcherState>>,
}

#[derive(Default)]
struct IssueWatcherState {
    /// Per-`(tenant, principal)`: the Issue subject_roots that principal watches (real watcher tuples).
    watches: BTreeMap<(String, String), BTreeSet<String>>,
    /// The monotone revision (bumped on every watch/unwatch — the zookie watermark source).
    revision: u64,
    /// If set, the index reports UNAVAILABLE (an Identity hiccup — held-not-leaked is exercised).
    unavailable: bool,
}

impl IssueWatcherIndex {
    /// A fresh, empty Issues watcher reverse index at revision 0.
    pub fn new() -> IssueWatcherIndex {
        IssueWatcherIndex::default()
    }

    /// **`watch` — `principal` now watches the Issue `subject_root` (a real `issue.watcher` tuple
    /// write; bumps the revision).** The new revision is the watermark a subsequent strong read pins
    /// (a fresh watch reflected at-or-after it). Returns the new zookie (`zk-<rev>`).
    pub fn watch(&self, tenant: &TenantId, principal: &str, subject_root: &str) -> Zookie {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.revision += 1;
        g.watches
            .entry((tenant.0.clone(), principal.to_string()))
            .or_default()
            .insert(subject_root.to_string());
        Zookie(format!("zk-{}", g.revision))
    }

    /// **`unwatch` — revoke `principal`'s watch on the Issue `subject_root` (bumps the revision).** A
    /// read at the NEW watermark reflects the revocation: Notif's read-fanout JOINs the reverse index
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

impl WatcherResolvePort for IssueWatcherIndex {
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
                "issue watcher reverse index unavailable (held, not leaked)".into(),
            ));
        }
        // The S8 PUSHED-DOWN path: the read-fanout lowers the watcher relation via the JOIN (the
        // density path Notif exercises) — return the Filter{InRelation{watcher}} the production reverse
        // index returns, stamped with the current revision's zookie. Notif's read-fanout lowers the
        // SetExpr; this index resolves the relational leaf below.
        Ok(ListObjectsResult::Filter {
            set_expr: SetExpr::InRelation {
                relation: RelName(ISSUE_WATCHER_RELATION.into()),
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
                "issue watcher reverse index unavailable (held, not leaked)".into(),
            ));
        }
        // Only the Issues `watcher` relation is served by THIS index (a different relation → empty —
        // never a widen). Both the InRelation + TupleSet forms resolve to the same real watched set.
        let watched = match leaf {
            RelationalLeaf::InRelation { relation, .. } if relation.0 == ISSUE_WATCHER_RELATION => {
                g.watches
                    .get(&(subject.tenant.0.clone(), subject.principal_id.0.clone()))
                    .cloned()
                    .unwrap_or_default()
            }
            RelationalLeaf::TupleSet { .. } => g
                .watches
                .get(&(subject.tenant.0.clone(), subject.principal_id.0.clone()))
                .cloned()
                .unwrap_or_default(),
            _ => BTreeSet::new(),
        };
        // The index serves at the CURRENT revision; Notif's `resolve_leaf` rejects a served revision
        // below the required watermark (the held-not-leaked gate is Notif's, not Issues').
        Ok(ReverseIndexAnswer {
            subject_roots: watched,
            revision: RevisionWatermark(g.revision),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalId as IdPrincipalId, PrincipalKind};

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }

    fn viewer(id: &str) -> Principal {
        Principal::stub(IdPrincipalId(id.into()), PrincipalKind::Human, tenant())
    }

    fn strong(zk: &str) -> Consistency {
        Consistency {
            at_least: Zookie(zk.into()),
            mode: myelin_identity::ConsistencyMode::Strong,
        }
    }

    // --- the REAL SLA escalation chain (contract 7.5 / §2.4) ---

    /// **Issues' real SLA chain is the FROZEN §2.4 three-tier shape** (team → project → org), every
    /// step `Class::Critical` (an on-call page pierces quiet-hours). The chain replaces the
    /// Notif-defined `esc-test-chain` floor — its policy id is the real Issues policy id, and each
    /// tier resolves a schedule target AT FIRE TIME.
    #[test]
    fn sla_chain_is_the_three_tier_frozen_shape() {
        let policy = issue_sla_escalation_policy(15, 1);
        assert_eq!(policy.policy_id, SLA_ESCALATION_POLICY_ID);
        assert_ne!(
            policy.policy_id, "esc-test-chain",
            "Issues passes its REAL chain, not the Notif test floor"
        );
        assert_eq!(policy.steps.len(), 3, "team → project → org incident lead");
        assert_eq!(policy.repeat, 1);

        // each tier is a SCHEDULE target (resolved at fire time) on the frozen ordered tiers.
        let tiers: Vec<&EscalationTarget> = policy.steps.iter().map(|s| &s.target).collect();
        assert_eq!(
            tiers,
            vec![
                &EscalationTarget::Schedule(SLA_TEAM_ONCALL_SCHEDULE.to_string()),
                &EscalationTarget::Schedule(SLA_PROJECT_ONCALL_SCHEDULE.to_string()),
                &EscalationTarget::Schedule(SLA_ORG_ONCALL_SCHEDULE.to_string()),
            ]
        );
        // every step carries the same ack window + in-app/web-push channels.
        for step in &policy.steps {
            assert_eq!(step.ack_window_minutes, 15);
            assert_eq!(
                step.channels,
                vec![EscalationChannel::InApp, EscalationChannel::WebPush]
            );
        }
    }

    /// **`repeat` is clamped to ≥ 1** (a `repeat = 0` would otherwise exhaust before the first page).
    /// The chain walks at least once.
    #[test]
    fn sla_chain_repeat_is_clamped_to_at_least_one() {
        assert_eq!(issue_sla_escalation_policy(15, 0).repeat, 1);
        assert_eq!(issue_sla_escalation_policy(15, 3).repeat, 3);
        // step_at over a repeat=2 chain walks all six positions then exhausts.
        let policy = issue_sla_escalation_policy(10, 2);
        assert!(policy.step_at(0).is_some());
        assert!(
            policy.step_at(5).is_some(),
            "6 positions over 3 steps × 2 loops"
        );
        assert!(policy.step_at(6).is_none(), "exhausted after 3×2 walks");
    }

    // --- the watcher read-fanout reverse index (contract 4.9 / 4.3 / 4.10) ---

    /// **The watcher relation Issues wires the reverse index over IS the frozen relation Issues' ReBAC
    /// fragment declares (one name, X-5).** The read-fanout JOIN resolves `watcher`; the fragment
    /// declares `watcher` on `issue`; this asserts they are the SAME name + the consumer reads it too.
    #[test]
    fn issue_watcher_relation_matches_the_frozen_fragment() {
        assert_eq!(ISSUE_WATCHER_RELATION, "watcher");
        assert_eq!(ISSUE_WATCHER_RELATION, myelin_notif::WATCHER_RELATION);
        assert_eq!(issue_watchable_object_type(), "issue");
        // the fragment really declares `watcher` on the issue type (the producer half).
        let issue_rels: Vec<String> = crate::rebac_fragment::issue_fragment()
            .relations
            .iter()
            .map(|r| r.0.clone())
            .collect();
        assert!(issue_rels.contains(&"watcher".to_string()));
    }

    /// **The Issues watcher index serves the read-fanout push-down + resolves the real watched set.**
    /// A principal who watches an issue gets that subject_root back through the `InRelation{watcher}`
    /// leaf; a principal who watches nothing gets the empty set (never a widen).
    #[test]
    fn issue_watcher_index_resolves_real_watched_issues() {
        let idx = IssueWatcherIndex::new();
        let issue = "myelin://acme/issue/issue/ENG-1421";
        idx.watch(&tenant(), "psn:alice", issue);

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

        // resolving the watcher leaf returns alice's REAL watched issue.
        let leaf = RelationalLeaf::InRelation {
            relation: RelName("watcher".into()),
            via_column: myelin_notif::subject_root_col(),
        };
        let answer = idx
            .resolve_relation(&viewer("psn:alice"), &leaf, RevisionWatermark(0))
            .expect("available");
        assert!(
            answer.subject_roots.contains(issue),
            "alice watches the issue"
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

    /// **A different relation than `watcher` resolves to the empty set (never a widen).** The Issues
    /// index serves ONLY the Issues watcher relation; a stray relation leaf is deny-by-default.
    #[test]
    fn issue_watcher_index_only_serves_the_watcher_relation() {
        let idx = IssueWatcherIndex::new();
        idx.watch(&tenant(), "psn:alice", "myelin://acme/issue/issue/ENG-1");
        let other = RelationalLeaf::InRelation {
            relation: RelName("assignee".into()),
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

    /// **An unwatch bumps the revision + drops the subject from the watched set (held, not leaked).**
    /// A read after the unwatch no longer reaches the revoked issue; the revision advanced.
    #[test]
    fn issue_watcher_unwatch_revokes_and_bumps_revision() {
        let idx = IssueWatcherIndex::new();
        let issue = "myelin://acme/issue/issue/ENG-9";
        let zk1 = idx.watch(&tenant(), "psn:alice", issue);
        let zk2 = idx.unwatch(&tenant(), "psn:alice", issue);
        assert_ne!(zk1, zk2, "the unwatch bumps the revision (a newer zookie)");

        let leaf = RelationalLeaf::InRelation {
            relation: RelName("watcher".into()),
            via_column: myelin_notif::subject_root_col(),
        };
        let answer = idx
            .resolve_relation(&viewer("psn:alice"), &leaf, RevisionWatermark(0))
            .expect("available");
        assert!(
            !answer.subject_roots.contains(issue),
            "the revoked watch is absent (held, not leaked)"
        );
    }

    /// **An unavailable index is surfaced as `AuthzError::Unavailable` (held, not leaked) on BOTH the
    /// push-down + the relational-leaf path.** Notif's read-fanout treats an unavailable reverse index
    /// as a hold (it does not widen the inbox to "everything").
    #[test]
    fn issue_watcher_unavailable_is_held_not_leaked() {
        let idx = IssueWatcherIndex::new();
        idx.watch(&tenant(), "psn:alice", "myelin://acme/issue/issue/ENG-1");
        idx.set_unavailable(true);

        assert!(matches!(
            idx.list_objects(
                &viewer("psn:alice"),
                &Permission(myelin_notif::WATCH_PERMISSION.into()),
                &ObjectType(myelin_notif::SUBJECT_ROOT_TYPE.into()),
                &strong("zk-1"),
            ),
            Err(AuthzError::Unavailable(_))
        ));
        let leaf = RelationalLeaf::InRelation {
            relation: RelName("watcher".into()),
            via_column: myelin_notif::subject_root_col(),
        };
        assert!(matches!(
            idx.resolve_relation(&viewer("psn:alice"), &leaf, RevisionWatermark(0)),
            Err(AuthzError::Unavailable(_))
        ));
    }
}
