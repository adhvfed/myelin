//! # `dispatch` — explicit-first dispatch wiring (AG-P20 → P-347, M4; contract 8.6, §3.4 / CHAT-1)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/agent-fabric.md` §3.4 (**explicit-first dispatch,
//! pinned** — runtime dispatch is the explicit "run an agent here"; a mention **NOTIFIES** via Notif's
//! one inbox, it does **NOT** auto-spawn a costed agent run; even an explicit run passes the
//! reserve/settle gate; implicit auto-dispatch on a casual mention is **L-3, counsel-gated** — GDPR
//! Art. 22 / EU AI-Act human-oversight — NOT built here), §3.6 (the dispatch tier), §5.6 (a run is a
//! durable workflow fronted by reserve/settle).
//!
//! **VISION §3** (explicit, governed agent dispatch; consequential actions human-confirmed).
//! **EI-03 §7** (explicit-first dispatch: a mention notifies, does not auto-spawn a costed run).
//! **EI-01 §3** (a property does not exist until a test forces it — the 0-auto-spawn property is the
//! [`DispatchCounter`], asserted by the CHAT-D17 drill).
//!
//! **Contract-index:** OWNS the wiring of **8.6** (`EventInbox::deliver` — explicit-first: deliver
//! NOTIFIES; the COSTED run is started only by an EXPLICIT trigger, behind reserve). The dispatch-tier
//! transport is the Bus's ([`crate::app::SkeletonDispatchConsumer`] binds the subject whitelist); THIS
//! module is the TYPED CLASSIFIER the dispatch tier consults to decide notify-vs-dispatch.
//!
//! ## The explicit-first decision (the typed classifier — the NEW wiring AG-P20 adds)
//! A delivered trigger is classified into a [`DispatchDecision`] BEFORE any cost is incurred:
//! - a **casual `@agent` mention** ([`DispatchTrigger::Mention`]) → [`DispatchDecision::Notify`] — the
//!   inbox is notified (0 cost, 0 spawn). The dispatch-counter STAYS 0. There is NO path from a casual
//!   mention to a costed run (the L-3 floor — implicit auto-dispatch is not wired).
//! - an **explicit "run an agent here" trigger** ([`DispatchTrigger::ExplicitRun`] — a structured
//!   action / a slash-command / a button, NOT raw text) → [`DispatchDecision::Dispatch`] — the run is
//!   dispatched, and EVEN the explicit run passes the reserve gate (§3.4 last sentence).
//! - a **structured artifact-ref re-trigger** ([`DispatchTrigger::StructuredRef`]) → also
//!   [`DispatchDecision::Dispatch`] (the loop-path re-trigger the reference gate already admits,
//!   [`crate::loop_guards::ReferenceGate`]); it too passes reserve.
//!
//! The classifier is the SAFETY BOUNDARY in the TYPE: only an [`ExplicitRun`](DispatchTrigger::
//! ExplicitRun) / [`StructuredRef`](DispatchTrigger::StructuredRef) can produce a
//! [`Dispatch`](DispatchDecision::Dispatch); a [`Mention`](DispatchTrigger::Mention) can NEVER (there
//! is no arm that maps a mention to dispatch). This mirrors the reference gate's "raw text never
//! re-triggers" invariant (§5.5) — a casual mention is the dispatch-tier analogue.
//!
//! ## Why this is NOT a new engine (EI-03 §4)
//! The COSTED run is still [`crate::skeleton::SkeletonAgent::handle_run`] (mint → reserve → step →
//! settle); this module does NOT reserve, run, or settle — it makes the notify-vs-dispatch DECISION
//! that gates entry into that existing path. The reserve gate is the existing 11.7 seam; the inbox is
//! the existing 8.6 seam. This is the typed decision the dispatch tier was missing.
//!
//! ## FLOOR named (cross-reference; VISION §3, EI-01 §1)
//! - **Implicit auto-dispatch on a casual mention remains [OPEN → LEGAL] (L-3, counsel-gated)** — GDPR
//!   Art. 22 / EU AI-Act human-oversight. Explicit-first is v1; NO auto-spawn path is wired — a casual
//!   [`Mention`](DispatchTrigger::Mention) ALWAYS resolves [`Notify`](DispatchDecision::Notify), never
//!   [`Dispatch`](DispatchDecision::Dispatch). The auto-spawn path (intent/cost detection) is NOT built
//!   until counsel ratifies the human-oversight basis (Chat P6 + counsel). This is the defensible
//!   posture, stated in writing.

