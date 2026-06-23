//! # `trigger` — the stateful Trigger flagship ("Remind me when unblocked") (ISS-P25 / P-392, M4-I6)
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/issue-tracker/architecture/03-events-contracts-and-glue.md`
//! §10 (*The stateful Trigger — the Issues-side ownership*): Issues **owns the Issues-side Trigger UX
//! and semantics** (the armable conditions + the `armed → {resolved | stale | disarmed}` surface); it
//! **consumes** the bus `arm_trigger`/`disarm_trigger` primitive (contract 3.3/3.4), the `myelin-flow`
//! `stale_after` durable timer (contract 9.3), and the ONE Notif inbox for `on_resolve` (contract
//! 7.1). The condition is the **frozen [`myelin_query::EventMatcher`] = the `QueryAst`** over `issue.*`
//! events + `issue_relation` projection state (`Has`/`Ref`/`In` predicates) — the granted CR-5; there
//! is no per-subsystem trigger DSL. VISION §3 (first-class triggers); external-insights/01 §3
//! (prove-it — exactly-once across a restart, stale-once).
//!
//! ## What ISS-P25 ships — the Issues-side stateful Trigger over the CONSUMED primitives
//!
//! Issues does **not** re-implement the Trigger engine, the matcher, or the durable timer wheel
//! (EI-01 §7 — one mechanism, never a second). The fire-once-per-arming engine is the frozen
//! [`myelin_query::TriggerEngine`] (the bus primitive, contract 3.3/3.4); the `stale_after` durability
//! is the frozen [`myelin_query::DurableTimer`] seam (contract 9.3 — the `myelin-flow` minute-bucket
//! wheel, whose deterministic `trigger/<owner>/<arms_subject>` key + cheap disarm/re-arm idiom is
//! documented at `myelin_flow::timer::sla::trigger_stale_timer_id`). This module adds the four things
//! genuinely Issues-owned (§10):
//!
//! 1. **The armable-condition catalogue** ([`ArmableCondition`]) — each a frozen [`EventMatcher`] over
//!    `issue.*` events + `issue_relation` projection state. The flagship is
//!    [`ArmableCondition::RemindWhenUnblocked`] (`condition: blocked_by_unresolved == 0` after all
//!    `blocked_by` edges resolve — reads the projection, NOT a join); the catalogue also carries
//!    "ping me when this leaves state X", "notify me when assigned to me", "SLA at risk", and
//!    "initiative goes at-risk" (§10).
//! 2. **The one inbox for `on_resolve`** ([`IssueTriggerEngine::on_event`]) — a resolve emits **ONE**
//!    inbox item into the ONE Notif inbox (contract 7.1, [`Reason::Unblocked`] →
//!    [`Class::Watching`]), humanised via the ONE templating surface (the [`crate::my_work`] templates).
//!    Never a second store.
//! 3. **The stale nudge that fires ONCE** ([`IssueTriggerEngine::on_stale_timer`]) — after
//!    `stale_after` (default **30d**, per-tenant tunable — [`DEFAULT_STALE_AFTER_DAYS`]) with no
//!    resolution, a single "still blocked after 30d — escalate?" nudge fires into the one inbox and the
//!    arming goes `stale`. **No silent forever-armed promises** (§10); the stale nudge fires
//!    **exactly once** (the stale-once half of ISS-D7).
//! 4. **Durability across a restart** ([`IssueTriggerEngine::snapshot`] /
//!    [`IssueTriggerEngine::restore`]) — the armings are a serializable [`TriggerSnapshot`] the durable
//!    `trigger` table (architecture §3.6) persists; a restart re-loads them and the SAME resolving
//!    event fires **exactly once** (a re-delivery after the restart loses the atomic guarded UPDATE —
//!    the engine's fire-once-per-arming is the source of truth, ISS-D7).
//!
//! ## ISS-D7 — the make-or-break agent-adjacent UX (the green artifact)
//!
//! Arm "remind me when unblocked"; resolve the last blocker **across a restart** → fires **exactly
//! once** into the one inbox; after `stale_after`, the stale nudge fires **once** and the trigger goes
//! stale. "My Work that comes to you" — the platform watches on your behalf and re-surfaces precisely
//! when relevant, durable across restarts/days, zero polling. The 1-fire + stale-once is the dated
//! green artifact (`tests/drill_iss_d7_stateful_trigger.rs`).
//!
//! ## The `stale_after` default (named, per the prompt's FLOOR line)
//!
//! The `stale_after` default is **30 days** ([`DEFAULT_STALE_AFTER_DAYS`]), a **per-tenant tunable**
//! (architecture §10) — a tenant may shorten/lengthen the staleness window, and a trigger armed with
//! `stale_after = None` never goes stale (it waits indefinitely; the engine still fires-once on
//! resolve). No NEW floor beyond this default note (the prompt states so).
//!
//! ## Mutation floor (mandatory-core, EI-01 §2)
//!
//! The fire/stale path is **mandatory-core** — fire-twice-or-never is a governance failure (the
//! prompt's TESTS line). The `cargo-mutants` mutation-score floor for the fire/stale module
//! ([`IssueTriggerEngine::on_event`] + [`IssueTriggerEngine::on_stale_timer`] + the inbox-delivery
//! chokepoint) is **≥ 80%** (the same mandatory-core threshold the write path and reserve/settle carry,
//! EI-01 §2): a surviving mutant that lets a resolve fire twice, never fire, or skip the inbox emit is
//! a false-green and fails the floor. The arming book-keeping (the catalogue → matcher mapping) is not
//! itself data-loss-bearing — its correctness is pinned by the unit + e2e + drill assertions.

use myelin_events::{ArtifactRef, EventEnvelope};
use myelin_identity::{Literal, ObjectType, PrincipalId, SetExpr};
use myelin_notif::router::{InboxProjection, RoutedInboxItem};
use myelin_notif::{Class, Reason};
use myelin_query::{
    arm_trigger, CmpOp, DurableTimer, EventMatcher, Expr, InMemoryTimer, Predicate, Resolution,
    StaleAfter, Trigger, TriggerArming, TriggerEngine, TriggerId, TriggerState,
};
use myelin_tenancy::{Region, TenantId};
use serde::{Deserialize, Serialize};

/// **The default `stale_after` window — 30 days (architecture §10, a per-tenant tunable).** A trigger
/// armed via the catalogue defaults its staleness deadline to `now + 30d`; a tenant may override it,
/// and `stale_after = None` waits indefinitely (the engine still fires-once on resolve). Named here
/// per the prompt's FLOOR line ("the `stale_after` default is 30d (per-tenant tunable)").
pub const DEFAULT_STALE_AFTER_DAYS: i64 = 30;

/// The projection-state variable the flagship "remind me when unblocked" condition reads — the count
/// of still-open `blocked_by` edges on the `arms_subject` issue (the `issue_relation` projection, 01
/// §4). The condition resolves when this reaches 0 (all blockers cleared). This is **projection
/// state**, NOT a join the engine executes (§10) — the rollup/relation consumer maintains the count on
/// the `issue.*`/`issue.relation.*` stream and threads it onto the resolving event's payload.
pub const VAR_BLOCKED_BY_UNRESOLVED: &str = "payload.blocked_by_unresolved";

/// The projection-state variable the "leaves state X" condition reads — the FIXED cross-subsystem
/// `state_category` of the `arms_subject` after a transition (the `issue.transitioned` event, §10).
pub const VAR_STATE_CATEGORY: &str = "payload.state_category";

/// The projection-state variable the "assigned to me" condition reads — the new `assignee` pseudonym
/// after an `issue.assigned` event (§10).
pub const VAR_ASSIGNEE: &str = "payload.assignee";

/// **The armable-condition catalogue (architecture §10).** Each variant compiles to a frozen
/// [`EventMatcher`] = the SAME bounded `QueryAst` core (contract 3.4) over `issue.*` events +
/// `issue_relation` projection state — there is **no per-subsystem trigger DSL** (the granted CR-5).
/// A person arms one of these as a stateful promise; the [`IssueTriggerEngine`] fires it ONCE per
/// arming (resolve) or ONCE on stale (the `stale_after` nudge). The flagship is
/// [`ArmableCondition::RemindWhenUnblocked`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArmableCondition {
    /// **"Remind me when unblocked"** — the flagship (§10). `condition: blocked_by_unresolved == 0`
    /// after all `blocked_by` edges on the `arms_subject` resolve (reads the `issue_relation`
    /// projection, NOT a join). The most cross-subsystem-coupled, make-or-break agent-adjacent UX.
    RemindWhenUnblocked,
    /// **"Ping me when this leaves triage / state X"** — `condition: state_category != X` (§10). `x`
    /// is the FIXED cross-subsystem state-category to leave (e.g. `unstarted`).
    PingWhenLeavesState {
        /// The state-category the issue must LEAVE for the promise to resolve.
        x: String,
    },
    /// **"Notify me when assigned to me"** — `condition: assignee == me` (§10). `me` is the owner's
    /// pseudonym (the assignee the promise watches for).
    NotifyWhenAssignedToMe {
        /// The owner's assignee pseudonym (resolve when the `arms_subject` is assigned to THIS person).
        me: String,
    },
    /// **"Tell me when SLA at risk"** — driven by `sla.at_risk` (§10). The condition resolves on the
    /// SLA-at-risk event for the `arms_subject` (the ISS-P26 SLA engine is the producer).
    TellWhenSlaAtRisk,
    /// **"Tell me when this initiative goes at-risk"** — driven by `initiative.health_changed` (§10).
    TellWhenInitiativeAtRisk,
}

impl ArmableCondition {
    /// **The base condition predicate** (the catalogue's projection-state condition, BEFORE the
    /// `arms_subject` scope). Each is a bounded `Cmp` over `issue.*` projection state — the closed set
    /// of pre-authored predicates (never a user DSL). [`ArmableCondition::to_matcher`] conjoins it with
    /// the `event.subject == arms_subject` scope so a promise resolves ONLY on ITS issue.
    fn base_predicate(&self) -> Predicate {
        match self {
            // blocked_by_unresolved == 0 — all blockers cleared (the projection-state condition).
            ArmableCondition::RemindWhenUnblocked => Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: Expr::Var(VAR_BLOCKED_BY_UNRESOLVED.into()),
                rhs: Expr::Lit(Literal::Int(0)),
            },
            // state_category != X — the issue left state X.
            ArmableCondition::PingWhenLeavesState { x } => Predicate::Cmp {
                op: CmpOp::Ne,
                lhs: Expr::Var(VAR_STATE_CATEGORY.into()),
                rhs: Expr::Lit(Literal::Str(x.clone())),
            },
            // assignee == me — the issue was assigned to the owner.
            ArmableCondition::NotifyWhenAssignedToMe { me } => Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: Expr::Var(VAR_ASSIGNEE.into()),
                rhs: Expr::Lit(Literal::Str(me.clone())),
            },
            // event.type == issue.sla.at_risk — driven by the SLA engine (ISS-P26).
            ArmableCondition::TellWhenSlaAtRisk => Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: Expr::Var("event.type".into()),
                rhs: Expr::Lit(Literal::Str("issue.sla.at_risk".into())),
            },
            // event.type == issue.initiative.health_changed — the initiative health Signal.
            ArmableCondition::TellWhenInitiativeAtRisk => Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: Expr::Var("event.type".into()),
                rhs: Expr::Lit(Literal::Str("issue.initiative.health_changed".into())),
            },
        }
    }

    /// **Compile the catalogue entry to its frozen [`EventMatcher`] (= the `QueryAst` core, 3.4),
    /// SCOPED to the `arms_subject`.** The matcher is `event.subject == arms_subject AND <condition>`
    /// — a bounded, permission-aware predicate over the issue's projection state (no UDFs/loops/
    /// recursion; the over-budget AST is rejected at `EventMatcher::compile`). The `arms_subject` scope
    /// is **load-bearing**: a stateful promise is "wait until C on THIS issue" (§10) — without it, a
    /// `blocked_by_unresolved == 0` event for issue A would also resolve an identical promise armed over
    /// issue B (a spurious cross-issue fire). There is ONE grammar; the catalogue is a closed set of
    /// pre-authored predicates, never a user-supplied DSL.
    pub fn to_matcher(&self, arms_subject: &ArtifactRef) -> EventMatcher {
        let scoped = Predicate::And(vec![
            Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: Expr::Var("event.subject".into()),
                rhs: Expr::Lit(Literal::Str(arms_subject.0.clone())),
            },
            self.base_predicate(),
        ]);
        EventMatcher::compile(ObjectType("issue".into()), scoped)
            .expect("the scoped catalogue predicate is within the cost budget")
    }

    /// **The Notif [`Reason`] this condition surfaces under when it resolves** (contract 7.6 — the
    /// Issues reason set declared at [`crate::declares`]). The flagship "unblocked" surfaces as
    /// [`Reason::Unblocked`] (the calm WATCHING band — the re-surface, not a critical page); the SLA
    /// condition surfaces as [`Reason::Sla`] (critical). The reason drives the §3.1 ranking + the
    /// scoped "My Work" filter.
    pub fn resolve_reason(&self) -> Reason {
        match self {
            ArmableCondition::RemindWhenUnblocked => Reason::Unblocked,
            ArmableCondition::PingWhenLeavesState { .. } => Reason::StateChanged,
            ArmableCondition::NotifyWhenAssignedToMe { .. } => Reason::Assigned,
            ArmableCondition::TellWhenSlaAtRisk => Reason::Sla,
            ArmableCondition::TellWhenInitiativeAtRisk => Reason::StateChanged,
        }
    }
}

/// **One inbox item the stateful Trigger delivered into the ONE Notif inbox (contract 7.1).** A resolve
/// delivers a [`TriggerInboxKind::Resolved`] item ("you can act now — unblocked"); a stale fires a
/// [`TriggerInboxKind::StaleNudge`] item ("still blocked after 30d — escalate?"). References-not-
/// payloads: it carries the `arms_subject` [`ArtifactRef`], never a PII body (the dispatch tier
/// humanises it per-viewer via the ONE templating surface, so a confidential subject still tombstones).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TriggerInboxItem {
    /// Whether this is the resolve fire or the stale nudge.
    pub kind: TriggerInboxKind,
    /// The owner the item is for (the person who armed the promise).
    pub recipient: PrincipalId,
    /// The artifact the promise is about (a ref, never a payload).
    pub subject: ArtifactRef,
    /// The structured why-it-fired (the C-9 scoped-view filter basis).
    pub reason: Reason,
}

/// Whether a delivered [`TriggerInboxItem`] is the resolve fire or the stale nudge (§10).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TriggerInboxKind {
    /// The condition resolved → "you can act now" (the resolve fire — fires exactly once per arming).
    Resolved,
    /// The `stale_after` deadline elapsed with no resolution → "still blocked — escalate?" (the stale
    /// nudge — fires exactly once, then the trigger goes stale).
    StaleNudge,
}

/// **A serializable snapshot of the live armings (the durable `trigger` table, architecture §3.6).**
/// The Issues stateful Trigger's durability across a restart: [`IssueTriggerEngine::snapshot`] captures
/// the armings (state column = the fire-once guard), the durable `trigger` table persists them, and a
/// restart [`IssueTriggerEngine::restore`]s a fresh engine from them so the SAME resolving event fires
/// **exactly once** after the restart (a re-delivery loses the atomic guarded UPDATE). The flagship
/// ISS-D7 across-restart durability rides on this snapshot.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerSnapshot {
    /// The live armings (one per [`TriggerId`]) — the durable `trigger` rows.
    pub armings: Vec<TriggerArming>,
    /// The arming-id counter high-water (so a restored engine mints fresh, non-colliding arming ids).
    pub next_arming: u64,
    /// The deterministic timer-id high-water (so a restored engine's `stale_after` timers stay unique).
    pub next_timer: u64,
}

/// **An armable-condition arm request** — what a person hands to [`IssueTriggerEngine::arm`]. Binds the
/// catalogue entry, the owner, the `arms_subject`, and the optional `stale_after` deadline (defaulting
/// to `now + 30d` when the caller asks for the default).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArmRequest {
    /// The stable trigger id (durable across re-armings; a re-arm mints a fresh arming id).
    pub trigger_id: TriggerId,
    /// The armable condition from the catalogue (§10).
    pub condition: ArmableCondition,
    /// The person who owns the promise (the inbox recipient on resolve/stale).
    pub owner: PrincipalId,
    /// The artifact the promise is about (the `arms_subject`).
    pub arms_subject: ArtifactRef,
    /// The staleness deadline (the precomputed `fire_at`, RFC-3339 UTC). `Some` ⇒ a stale nudge fires
    /// after it; `None` ⇒ the promise never goes stale (waits indefinitely, §10).
    pub stale_after: Option<StaleAfter>,
}

/// **The Issues-side stateful Trigger engine (architecture §10).** Holds the CONSUMED
/// [`myelin_query::TriggerEngine`] (the bus fire-once-per-arming primitive, 3.3/3.4) + a CONSUMED
/// [`myelin_query::DurableTimer`] (the `myelin-flow` 9.3 `stale_after` wheel seam) + the ONE Notif
/// inbox ([`InboxProjection`], 7.1). It NEVER re-implements any of those (EI-01 §7); it adds the
/// Issues-owned semantics: the armable catalogue → matcher mapping, the **one inbox item per resolve**,
/// the **stale nudge that fires once**, and the **snapshot/restore durability**.
///
/// **Fire-once + stale-once, by construction.** A resolve goes through the inner engine's atomic
/// guarded UPDATE (`armed → resolved` only if still `armed`) — under N concurrent resolving events
/// (including a re-delivery after a restart) exactly ONE wins and delivers ONE inbox item; the rest are
/// no-ops. A stale-timer fire goes through the same guard (`armed → stale` only if still `armed`) and
/// delivers ONE stale nudge — a re-fire of the wheel finds the arming `stale` and is a no-op
/// (stale-once). This is the ISS-D7 1-fire + stale-once property.
pub struct IssueTriggerEngine {
    tenant: TenantId,
    region: Region,
    /// The CONSUMED bus Trigger primitive (3.3/3.4) — fire-once-per-arming, never re-implemented.
    engine: TriggerEngine,
    /// The CONSUMED `myelin-flow` durable-timer seam (9.3) — the `stale_after` wheel. The in-memory
    /// floor models the wheel's arm/disarm semantics (the live wheel is `myelin-flow`, dev↔prod a
    /// config swap); the deterministic key convention is documented at
    /// `myelin_flow::timer::sla::trigger_stale_timer_id`.
    timer: InMemoryTimer,
    /// The ONE Notif inbox (7.1) the resolve fire + the stale nudge deliver into. A FILTER over this is
    /// "My Work" ([`crate::my_work`]); there is NO second store.
    inbox: InboxProjection,
    /// The arming-id high-water (snapshot/restore continuity).
    next_arming: u64,
    /// The timer-id high-water (snapshot/restore continuity).
    next_timer: u64,
    /// The catalogue + arming metadata keyed by [`TriggerId`] (so on_event/on_stale can recover the
    /// reason + recipient + subject for the inbox item). Mirrors the durable `trigger` row's columns.
    meta: std::collections::BTreeMap<TriggerId, ArmMeta>,
    /// The delivered inbox items (the test/audit observes the 1-fire + stale-once). The live inbox row
    /// is the `InboxProjection` UPSERT above; this is the ordered delivery log.
    delivered: Vec<TriggerInboxItem>,
}

/// The per-trigger arming metadata the engine recovers on resolve/stale (the durable `trigger` row's
/// owner/subject/condition columns). Snapshot/restore carries it so an across-restart fire still
/// delivers the right inbox item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct ArmMeta {
    condition: ArmableCondition,
    owner: PrincipalId,
    arms_subject: ArtifactRef,
}

impl IssueTriggerEngine {
    /// A fresh engine for one `(tenant, region)` partition over a fresh inbox.
    pub fn new(tenant: TenantId, region: Region) -> IssueTriggerEngine {
        IssueTriggerEngine::with_inbox(tenant, region, InboxProjection::new())
    }

    /// A fresh engine over a SHARED Notif inbox (so a test/the dispatch tier observes the SAME one
    /// inbox the rest of Notif reads — the C-9 one-store truth).
    pub fn with_inbox(
        tenant: TenantId,
        region: Region,
        inbox: InboxProjection,
    ) -> IssueTriggerEngine {
        IssueTriggerEngine {
            tenant,
            region,
            engine: TriggerEngine::new(),
            timer: InMemoryTimer::new(),
            inbox,
            next_arming: 0,
            next_timer: 0,
            meta: std::collections::BTreeMap::new(),
            delivered: Vec::new(),
        }
    }

    /// The ONE Notif inbox handle (7.1) — a FILTER over it is "My Work" ([`crate::my_work`]).
    pub fn inbox(&self) -> &InboxProjection {
        &self.inbox
    }

    /// The ordered delivery log (the test/audit reads it to assert the 1-fire + stale-once artifact).
    pub fn delivered(&self) -> &[TriggerInboxItem] {
        &self.delivered
    }

    /// The current arming for a trigger id (read-only inspection; the dispatch tier reads the durable
    /// `trigger` table, this is the in-engine view).
    pub fn arming(&self, trigger_id: &TriggerId) -> Option<&TriggerArming> {
        self.engine.arming(trigger_id)
    }

    /// **`arm` an armable-condition trigger (contract 3.3 `arm_trigger`).** Compiles the catalogue
    /// entry to its frozen [`EventMatcher`] (3.4), constructs the bus [`Trigger`] (notify-on-resolve),
    /// and arms it through the CONSUMED [`myelin_query::TriggerEngine`] — minting a fresh arming and
    /// DELEGATING the `stale_after` deadline to the [`DurableTimer`] seam (9.3, the `myelin-flow`
    /// wheel; never reinvented). A re-arm of the same [`TriggerId`] mints a NEW arming (idempotency is
    /// per-arming — a re-armed promise can fire again, §10). Returns the trigger id (the durable
    /// handle). A `stale_after` arm failure is surfaced (never a silent drop).
    pub fn arm(&mut self, req: ArmRequest) -> Result<TriggerId, myelin_query::TimerError> {
        let matcher = req.condition.to_matcher(&req.arms_subject);
        let trigger: Trigger = arm_trigger(
            req.owner.clone(),
            matcher,
            req.arms_subject.clone(),
            myelin_query::OnResolve::Notify,
            req.stale_after.clone(),
        );
        self.engine
            .arm(req.trigger_id.clone(), trigger, &self.timer)?;
        // The arming-id/timer-id high-water track the inner engine (snapshot/restore continuity).
        self.next_arming = self.next_arming.max(self.engine_next_arming());
        if req.stale_after.is_some() {
            self.next_timer += 1;
        }
        self.meta.insert(
            req.trigger_id.clone(),
            ArmMeta {
                condition: req.condition,
                owner: req.owner,
                arms_subject: req.arms_subject,
            },
        );
        Ok(req.trigger_id)
    }

    /// **`disarm_trigger(id)` (contract 3.3) — the owner cancels: `armed → disarmed`.** The atomic
    /// guarded UPDATE (only an armed arming disarms; a resolved/stale one is untouched). The
    /// `stale_after` timer is disarmed through the seam so it never fires on a cancelled arming.
    /// Returns `true` iff an armed arming was disarmed.
    pub fn disarm(&mut self, trigger_id: &TriggerId) -> Result<bool, myelin_query::TimerError> {
        self.engine.disarm_trigger(trigger_id, &self.timer)
    }

    /// **The per-event resolve reflex (§10) — fires ONE inbox item exactly once per arming.** Each
    /// ARMED arming's condition is evaluated (permission-aware, 0-leak via the matcher); on a match the
    /// inner engine performs the atomic guarded UPDATE (`armed → resolved`). For the arming that WON
    /// the guard, this delivers **ONE** inbox item into the ONE Notif inbox (7.1) and the `stale_after`
    /// timer is disarmed (it must not fire on a resolved arming). A loser (already resolved / a
    /// re-delivery after a restart) delivers NOTHING (fire-once-per-arming). Returns the count of
    /// armings that newly fired (0 or more — usually 1).
    ///
    /// `visible` is the owner's `list_objects(owner, read, type)` [`SetExpr`] (4.3) the condition
    /// composes with (0-leak); `member_oracle` answers the relational `SetExpr` arms.
    pub fn on_event(
        &mut self,
        envelope: &EventEnvelope,
        visible: &SetExpr,
        member_oracle: &dyn Fn(&myelin_query::RelMembership) -> bool,
    ) -> usize {
        let resolutions = self
            .engine
            .on_event(envelope, visible, member_oracle, &self.timer);
        let mut fired = 0usize;
        for res in resolutions {
            if let Resolution::Resolved {
                trigger_id,
                owner,
                arms_subject,
                ..
            } = res
            {
                // The winner of the guarded UPDATE delivers exactly ONE inbox item (7.1). The reason is
                // the catalogue entry's resolve reason; the recipient is the owner; the subject is the
                // arms_subject (references-not-payloads). A loser/re-delivery never reaches here.
                let reason = self
                    .meta
                    .get(&trigger_id)
                    .map(|m| m.condition.resolve_reason())
                    .unwrap_or(Reason::Unblocked);
                self.deliver(
                    TriggerInboxKind::Resolved,
                    owner,
                    arms_subject,
                    reason,
                    &trigger_id,
                );
                fired += 1;
            }
        }
        fired
    }

    /// **`armed → stale` — the `stale_after` durable timer fired (§10): fire the stale nudge ONCE.**
    /// The CONSUMED `myelin-flow` wheel (9.3) delivers the stale callback for `trigger_id`'s live
    /// arming. The inner engine's atomic guarded UPDATE goes `armed → stale` ONLY if still `armed` (a
    /// prior resolve leaves it `resolved` — the timer LOSES and does NOT clobber the resolution). When
    /// the arming newly goes stale, this delivers **ONE** "still blocked — escalate?" nudge into the
    /// one inbox (7.1) and returns `true`. A re-fire of the wheel finds the arming already `stale` and
    /// is a no-op (stale-once). Returns `true` iff the stale nudge fired.
    ///
    /// **No silent forever-armed promises (§10):** a `stale_after`-armed trigger that never resolves
    /// ALWAYS terminates in a stale nudge — the platform never holds an armed promise forever in
    /// silence.
    pub fn on_stale_timer(&mut self, trigger_id: &TriggerId) -> bool {
        // Recover the live arming id for this trigger (the wheel fires per-arming).
        let Some(arming) = self.engine.arming(trigger_id) else {
            return false;
        };
        let arming_id = arming.arming_id.clone();
        // The guarded transition armed → stale (only an armed arming; the timer loses to a resolve).
        if !self.engine.on_timer_fired(&arming_id) {
            return false; // already resolved / stale / disarmed — no second stale nudge (stale-once).
        }
        // The arming newly went stale → fire ONE stale nudge into the one inbox (7.1).
        if let Some(meta) = self.meta.get(trigger_id).cloned() {
            self.deliver(
                TriggerInboxKind::StaleNudge,
                meta.owner,
                meta.arms_subject,
                meta.condition.resolve_reason(),
                trigger_id,
            );
        }
        true
    }

    /// **Snapshot the live armings (the durable `trigger` table, §3.6) — the across-restart seam.**
    /// Captures the armings (the fire-once guard column) + the id high-waters so a restart can
    /// [`IssueTriggerEngine::restore`] a fresh engine that fires the SAME resolving event **exactly
    /// once** (a re-delivery after the restart loses the guarded UPDATE). The durable `trigger` table
    /// persists this; this models the read of those rows.
    pub fn snapshot(&self) -> TriggerSnapshot {
        let mut armings: Vec<TriggerArming> = self
            .meta
            .keys()
            .filter_map(|id| self.engine.arming(id).cloned())
            .collect();
        armings.sort_by(|a, b| a.trigger_id.0.cmp(&b.trigger_id.0));
        TriggerSnapshot {
            armings,
            next_arming: self.next_arming,
            next_timer: self.next_timer,
        }
    }

    /// **Restore an engine from a [`TriggerSnapshot`] (the across-restart re-load).** Re-loads the
    /// durable `trigger` rows into a fresh inner [`myelin_query::TriggerEngine`] (and re-arms each live
    /// `stale_after` timer through the seam) so the engine resumes EXACTLY where it left off — an
    /// already-resolved arming stays resolved (a re-delivered resolving event after the restart is a
    /// no-op; the fire happened before the restart), an armed arming is still live (the resolving event
    /// fires it once). This is the ISS-D7 across-restart durability.
    pub fn restore(
        tenant: TenantId,
        region: Region,
        inbox: InboxProjection,
        snapshot: TriggerSnapshot,
    ) -> IssueTriggerEngine {
        let mut engine = IssueTriggerEngine::with_inbox(tenant, region, inbox);
        engine.next_arming = snapshot.next_arming;
        engine.next_timer = snapshot.next_timer;
        for arming in snapshot.armings {
            // Re-arm the stale_after timer for a still-armed arming so the wheel can fire it
            // post-restart (a resolved/stale/disarmed arming needs no live timer).
            if arming.state == TriggerState::Armed {
                if let Some(deadline) = &arming.trigger.stale_after {
                    let _ = engine.timer.arm(&arming.arming_id, deadline);
                }
            }
            // Re-insert the durable arming VERBATIM (the `state` column IS the fire-once guard — a
            // resolved arming stays resolved, so a re-delivered event after the restart is a no-op).
            // The catalogue meta (condition/owner/subject) is the durable `trigger` row's other columns,
            // threaded back by the caller via `restore_meta` (the matcher is opaque post-compile).
            engine.engine.restore_arming(arming);
        }
        engine
    }

    /// **Restore the per-trigger catalogue meta (the durable `trigger` row's condition/owner/subject
    /// columns).** Threaded alongside [`IssueTriggerEngine::restore`] — the durable table stores the
    /// catalogue tag + owner + subject as columns, so a restart recovers the inbox-item shape a
    /// post-restart fire needs.
    pub fn restore_meta(
        &mut self,
        trigger_id: TriggerId,
        condition: ArmableCondition,
        owner: PrincipalId,
        arms_subject: ArtifactRef,
    ) {
        self.meta.insert(
            trigger_id,
            ArmMeta {
                condition,
                owner,
                arms_subject,
            },
        );
    }

    /// The catalogue meta for snapshot persistence (the durable `trigger` row's columns) — the caller
    /// persists these alongside the [`TriggerSnapshot`] armings and threads them back through
    /// [`IssueTriggerEngine::restore_meta`].
    pub fn meta_for_snapshot(
        &self,
    ) -> Vec<(TriggerId, ArmableCondition, PrincipalId, ArtifactRef)> {
        self.meta
            .iter()
            .map(|(id, m)| {
                (
                    id.clone(),
                    m.condition.clone(),
                    m.owner.clone(),
                    m.arms_subject.clone(),
                )
            })
            .collect()
    }

    /// Deliver ONE inbox item into the ONE Notif inbox (7.1) + record it in the delivery log. The
    /// dedup key is `(trigger_id, kind)` so a re-fire would collapse onto the SAME row (it never
    /// double-counts) — but the fire-once/stale-once guards mean this is reached at most once per kind
    /// per arming anyway. References-not-payloads: the row carries the `arms_subject` ref, never PII.
    fn deliver(
        &mut self,
        kind: TriggerInboxKind,
        owner: PrincipalId,
        arms_subject: ArtifactRef,
        reason: Reason,
        trigger_id: &TriggerId,
    ) {
        let class = match reason {
            Reason::Sla | Reason::ApprovalRequested => Class::Critical,
            Reason::Assigned => Class::Direct,
            _ => Class::Watching,
        };
        let dedup_key = format!(
            "trigger/{}/{}",
            trigger_id.0,
            match kind {
                TriggerInboxKind::Resolved => "resolved",
                TriggerInboxKind::StaleNudge => "stale",
            }
        );
        let item_id = format!("{}/{}/{}", self.tenant.0, owner.0, dedup_key);
        // UPSERT into the ONE inbox (7.1) — the model of the notif_inbox_item table. A FILTER over it
        // is "My Work"; there is NO second store.
        self.inbox.upsert_for_test(RoutedInboxItem {
            tenant: self.tenant.clone(),
            region: self.region.clone(),
            item_id,
            recipient: owner.0.clone(),
            subject: arms_subject.clone(),
            reason,
            class,
            origin_event: arms_subject.clone(),
            dedup_key,
            coalesce_count: 0,
            state: "unread".into(),
            snooze_until: None,
        });
        self.delivered.push(TriggerInboxItem {
            kind,
            recipient: owner,
            subject: arms_subject,
            reason,
        });
    }

    /// The inner engine's arming-id high-water (so the wrapper's snapshot continuity tracks it).
    fn engine_next_arming(&self) -> u64 {
        self.engine.next_arming()
    }
}

/// Build the [`StaleAfter`] deadline for `now + 30d` (the [`DEFAULT_STALE_AFTER_DAYS`] default, §10).
/// `now_secs` is epoch seconds; the deadline is the precomputed `fire_at` (RFC-3339 UTC) the
/// `myelin-flow` wheel arms on (the cheap disarm/re-arm idiom). A per-tenant override passes its own
/// window instead.
pub fn default_stale_after(now_secs: i64) -> StaleAfter {
    let fire_at = now_secs + DEFAULT_STALE_AFTER_DAYS * 24 * 3600;
    StaleAfter(epoch_secs_to_rfc3339(fire_at))
}

/// Render epoch seconds as an RFC-3339 UTC timestamp (the `stale_after` `fire_at` wire form, §2.10).
/// A minimal, dependency-free encoder (the platform's timestamps are RFC-3339 UTC; the wheel buckets
/// on `epoch_minute(fire_at)`). Used only to mint the default deadline.
fn epoch_secs_to_rfc3339(epoch_secs: i64) -> String {
    // Days since the Unix epoch + the time-of-day, via the civil-from-days algorithm (Howard Hinnant).
    let days = epoch_secs.div_euclid(86_400);
    let secs_of_day = epoch_secs.rem_euclid(86_400);
    let (h, m, s) = (
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60,
    );
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    format!("{year:04}-{month:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{
        Actor, AggregateKey, CorrelationId, DataRole, EventId, EventType, Timestamp, Visibility,
    };
    use myelin_identity::{Principal, PrincipalKind};

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }
    fn owner() -> PrincipalId {
        PrincipalId("alice".into())
    }
    fn subject() -> ArtifactRef {
        ArtifactRef("myelin://acme/issues/issue/PROJ-7".into())
    }

    /// An `issue.relation.resolved`-style event carrying the projection-state count
    /// `payload.blocked_by_unresolved` (the trigger reads projection state, NOT a join, §10).
    fn unblock_event(event_id: &str, blockers_open: i64) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId(event_id.into()),
            type_: EventType("issue.relation.removed".into()),
            schema_ver: 1,
            tenant: tenant(),
            region: region(),
            actor: Actor(Principal::stub(
                PrincipalId("svc-bot".into()),
                PrincipalKind::Human,
                tenant(),
            )),
            subject: subject(),
            aggregate: AggregateKey("issue:PROJ-7".into()),
            causation_id: None,
            correlation_id: CorrelationId("root".into()),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-21T00:00:00Z".into()),
            payload: serde_json::json!({ "blocked_by_unresolved": blockers_open }),
        }
    }

    fn no_rel(_m: &myelin_query::RelMembership) -> bool {
        false
    }

    fn arm_unblock(stale: Option<StaleAfter>) -> ArmRequest {
        ArmRequest {
            trigger_id: TriggerId("t-unblock".into()),
            condition: ArmableCondition::RemindWhenUnblocked,
            owner: owner(),
            arms_subject: subject(),
            stale_after: stale,
        }
    }

    /// **The flagship catalogue entry compiles to the `blocked_by_unresolved == 0` matcher (§10).**
    #[test]
    fn flagship_compiles_to_the_unblock_predicate() {
        let m = ArmableCondition::RemindWhenUnblocked.to_matcher(&subject());
        // The matcher resolves only when the projection-state count is 0 (all blockers cleared) AND
        // the event is about the arms_subject.
        let e0 = unblock_event("e", 0);
        let e1 = unblock_event("e", 1);
        assert!(m.matches(&e0, &SetExpr::All, &no_rel).unwrap());
        assert!(!m.matches(&e1, &SetExpr::All, &no_rel).unwrap());
        // An event for a DIFFERENT issue does not resolve (the arms_subject scope is load-bearing).
        let mut other = unblock_event("e", 0);
        other.subject = ArtifactRef("myelin://acme/issues/issue/OTHER-1".into());
        assert!(!m.matches(&other, &SetExpr::All, &no_rel).unwrap());
    }

    /// **GATE (ISS-D7 fire-half): a partial resolution does NOT fire; the last blocker clearing fires
    /// EXACTLY ONCE into the ONE inbox.**
    #[test]
    fn fires_exactly_once_on_last_blocker_into_the_one_inbox() {
        let mut eng = IssueTriggerEngine::new(tenant(), region());
        eng.arm(arm_unblock(None)).unwrap();

        // One blocker still open → no fire.
        assert_eq!(
            eng.on_event(&unblock_event("e-partial", 1), &SetExpr::All, &no_rel),
            0,
            "a partial resolution does not fire"
        );
        assert!(eng.delivered().is_empty());

        // The last blocker clears → fires exactly once.
        assert_eq!(
            eng.on_event(&unblock_event("e-done", 0), &SetExpr::All, &no_rel),
            1,
            "the last blocker clearing fires exactly once"
        );
        assert_eq!(eng.delivered().len(), 1);
        assert_eq!(eng.delivered()[0].kind, TriggerInboxKind::Resolved);
        assert_eq!(eng.delivered()[0].reason, Reason::Unblocked);
        // The ONE inbox carries exactly one row (7.1).
        assert_eq!(eng.inbox().snapshot_for_tenant(&tenant()).len(), 1);

        // A SECOND resolving event (a re-delivery) does NOT fire again (fire-once-per-arming).
        assert_eq!(
            eng.on_event(&unblock_event("e-dup", 0), &SetExpr::All, &no_rel),
            0,
            "a re-delivery does not fire a second time"
        );
        assert_eq!(eng.delivered().len(), 1, "still exactly one fire");
    }

    /// **GATE (ISS-D7 stale-half): after `stale_after`, the stale nudge fires EXACTLY ONCE and the
    /// trigger goes stale; a re-fire of the wheel is a no-op (stale-once).**
    #[test]
    fn stale_nudge_fires_exactly_once_then_stale() {
        let mut eng = IssueTriggerEngine::new(tenant(), region());
        let deadline = default_stale_after(0);
        eng.arm(arm_unblock(Some(deadline))).unwrap();
        let id = TriggerId("t-unblock".into());

        // The stale_after timer fires (the myelin-flow wheel delivers the callback) → ONE stale nudge.
        assert!(eng.on_stale_timer(&id), "the stale nudge fires once");
        assert_eq!(eng.delivered().len(), 1);
        assert_eq!(eng.delivered()[0].kind, TriggerInboxKind::StaleNudge);
        assert_eq!(eng.arming(&id).unwrap().state, TriggerState::Stale);

        // A re-fire of the wheel is a no-op (stale-once — no second nudge).
        assert!(
            !eng.on_stale_timer(&id),
            "a re-fire after stale is a no-op (stale-once)"
        );
        assert_eq!(eng.delivered().len(), 1, "still exactly one stale nudge");
    }

    /// **A resolve before the stale timer fires WINS — a late stale timer does not clobber it (no
    /// stale nudge after a resolve).**
    #[test]
    fn resolve_wins_over_a_late_stale_timer() {
        let mut eng = IssueTriggerEngine::new(tenant(), region());
        eng.arm(arm_unblock(Some(default_stale_after(0)))).unwrap();
        let id = TriggerId("t-unblock".into());

        assert_eq!(
            eng.on_event(&unblock_event("e", 0), &SetExpr::All, &no_rel),
            1
        );
        assert_eq!(eng.arming(&id).unwrap().state, TriggerState::Resolved);

        // A late stale timer fires — but the arming already resolved, so it loses (no stale nudge).
        assert!(
            !eng.on_stale_timer(&id),
            "the late stale timer loses to the resolve"
        );
        assert_eq!(eng.delivered().len(), 1, "only the resolve fire, no nudge");
        assert_eq!(eng.delivered()[0].kind, TriggerInboxKind::Resolved);
    }

    /// **GATE (ISS-D7 across-restart): arm → snapshot → RESTART (restore) → resolve fires EXACTLY ONCE
    /// after the restart.** The durable `trigger` row survives; the fire-once guard is the source of
    /// truth post-restart.
    #[test]
    fn fires_exactly_once_across_a_restart() {
        let inbox = InboxProjection::new();
        let id = TriggerId("t-unblock".into());

        // Arm before the "restart".
        let mut eng = IssueTriggerEngine::with_inbox(tenant(), region(), inbox.clone());
        eng.arm(arm_unblock(Some(default_stale_after(0)))).unwrap();
        let snapshot = eng.snapshot();
        let meta = eng.meta_for_snapshot();
        // The engine process "crashes" — drop it; the durable trigger table holds the snapshot.
        drop(eng);

        // RESTART: a fresh engine restores from the durable rows (a NEW inbox handle modelling the
        // post-restart process reading the SAME durable inbox table; we re-use the shared inbox).
        let mut eng2 =
            IssueTriggerEngine::restore(tenant(), region(), inbox.clone(), snapshot.clone());
        for (tid, cond, ow, subj) in meta {
            eng2.restore_meta(tid, cond, ow, subj);
        }
        // The arming is still ARMED after the restart (it never fired before the crash).
        assert_eq!(eng2.arming(&id).unwrap().state, TriggerState::Armed);

        // The last blocker clears AFTER the restart → fires exactly once.
        assert_eq!(
            eng2.on_event(&unblock_event("e-after-restart", 0), &SetExpr::All, &no_rel),
            1,
            "the resolve fires exactly once after the restart"
        );
        assert_eq!(eng2.delivered().len(), 1);
        assert_eq!(eng2.delivered()[0].kind, TriggerInboxKind::Resolved);

        // A SECOND restart + a re-delivered resolving event does NOT fire again (the durable state is
        // Resolved — the guard is the source of truth).
        let snap2 = eng2.snapshot();
        let meta2 = eng2.meta_for_snapshot();
        drop(eng2);
        let mut eng3 = IssueTriggerEngine::restore(tenant(), region(), inbox.clone(), snap2);
        for (tid, cond, ow, subj) in meta2 {
            eng3.restore_meta(tid, cond, ow, subj);
        }
        assert_eq!(eng3.arming(&id).unwrap().state, TriggerState::Resolved);
        assert_eq!(
            eng3.on_event(&unblock_event("e-replay", 0), &SetExpr::All, &no_rel),
            0,
            "a re-delivery after a second restart does not re-fire"
        );
        assert!(
            eng3.delivered().is_empty(),
            "the restored engine fires nothing for an already-resolved arming"
        );
    }

    /// **An owner cancel (`disarm`) stops the promise — a later resolving event does not fire.**
    #[test]
    fn owner_cancel_disarms_the_promise() {
        let mut eng = IssueTriggerEngine::new(tenant(), region());
        eng.arm(arm_unblock(Some(default_stale_after(0)))).unwrap();
        let id = TriggerId("t-unblock".into());

        assert!(eng.disarm(&id).unwrap(), "the owner cancel disarms");
        assert_eq!(eng.arming(&id).unwrap().state, TriggerState::Disarmed);
        assert_eq!(
            eng.on_event(&unblock_event("e", 0), &SetExpr::All, &no_rel),
            0,
            "a disarmed promise does not fire"
        );
        // Nor does the stale timer fire on a disarmed arming.
        assert!(!eng.on_stale_timer(&id));
        assert!(eng.delivered().is_empty());
    }

    /// **The 0-leak property: an unviewable subject never resolves the trigger (§10, contract 4.3).**
    #[test]
    fn unviewable_subject_never_resolves() {
        let mut eng = IssueTriggerEngine::new(tenant(), region());
        eng.arm(arm_unblock(None)).unwrap();
        // SetExpr::None ⇒ the owner sees nothing ⇒ no resolution (the condition is never consulted).
        assert_eq!(
            eng.on_event(&unblock_event("e", 0), &SetExpr::None, &no_rel),
            0,
            "an unviewable subject never resolves (0-leak)"
        );
        assert!(eng.delivered().is_empty());
        assert_eq!(
            eng.arming(&TriggerId("t-unblock".into())).unwrap().state,
            TriggerState::Armed
        );
    }

    /// **The default `stale_after` is `now + 30d` (the named default, §10).**
    #[test]
    fn default_stale_after_is_thirty_days() {
        // epoch 0 = 1970-01-01T00:00:00Z; +30d = 1970-01-31T00:00:00Z.
        assert_eq!(default_stale_after(0).0, "1970-01-31T00:00:00Z");
        // A known date: 2026-06-21T00:00:00Z is epoch 1_782_000_000; +30d = 2026-07-21.
        let known = 1_782_000_000;
        assert_eq!(default_stale_after(known).0, "2026-07-21T00:00:00Z");
    }

    /// **The snapshot round-trips stably (the durable `trigger` row — the across-restart wire form).**
    #[test]
    fn snapshot_round_trips_stably() {
        let mut eng = IssueTriggerEngine::new(tenant(), region());
        eng.arm(arm_unblock(Some(default_stale_after(0)))).unwrap();
        let snap = eng.snapshot();
        let json = serde_json::to_string(&snap).unwrap();
        let back: TriggerSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(snap, back);
        assert_eq!(back.armings.len(), 1);
    }
}