/// **A delivered dispatch trigger, classified by KIND (§3.4 / 8.6).** The dispatch tier builds this
/// from a delivered event's binding (the Bus matched it to this agent). The KIND is what
/// explicit-first turns on: a casual mention is a notification; only an explicit action / a structured
/// re-trigger dispatches a costed run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DispatchTrigger {
    /// **A casual `@agent` mention** — a human (or another agent) named the agent in a message. This
    /// NOTIFIES the inbox; it does NOT auto-spawn a costed run (explicit-first, §3.4). Carries the
    /// mentioning message's ref (references-not-payloads) for the notification.
    Mention(String),
    /// **An EXPLICIT "run an agent here" trigger** — a structured action: a slash-command, a "run
    /// agent" button, a structured trigger event (NOT raw text). DISPATCHES a costed run (through the
    /// reserve gate). Carries the explicit-run request ref.
    ExplicitRun(String),
    /// **A structured `artifact_ref` re-trigger** — the loop-path re-trigger the reference gate admits
    /// (§5.5; [`crate::loop_guards::ReferenceGate`]). DISPATCHES (through reserve). Carries the
    /// structured artifact-ref the re-trigger keys on.
    StructuredRef(String),
}

/// **The explicit-first dispatch decision (the typed classifier output, §3.4).** Either the inbox is
/// NOTIFIED (no costed run; the casual-mention path) or a run is DISPATCHED (through the reserve gate;
/// the explicit-trigger path). A [`Mention`](DispatchTrigger::Mention) can ONLY ever produce
/// [`Notify`] — the 0-auto-spawn invariant is in the type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DispatchDecision {
    /// **NOTIFY the inbox** — a casual mention notified, no costed run spawned (explicit-first, §3.4).
    /// Carries the notified ref. This is the ONLY decision a [`Mention`](DispatchTrigger::Mention) can
    /// produce.
    Notify(String),
    /// **DISPATCH a costed run** — an explicit trigger / a structured re-trigger. The run is dispatched
    /// behind the reserve gate (even an explicit run passes reserve, §3.4). Carries the trigger ref the
    /// dispatch tier builds the `RunSubstrate` from.
    Dispatch(String),
}

impl DispatchDecision {
    /// Whether this decision DISPATCHES a costed run (vs notifies). The CHAT-D17 drill asserts a casual
    /// mention NEVER dispatches.
    pub fn dispatches(&self) -> bool {
        matches!(self, DispatchDecision::Dispatch(_))
    }

    /// Whether this decision merely NOTIFIES (no costed run).
    pub fn notifies(&self) -> bool {
        matches!(self, DispatchDecision::Notify(_))
    }
}

/// **Classify a delivered trigger into the explicit-first dispatch decision (§3.4 / 8.6 / CHAT-1) —
/// the SAFETY BOUNDARY in the type.** A casual [`Mention`](DispatchTrigger::Mention) ALWAYS resolves
/// [`Notify`](DispatchDecision::Notify) (0 auto-spawn — the L-3 floor: implicit auto-dispatch is NOT
/// wired). An [`ExplicitRun`](DispatchTrigger::ExplicitRun) or a structured
/// [`StructuredRef`](DispatchTrigger::StructuredRef) resolves [`Dispatch`](DispatchDecision::Dispatch)
/// (the costed run, which STILL passes the reserve gate). There is NO arm that maps a mention to
/// dispatch — the 0-auto-spawn property is structural, not a runtime branch that could be flipped.
pub fn classify(trigger: &DispatchTrigger) -> DispatchDecision {
    match trigger {
        // Explicit-first: a casual mention NOTIFIES. No arm here produces Dispatch — implicit
        // auto-dispatch is the L-3 floor (counsel-gated), so a mention can NEVER spawn a costed run.
        DispatchTrigger::Mention(r) => DispatchDecision::Notify(r.clone()),
        // An explicit "run an agent here" / a structured re-trigger DISPATCHES (still through reserve).
        DispatchTrigger::ExplicitRun(r) | DispatchTrigger::StructuredRef(r) => {
            DispatchDecision::Dispatch(r.clone())
        }
    }
}

/// **The dispatch counter — the 0-auto-spawn telemetry the CHAT-D17 drill reads (§3.4 / EI-01 §3).**
/// The dispatch tier increments [`auto_spawns`](Self::auto_spawns) ONLY when it actually dispatches a
/// costed run; a casual-mention notification increments [`notifications`](Self::notifications). The
/// CHAT-D17 gate is: after N casual mentions, `auto_spawns == 0`. A property that is not counted does
/// not exist (EI-01 §3) — this is the counted artifact.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DispatchCounter {
    auto_spawns: u64,
    notifications: u64,
}

impl DispatchCounter {
    /// A fresh counter (0 auto-spawns, 0 notifications).
    pub fn new() -> DispatchCounter {
        DispatchCounter::default()
    }

    /// **Route a classified decision through the counter — the dispatch tier's single accounting
    /// seam.** A [`Notify`](DispatchDecision::Notify) increments the notification counter (0 spawn); a
    /// [`Dispatch`](DispatchDecision::Dispatch) increments the auto-spawn counter. Returns the decision
    /// unchanged (so the dispatch tier acts on it) — the counting is the side-effect the drill reads.
    pub fn route(&mut self, decision: DispatchDecision) -> DispatchDecision {
        match &decision {
            DispatchDecision::Notify(_) => self.notifications += 1,
            DispatchDecision::Dispatch(_) => self.auto_spawns += 1,
        }
        decision
    }

    /// **Classify-and-route in one step — the dispatch tier's entry point.** Classifies the trigger
    /// (explicit-first) and routes the decision through the counter. The CHAT-D17 invariant: feeding
    /// only [`Mention`](DispatchTrigger::Mention)s leaves [`auto_spawns`](Self::auto_spawns) == 0.
    pub fn dispatch(&mut self, trigger: &DispatchTrigger) -> DispatchDecision {
        let decision = classify(trigger);
        self.route(decision)
    }

    /// **The number of costed runs auto-spawned (the CHAT-D17 gate — must be 0 for casual mentions).**
    pub fn auto_spawns(&self) -> u64 {
        self.auto_spawns
    }

    /// The number of casual-mention notifications delivered (0 spawn each).
    pub fn notifications(&self) -> u64 {
        self.notifications
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **A casual `@agent` mention NOTIFIES — it does NOT dispatch a costed run (explicit-first,
    /// §3.4 / CHAT-1).** The classifier resolves [`Notify`]; the decision does not dispatch.
    #[test]
    fn a_casual_mention_notifies_never_dispatches() {
        let d = classify(&DispatchTrigger::Mention("myelin://acme/chat/msg/1".into()));
        assert!(d.notifies(), "a casual mention NOTIFIES");
        assert!(
            !d.dispatches(),
            "a casual mention does NOT auto-spawn a costed run (explicit-first)"
        );
        assert_eq!(
            d,
            DispatchDecision::Notify("myelin://acme/chat/msg/1".into())
        );
    }

    /// **An EXPLICIT "run an agent here" trigger DISPATCHES a costed run (which still passes reserve,
    /// §3.4).** The classifier resolves [`Dispatch`].
    #[test]
    fn an_explicit_trigger_dispatches() {
        let d = classify(&DispatchTrigger::ExplicitRun(
            "myelin://acme/agent/run-req/9".into(),
        ));
        assert!(
            d.dispatches(),
            "an explicit trigger DISPATCHES a costed run"
        );
        assert!(!d.notifies());
    }

    /// **A structured artifact-ref re-trigger DISPATCHES (the loop-path re-trigger the reference gate
    /// admits, §5.5).** A structured node may re-trigger (raw text never does, the precedent in
    /// [`crate::loop_guards::ReferenceGate`]); the dispatch classifier mirrors that.
    #[test]
    fn a_structured_ref_re_trigger_dispatches() {
        let d = classify(&DispatchTrigger::StructuredRef(
            "myelin://acme/issues/issue/PROJ-1".into(),
        ));
        assert!(
            d.dispatches(),
            "a structured artifact-ref re-trigger dispatches"
        );
    }

    /// **CHAT-D17 (the gate) — N casual `@agent` mentions → 0 auto-spawn (the dispatch-counter STAYS
    /// 0); each is a notification.** The 0-auto-spawn property is the COUNTED artifact (EI-01 §3): no
    /// number of casual mentions can spawn a costed run.
    #[test]
    fn chat_d17_casual_mentions_zero_auto_spawn() {
        let mut counter = DispatchCounter::new();
        // a stream of casual mentions — the typical "@agent can you look at this?" chatter.
        for i in 0..10 {
            let decision = counter.dispatch(&DispatchTrigger::Mention(format!(
                "myelin://acme/chat/msg/{i}"
            )));
            assert!(decision.notifies(), "each casual mention NOTIFIES");
        }
        assert_eq!(
            counter.auto_spawns(),
            0,
            "CHAT-D17: 0 auto-spawn on casual mentions (the dispatch-counter stays 0)"
        );
        assert_eq!(
            counter.notifications(),
            10,
            "all ten mentions were delivered as notifications (0 cost)"
        );
    }

    /// **CHAT-D17 (the other half) — an EXPLICIT run on the SAME counter increments auto-spawns, and
    /// THAT run is the one that passes the reserve gate (§3.4).** The mix proves the boundary is the
    /// trigger KIND, not the counter: casual chatter never spawns, the one explicit run does.
    #[test]
    fn chat_d17_explicit_run_is_the_only_spawn() {
        let mut counter = DispatchCounter::new();
        counter.dispatch(&DispatchTrigger::Mention("msg/1".into()));
        counter.dispatch(&DispatchTrigger::Mention("msg/2".into()));
        let explicit = counter.dispatch(&DispatchTrigger::ExplicitRun("run-req/1".into()));
        assert!(
            explicit.dispatches(),
            "the explicit run dispatches (and passes reserve downstream)"
        );
        assert_eq!(
            counter.auto_spawns(),
            1,
            "exactly ONE costed run was dispatched — the explicit one (the two mentions spawned 0)"
        );
        assert_eq!(
            counter.notifications(),
            2,
            "the two casual mentions notified"
        );
    }

    /// **The L-3 floor is structural: no [`DispatchTrigger::Mention`] maps to
    /// [`DispatchDecision::Dispatch`].** This pins the invariant the no-auto-spawn floor relies on — a
    /// future hand-edit that wired a mention to dispatch would flip this assertion (the implicit
    /// auto-dispatch path stays unwired until counsel ratifies it).
    #[test]
    fn no_mention_can_ever_dispatch_the_l3_floor() {
        for r in ["a", "b", "c", "@agent please ship it"] {
            let d = classify(&DispatchTrigger::Mention(r.into()));
            assert!(
                !d.dispatches(),
                "a mention can NEVER dispatch (the L-3 auto-dispatch floor is structural): {r}"
            );
        }
    }
}
