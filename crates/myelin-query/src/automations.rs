//! # `automations` — Automations: the stateless per-event reflex over the matcher
//! (contract 3.2; Bus §3.5 / §1.2 / §5.4; P-139 / EB-19)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/event-bus.md` §1.2 (the four primitives —
//! **Automation rule** = *a stateless, per-event reflex the project owns: "when X, do Y."* It
//! **may invoke a workflow**; its definition persists but **each firing is stateless** — there
//! is NO per-person state, that is the [`Trigger`](super) (§3.6, the stateful per-person
//! promise EB-20 builds, deliberately NOT collapsed with this), ADR-19), §3.5 (the
//! `automation_rule` store — `action.kind = workflow` invokes `myelin-flow`, contract 3.2),
//! §5.4 (the `register_automation(AutomationRule{ matcher /* QueryAst */, action, run_as,
//! delegation, budget, gates })` surface). Contract-index rows **3.2** (`register_automation`,
//! owned) + **9.1 / 9.2** (the `myelin-flow` durable-workflow surface — CONSUMED, for
//! `action.kind = workflow`).
//!
//! ## Why the Automation engine lives in `myelin-query`, not `myelin-events` (DOCUMENTED DEVIATION)
//! The EB-19 prompt's DELIVERABLE field says "In `myelin-events`: `automations.rs`". That is
//! **genuinely unworkable against the frozen crate DAG** for the SAME reason the
//! [`EventMatcher`](crate::EventMatcher) (P-137 / EB-17) and the [`SignalEngine`](crate::SignalEngine)
//! (P-138 / EB-18) had to be built here and not in `myelin-events` (see [`crate::matcher`] §"Why
//! the matcher lives in `myelin-query`"): an Automation's `matcher` field **IS** an
//! [`EventMatcher`], whose predicate ENGINE was promoted into `myelin-query` by P-133, and
//! `myelin-query` **depends on `myelin-events`** (architecture §2.9). Putting the Automation
//! engine in `myelin-events` would require `…-events → …-query` for the matcher type — the cycle
//! the `no-cross-sync-cycle` lint (E-5) and the events `Cargo.toml` forbid. So the Automation
//! engine is built HERE, ON TOP of the one [`EventMatcher`], over the upstream [`EventEnvelope`].
//! The Bus dispatch tier (EB-23) references `myelin_query::AutomationEngine`. This deviation is
//! recorded here and in the P-139 report, per external-insights/01 §1 (do the right thing;
//! document the deviation), and it is the SAME pattern the matcher + signals already follow.
//!
//! ## What this module adds (it does NOT re-define the matcher, the predicate engine, or `myelin-flow`)
//! - [`AutomationRule`] + [`register_automation`] — a stateless reflex: the [`EventMatcher`] that
//!   selects, the [`Action`] to run, the [`RunAs`] identity + [`Delegation`] caveats it runs
//!   under, the [`Budget`] it is bounded by, and the [`Gate`]s it must pass.
//! - [`AutomationEngine`] — the stateless per-event reflex: on a matching event (permission-aware
//!   BY CONSTRUCTION through [`EventMatcher::matches`]), it runs the action under
//!   `run_as + delegation` within `budget + gates`. **Idempotent on `event_id`** (the EB-06
//!   `consumer_dedup` discipline, modelled here by a per-`(rule, event)` seen-set): a redelivered
//!   event fires the automation **exactly once per delivery**, never twice.
//! - [`Action`] / [`ActionKind`] — `Emit` (yield an outbox [`PublishDraft`], the publish-is-
//!   outbox-only discipline, P-S10) and **`Workflow`** (DELEGATE to `myelin-flow`'s
//!   [`DurableExecutor::start`], contract 9.1 — NOT reinvented here; the engine never runs a
//!   bespoke durable loop).
//! - [`DurableExecutor`] — the CONSUMED `myelin-flow` seam (contract 9.1 `{start, …}`). The REAL
//!   engine is the downstream `myelin-flow` crate (**P-FLOW-04 / P-199 + P-203**, the named
//!   floor); this crate is upstream of it in the §2.9 DAG, so it consumes the seam exactly as
//!   [`crate::matcher`] consumes the `list_objects` push-down and `myelin-events::holder` consumes
//!   the KMS crypto-shred seam. [`InMemoryExecutor`] is the deterministic floor for the unit/CDC
//!   tests.
//!
//! **Stateless, by construction.** An [`AutomationEngine`] holds the *rules* and the *idempotency
//! ledger* (a fired-once guard, not application state) — it holds NO per-person promise, no
//! armed→resolved lifecycle (that is the [`Trigger`], EB-20). Each event is matched independently;
//! the only memory is "did THIS `(rule, event_id)` already fire?" (the effectively-once anchor,
//! 2.5). The reflex is a pure function of `(rule, event, visible-set, budget-state)` → effects;
//! the same event sequence replays to the same effects (what BUS-D3 in EB-23 relies on).

use crate::matcher::RelMembership;
use crate::{EventMatcher, PublishDraft, PublishKind, Severity, Signal, SignalState};
use myelin_events::{ArtifactRef, EventEnvelope, EventId};
use myelin_identity::{DelegationCaveats, PrincipalId, SetExpr};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// A stable automation-rule identifier (the `automation_rule.id`, §3.5). Scopes the idempotency
/// ledger key `(rule_id, event_id)` and names the rule in the dispatch-tier audit.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AutomationId(pub String);

/// A reference to a `myelin-flow` workflow definition (the `action.kind = workflow` target,
/// contract 9.1). Opaque here: the durable engine resolves it to a registered workflow. The
/// Automation engine never interprets it — it hands it to [`DurableExecutor::start`].
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct WorkflowRef(pub String);

/// **The identity an automation's action runs AS** (`run_as`, §5.4). An Automation is a
/// *project*-owned reflex, so it runs as a project/service principal (NEVER the triggering user's
/// ambient identity — a reflex must not silently act with a human's authority). The effective
/// authority is `run_as`'s policy intersected with [`Delegation`] (the monotone narrowing, 4.5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunAs(pub PrincipalId);

/// **The delegation caveats the action carries** (`delegation`, §5.4; contract 4.5). The action's
/// effective policy is `run_as.policy ∩ delegation ∩ tenant.policy` — a MONOTONE intersection
/// (delegation only ever NARROWS authority, never widens it; AG-2). Reuses the frozen Identity
/// [`DelegationCaveats`] type — there is no second delegation language.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delegation(pub DelegationCaveats);

impl Delegation {
    /// No extra caveats — the action runs at exactly `run_as`'s (already tenant-bounded) policy.
    pub fn none() -> Delegation {
        Delegation(DelegationCaveats(Vec::new()))
    }
}

/// **The budget an automation firing is bounded by** (`budget`, §5.4). Integer minor-units /
/// counts (the frozen unit posture, §2.10 — never floats). A firing that would exceed the
/// remaining budget is **shed**, not run: a runaway automation can never consume unbounded
/// resources (the reactive-tier shed posture, §4.7 / OQ-K). `max_firings` caps how many times the
/// rule may fire within the engine's accounting window; `cost_units` is the per-firing charge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Budget {
    /// The maximum number of firings the rule may make within the accounting window. `0` ⇒ the
    /// rule is effectively disabled (defined but never fires — a safe default for a paused rule).
    pub max_firings: u64,
    /// The cost charged per firing (integer minor-units / abstract cost tokens). The dispatch tier
    /// (EB-23) maps it onto the real per-tenant reactive budget (§4.7).
    pub cost_units: u64,
}

impl Budget {
    /// A generous default budget for a normally-active rule.
    pub fn unbounded_within(max_firings: u64) -> Budget {
        Budget {
            max_firings,
            cost_units: 1,
        }
    }
}

/// **A gate an automation firing must pass before its action runs** (`gates`, §5.4). Gates are
/// the structural pre-conditions a project attaches to a reflex (e.g. "only on the default
/// branch", "only when the actor is a service principal", "require human approval"). A gate that
/// does NOT hold **suppresses the firing** (fail-closed — a reflex never runs through a gate it
/// cannot satisfy). The set is closed (no UDFs) so the gate decision is bounded + auditable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Gate {
    /// The firing requires a human approval before the action runs. The engine SUPPRESSES the
    /// firing and yields an [`Outcome::AwaitingApproval`]; the dispatch tier (EB-23) raises the
    /// HITL card (the durable approval rides a `myelin-flow` workflow's signal, 9.1). A reflex
    /// behind this gate NEVER runs the action inline.
    RequireHumanApproval,
    /// The firing requires the triggering event NOT carry personal data (a reflex that fans out
    /// must not propagate PII inline — references-not-payloads, §3.1). Fail-closed: an event
    /// flagged `contains_personal_data` suppresses the firing.
    RequireNoPersonalData,
    /// The firing requires the event's `depth` be at or below a ceiling — the causal-loop guard
    /// (D-6): a reflex that would deepen an already-deep causal chain is suppressed before it can
    /// contribute to a self-trigger storm (the depth ceiling + shared-root tripwire, §4.7).
    MaxCausalDepth(u32),
}

impl Gate {
    /// Evaluate the gate against the triggering envelope. `RequireHumanApproval` is NOT decided
    /// here — it is a *suppress-and-escalate* gate handled by [`AutomationEngine::ingest`]; this
    /// returns `true` for it so the inline-pass loop treats it as "passes the inline checks, then
    /// the engine routes it to the approval lane". The other gates are pure predicates over the
    /// envelope.
    fn passes_inline(&self, envelope: &EventEnvelope) -> bool {
        match self {
            Gate::RequireHumanApproval => true,
            Gate::RequireNoPersonalData => !envelope.contains_personal_data,
            Gate::MaxCausalDepth(max) => envelope.depth <= *max,
        }
    }

    /// Whether this gate routes the firing to the human-approval lane (suppress-inline-and-escalate).
    fn is_approval(&self) -> bool {
        matches!(self, Gate::RequireHumanApproval)
    }
}

/// **What an automation does on a match** (`action`, §5.4). A closed set: `Emit` (yield an outbox
/// publish draft — publish is outbox-only, P-S10) or `Workflow` (DELEGATE to `myelin-flow`'s
/// [`DurableExecutor`], 9.1 — the durable engine is never reinvented here).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Action {
    pub kind: ActionKind,
}

/// The kind of an automation [`Action`] (the closed action vocabulary, §1.2 "do Y").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionKind {
    /// **Emit a derived event** — the reflex publishes a follow-on fact (e.g. an automation that
    /// labels an issue emits `issues.issue.labelled`). The engine yields a [`PublishDraft`]; the
    /// dispatch tier (EB-23) turns it into `OutboxTx::emit` in the SAME transaction as the
    /// automation-firing record (publish is outbox-only by construction — the engine NEVER
    /// publishes; the `no-raw-publish` lint, P-S10, enforces the absence of any other path).
    Emit {
        /// The event-type token of the derived event (`<subsystem>.<artifact>.<event>`, §6).
        emit_type: String,
        /// The subject the derived event is about (carried from / derived from the trigger).
        subject: ArtifactRef,
    },
    /// **Invoke a durable workflow** — DELEGATE to `myelin-flow`'s [`DurableExecutor::start`]
    /// (contract 9.1). The Automation engine does NOT run a bespoke durable loop: it hands the
    /// [`WorkflowRef`] + a references-not-payloads input to the executor, which returns a durable
    /// [`DurableHandle`]. This is the ADR-09 boundary — automations *invoke* `myelin-flow`, they
    /// do not re-implement it.
    Workflow {
        /// The `myelin-flow` workflow definition to start.
        workflow_ref: WorkflowRef,
        /// A references-not-payloads input (ids / `ArtifactRef`s, never PII bodies) the workflow
        /// reads. Opaque to the engine.
        input: serde_json::Value,
    },
}

/// **An Automation rule** (contract 3.2; §5.4 frozen shape `AutomationRule{ matcher, action,
/// run_as, delegation, budget, gates }`): the selector, the action, the identity + delegation it
/// runs under, the budget it is bounded by, and the gates it must pass.
///
/// Built via [`register_automation`]. The `matcher` is an [`EventMatcher`] — the one bounded,
/// permission-aware predicate surface — so an Automation can never select an artifact the rule's
/// principal can't see (§4.5; the 0-leak property rides through [`AutomationEngine::ingest`]). It
/// is **stateless**: the rule definition persists, but a firing carries no per-person promise
/// (that is the [`Trigger`], EB-20 — deliberately a different primitive, ADR-19).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationRule {
    /// The stable rule id (the `automation_rule.id`; the idempotency-ledger namespace + audit
    /// handle).
    pub rule_id: AutomationId,
    /// The selector — the [`EventMatcher`] (= the frozen `QueryAst`, §4.5). An event fires the
    /// rule iff it matches this matcher (after the permission compose).
    pub matcher: EventMatcher,
    /// The action to run on a match (`Emit` or `Workflow`).
    pub action: Action,
    /// The identity the action runs AS (a project/service principal — NOT the triggering user).
    pub run_as: RunAs,
    /// The delegation caveats narrowing the run-as authority (`run_as.policy ∩ delegation ∩
    /// tenant.policy`, monotone, 4.5).
    pub delegation: Delegation,
    /// The budget bounding firings (a runaway reflex is shed, not run; §4.7).
    pub budget: Budget,
    /// The gates a firing must pass before its action runs (fail-closed; suppress-and-escalate for
    /// human approval).
    pub gates: Vec<Gate>,
}

/// **`register_automation(AutomationRule{ matcher, action, run_as, delegation, budget, gates })`**
/// (contract 3.2) — the registration verb. Constructs an [`AutomationRule`]. The `matcher`
/// [`EventMatcher`] was already cost-validated at its own `compile` (the over-budget AST was
/// rejected at construction, §4.5), so this verb is total.
#[allow(clippy::too_many_arguments)]
pub fn register_automation(
    rule_id: AutomationId,
    matcher: EventMatcher,
    action: Action,
    run_as: RunAs,
    delegation: Delegation,
    budget: Budget,
    gates: Vec<Gate>,
) -> AutomationRule {
    AutomationRule {
        rule_id,
        matcher,
        action,
        run_as,
        delegation,
        budget,
        gates,
    }
}

/// A durable workflow handle returned by [`DurableExecutor::start`] (contract 9.1 — `start`
/// returns a durable handle). Opaque to the Automation engine; the dispatch tier carries it into
/// the automation-firing audit so the durable run is correlated with the trigger.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct DurableHandle(pub String);

/// An error starting a durable workflow (the `myelin-flow` seam's failure). A `start` failure is
/// surfaced as [`Outcome::WorkflowStartFailed`] — never swallowed, never a silent no-op (a
/// reflex whose workflow could not start is observable so it can be retried/alerted).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutorError(pub String);

/// **The CONSUMED `myelin-flow` durable-executor seam** (contract 9.1 — `DurableExecutor{start,
/// signal, describe, cancel}`). This is the ONLY part of 9.1 the Automation engine needs:
/// `start` (begin a durable workflow run). The REAL implementation is the downstream
/// `myelin-flow` crate (the named floor, **P-FLOW-04 / P-199 + P-203**); this crate is UPSTREAM
/// of it in the §2.9 DAG, so it depends on this trait, never on `myelin-flow` directly — exactly
/// the DAG-respecting seam pattern [`crate::matcher`] uses for the `list_objects` push-down and
/// `myelin_events::holder` uses for the KMS crypto-shred. The engine NEVER reinvents the durable
/// loop; `action.kind = workflow` is a single `start` call through this seam.
///
/// `idem_key` is the per-effect idempotency key (contract 9.1's frozen `idem_key` rule): a
/// redelivered trigger event that fires the same rule produces the SAME `idem_key`, so `start` is
/// effectively-once at the executor (a double-delivery is one workflow run, not two). The
/// Automation engine derives it `<rule_id>:<event_id>` (one firing per `(rule, event)`).
pub trait DurableExecutor {
    /// Start (or no-op-return-the-existing) a durable workflow run for `workflow_ref` with
    /// `input`, idempotent on `idem_key`. Returns the durable handle. A genuine failure (the
    /// engine is unreachable, the workflow_ref is unknown) is an [`ExecutorError`] — surfaced,
    /// never a silent drop (EI-02 §4: no fire-and-forget).
    fn start(
        &self,
        workflow_ref: &WorkflowRef,
        input: &serde_json::Value,
        idem_key: &str,
    ) -> Result<DurableHandle, ExecutorError>;
}

/// The deterministic in-memory [`DurableExecutor`] floor (for the unit/CDC tests + the EB-23
/// replay-determinism substrate). It records every `start` call (so a test can assert the
/// delegation happened — the workflow was INVOKED, not reinvented) and is **idempotent on
/// `idem_key`** (a redelivered firing returns the SAME handle, never starts a second run). The
/// real durable engine is `myelin-flow` (the named floor); this models exactly its `start`
/// semantics until then.
#[derive(Debug, Default)]
pub struct InMemoryExecutor {
    /// `idem_key → (workflow_ref, input, handle)` — the started runs, keyed by the idempotency
    /// key (so a redelivery is a no-op return of the existing handle). `RefCell` so `start` takes
    /// `&self` (the seam shape) while recording.
    started: std::cell::RefCell<std::collections::BTreeMap<String, StartedRun>>,
}

/// One recorded `start` on the [`InMemoryExecutor`] (the test-observable proof of delegation).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartedRun {
    pub workflow_ref: WorkflowRef,
    pub input: serde_json::Value,
    pub handle: DurableHandle,
}

impl InMemoryExecutor {
    /// A fresh executor with no started runs.
    pub fn new() -> InMemoryExecutor {
        InMemoryExecutor::default()
    }

    /// How many DISTINCT durable runs were started (distinct `idem_key`s). A redelivery does not
    /// increment this — the proof that `start` is effectively-once.
    pub fn started_count(&self) -> usize {
        self.started.borrow().len()
    }

    /// The recorded run for an `idem_key`, if any (a test asserts the workflow was invoked with
    /// the expected ref + input — delegation, not reinvention).
    pub fn run_for(&self, idem_key: &str) -> Option<StartedRun> {
        self.started.borrow().get(idem_key).cloned()
    }
}

impl DurableExecutor for InMemoryExecutor {
    fn start(
        &self,
        workflow_ref: &WorkflowRef,
        input: &serde_json::Value,
        idem_key: &str,
    ) -> Result<DurableHandle, ExecutorError> {
        let mut started = self.started.borrow_mut();
        if let Some(existing) = started.get(idem_key) {
            // Effectively-once: a redelivered firing returns the SAME handle (one run).
            return Ok(existing.handle.clone());
        }
        let handle = DurableHandle(format!("wf:{idem_key}"));
        started.insert(
            idem_key.to_string(),
            StartedRun {
                workflow_ref: workflow_ref.clone(),
                input: input.clone(),
                handle: handle.clone(),
            },
        );
        Ok(handle)
    }
}

/// **What one automation firing produced** (the per-event reflex result the dispatch tier acts
/// on). A firing is exactly-once per `(rule, event)`; this records what it did so the dispatch
/// tier (EB-23) can record the firing + carry the effect into the outbox.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The rule did NOT match this event (no firing). Carries the rule id for audit/observability.
    NoMatch { rule_id: AutomationId },
    /// The rule matched but a non-approval gate failed (fail-closed) — the action was SUPPRESSED.
    GateFailed { rule_id: AutomationId },
    /// The rule matched but the budget is exhausted — the firing was SHED (§4.7), not run.
    BudgetShed { rule_id: AutomationId },
    /// The rule matched and a `RequireHumanApproval` gate routed it to the approval lane — the
    /// action is HELD pending a human decision (the dispatch tier raises the HITL card; the
    /// durable approval rides a `myelin-flow` signal, 9.1). The action did NOT run inline.
    AwaitingApproval { rule_id: AutomationId },
    /// The rule fired and its `Emit` action yielded an outbox publish draft (publish-is-outbox-
    /// only; the dispatch tier emits it). Carries the draft the dispatch tier turns into
    /// `OutboxTx::emit`.
    Emitted {
        rule_id: AutomationId,
        draft: PublishDraft,
    },
    /// The rule fired and its `Workflow` action DELEGATED to `myelin-flow` (`DurableExecutor::start`
    /// succeeded). Carries the durable handle (the workflow was INVOKED, not reinvented).
    WorkflowStarted {
        rule_id: AutomationId,
        handle: DurableHandle,
    },
    /// The rule fired but the durable workflow `start` FAILED (the `myelin-flow` seam errored) —
    /// surfaced, never a silent no-op (so it can be retried/alerted).
    WorkflowStartFailed {
        rule_id: AutomationId,
        error: ExecutorError,
    },
    /// The rule was a redelivery of an already-fired `(rule, event_id)` — a NO-OP (effectively-once
    /// on `event_id`, the EB-06 dedup discipline). The action did NOT run a second time.
    AlreadyFired { rule_id: AutomationId },
}

/// The Budget accounting state for one rule (how many firings it has consumed). Held by the
/// engine; reset by the dispatch tier's accounting window (§4.7). This is NOT application state
/// (no per-person lifecycle) — it is the shed-counter that makes the reflex bounded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct BudgetState {
    firings: u64,
}

/// **The Automation engine** (Bus §3.5) — the stateless per-event reflex over the matcher. It is
/// built ON the [`EventMatcher`]: each rule's selector is a matcher, so the permission compose
/// (the 0-leak property, §4.5) rides through every match. It holds the rules + the idempotency
/// ledger (the fired-once guard, 2.5) + the per-rule budget counters — and NOTHING else (no
/// per-person promise, no armed→resolved lifecycle; that is the [`Trigger`], EB-20).
///
/// **Stateless reflex, by construction.** [`AutomationEngine::ingest`] is a pure function of
/// `(rules, event, visible-set, budget-state, dedup-ledger)` → [`Outcome`]s + side effects through
/// the [`DurableExecutor`] seam. The same event sequence replays to the same outcomes (what
/// BUS-D3 in EB-23 relies on). The store here is in-memory + deterministic; the durable
/// `automation_rule` table (architecture §3.5) is the dispatch tier's persistence concern.
#[derive(Debug, Default)]
pub struct AutomationEngine {
    rules: Vec<AutomationRule>,
    /// The effectively-once ledger: `(rule_id, event_id)` of every firing already made. A
    /// redelivered event whose `(rule, event_id)` is present is a NO-OP (2.5, the EB-06 dedup
    /// discipline modelled in-engine). NOT application state — the fired-once guard.
    fired: BTreeSet<(AutomationId, EventId)>,
    /// Per-rule budget counters (the shed accounting, §4.7).
    budgets: std::collections::BTreeMap<AutomationId, BudgetState>,
}

impl AutomationEngine {
    /// A fresh engine with no rules, an empty dedup ledger, and zero budget consumed.
    pub fn new() -> AutomationEngine {
        AutomationEngine::default()
    }

    /// Register a rule (contract 3.2 — `register_automation`). Returns `&mut self` for chaining.
    pub fn add_rule(&mut self, rule: AutomationRule) -> &mut AutomationEngine {
        self.rules.push(rule);
        self
    }

    /// How many firings a rule has consumed (the budget accounting; for tests + the dispatch
    /// tier's shed metric).
    pub fn firings(&self, rule_id: &AutomationId) -> u64 {
        self.budgets.get(rule_id).map(|b| b.firings).unwrap_or(0)
    }

    /// Whether a `(rule, event)` has already fired (the effectively-once ledger view).
    pub fn has_fired(&self, rule_id: &AutomationId, event_id: &EventId) -> bool {
        self.fired.contains(&(rule_id.clone(), event_id.clone()))
    }

    /// **The per-event reflex** (§3.5): for each rule, match (permission-aware) → idempotency
    /// guard → gates → budget → run the action (Emit ⇒ yield a draft; Workflow ⇒ DELEGATE to
    /// `myelin-flow`'s [`DurableExecutor::start`]). Returns one [`Outcome`] per rule (so the
    /// dispatch tier can audit every rule's decision for this event, not just the matches).
    ///
    /// **Idempotent on `event_id`** (the EB-06 dedup discipline): a redelivered event whose
    /// `(rule, event_id)` already fired yields [`Outcome::AlreadyFired`] and runs the action
    /// **zero** more times — the reflex fires exactly once per delivery.
    ///
    /// **Permission-aware BY CONSTRUCTION** (the 0-leak property): `visible` is the
    /// `list_objects(run_as, read, type)` [`SetExpr`] result (4.3) the matcher composes with; an
    /// event for an artifact the rule's `run_as` can't see NEVER fires the rule. `member_oracle`
    /// answers the relational `SetExpr` arms (the consumer's authz reverse-index lookup).
    ///
    /// The action runs under `run_as + delegation` within `budget + gates` — all four honoured:
    /// the `run_as` identity is carried into the workflow `idem_key`/audit; `delegation` rides on
    /// the rule (the monotone-narrowing intersection is the dispatch tier's authz call, 4.5);
    /// `budget` sheds an over-budget firing; `gates` fail-close (or route to the approval lane).
    pub fn ingest(
        &mut self,
        envelope: &EventEnvelope,
        visible: &SetExpr,
        member_oracle: &dyn Fn(&RelMembership) -> bool,
        executor: &dyn DurableExecutor,
    ) -> Vec<Outcome> {
        let rules = self.rules.clone();
        let mut outcomes = Vec::with_capacity(rules.len());
        for rule in &rules {
            outcomes.push(self.fire_one(rule, envelope, visible, member_oracle, executor));
        }
        outcomes
    }

    /// Evaluate ONE rule against the event — the inner reflex (factored out so `ingest` is the
    /// thin per-rule loop). Order is load-bearing: match (permission-aware, 0-leak) → idempotency
    /// → gates (fail-closed; approval routes out) → budget (shed) → action.
    fn fire_one(
        &mut self,
        rule: &AutomationRule,
        envelope: &EventEnvelope,
        visible: &SetExpr,
        member_oracle: &dyn Fn(&RelMembership) -> bool,
        executor: &dyn DurableExecutor,
    ) -> Outcome {
        // (1) MATCH the selector — permission-aware BY CONSTRUCTION (the 0-leak property rides
        // through EventMatcher::matches: an unviewable subject returns false with the predicate
        // never consulted). A mis-authored predicate that errors is treated as NO MATCH
        // (fail-closed) — never a silent fire, never a panic.
        let matched = rule
            .matcher
            .matches(envelope, visible, member_oracle)
            .unwrap_or(false);
        if !matched {
            return Outcome::NoMatch {
                rule_id: rule.rule_id.clone(),
            };
        }

        // (2) IDEMPOTENCY GUARD (effectively-once on event_id, 2.5 / EB-06). A redelivered
        // `(rule, event_id)` fires ZERO more times.
        let ledger_key = (rule.rule_id.clone(), envelope.event_id.clone());
        if self.fired.contains(&ledger_key) {
            return Outcome::AlreadyFired {
                rule_id: rule.rule_id.clone(),
            };
        }

        // (3) GATES — fail-closed. A non-approval gate that does not hold SUPPRESSES the firing; a
        // `RequireHumanApproval` gate routes it to the approval lane (held, not run inline). We
        // check the inline predicates first; if any fails, the action does not run.
        for gate in &rule.gates {
            if !gate.passes_inline(envelope) {
                return Outcome::GateFailed {
                    rule_id: rule.rule_id.clone(),
                };
            }
        }
        // A human-approval gate routes OUT (suppress-inline-and-escalate) — the action is held.
        // We mark the firing as consumed (it is "the firing for this event", just one whose
        // action awaits approval) so a redelivery does not re-raise a second approval card.
        if rule.gates.iter().any(Gate::is_approval) {
            self.fired.insert(ledger_key);
            return Outcome::AwaitingApproval {
                rule_id: rule.rule_id.clone(),
            };
        }

        // (4) BUDGET — shed an over-budget firing (§4.7), do not run it.
        let state = self.budgets.entry(rule.rule_id.clone()).or_default();
        if state.firings >= rule.budget.max_firings {
            return Outcome::BudgetShed {
                rule_id: rule.rule_id.clone(),
            };
        }

        // The firing is committed: charge the budget + mark the dedup ledger BEFORE running the
        // action so a re-entrant redelivery cannot double-fire (the action's effect — an outbox
        // draft or a durable `start` — is itself idempotent: the draft is co-committed with this
        // ledger row by the dispatch tier; the `start` is idempotent on the derived idem_key).
        state.firings += 1;
        self.fired.insert(ledger_key);

        // (5) ACTION — run under run_as + delegation (carried on the rule; the dispatch tier's
        // authz call applies the monotone intersection, 4.5). Emit ⇒ yield a draft; Workflow ⇒
        // DELEGATE to myelin-flow (DurableExecutor::start) — never reinvented here.
        match &rule.action.kind {
            ActionKind::Emit { emit_type, subject } => Outcome::Emitted {
                rule_id: rule.rule_id.clone(),
                draft: self.emit_draft(rule, envelope, emit_type, subject),
            },
            ActionKind::Workflow {
                workflow_ref,
                input,
            } => {
                // The per-effect idempotency key (9.1's frozen idem_key rule): one workflow run
                // per (rule, event) — a redelivery (were the ledger reset) maps to the same key,
                // so the executor is effectively-once too. run_as is folded in so two rules with
                // distinct identities firing on the same event are distinct runs.
                let idem_key = format!(
                    "{}:{}:{}",
                    rule.rule_id.0, rule.run_as.0 .0, envelope.event_id.0
                );
                match executor.start(workflow_ref, input, &idem_key) {
                    Ok(handle) => Outcome::WorkflowStarted {
                        rule_id: rule.rule_id.clone(),
                        handle,
                    },
                    Err(error) => Outcome::WorkflowStartFailed {
                        rule_id: rule.rule_id.clone(),
                        error,
                    },
                }
            }
        }
    }

    /// Build the outbox [`PublishDraft`] for an `Emit` action. The draft reuses the [`Signal`]
    /// carrier shape (the engine yields a draft; the dispatch tier turns it into
    /// `OutboxTx::emit`) — references-not-payloads, the subject + emit-type only, never a PII
    /// body. The publish subject is the automation's derived-event subject; the dispatch tier maps
    /// it to the real outbox row.
    fn emit_draft(
        &self,
        rule: &AutomationRule,
        envelope: &EventEnvelope,
        emit_type: &str,
        subject: &ArtifactRef,
    ) -> PublishDraft {
        // The Emit effect rides the same PublishDraft carrier the SignalEngine yields (one
        // outbox-draft shape across the reactive tier); `subject` (the publish subject string) is
        // the derived event type so the dispatch tier knows what to emit.
        PublishDraft {
            subject: emit_type.to_string(),
            signal: Signal {
                rule_id: crate::RuleId(rule.rule_id.0.clone()),
                tenant: envelope.tenant.clone(),
                severity: Severity::Info,
                dedup_key: crate::DedupKey(format!("{}:{}", rule.rule_id.0, envelope.event_id.0)),
                subject: subject.clone(),
                count: 1,
                state: SignalState::Open,
                first_seen: envelope.recorded_at.0.clone(),
                last_seen: envelope.recorded_at.0.clone(),
            },
            kind: PublishKind::Opened,
        }
    }

    /// Reset the per-rule budget accounting (the dispatch tier's accounting-window roll, §4.7).
    /// Does NOT clear the dedup ledger (effectively-once is per-event, forever within retention).
    pub fn reset_budgets(&mut self) {
        self.budgets.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CmpOp, Expr, Predicate};
    use myelin_events::{
        Actor, AggregateKey, CorrelationId, DataRole, EventType, Timestamp, Visibility,
    };
    use myelin_identity::{Literal, ObjectType, Principal, PrincipalKind};
    use myelin_tenancy::{Region, TenantId};

    fn var(name: &str) -> Expr {
        Expr::Var(name.into())
    }
    fn str_(s: &str) -> Expr {
        Expr::Lit(Literal::Str(s.into()))
    }

    fn principal() -> Principal {
        Principal::stub(
            PrincipalId("svc-bot".into()),
            PrincipalKind::Human,
            TenantId("t1".into()),
        )
    }

    /// `event.type == <type>` matcher over the given object type.
    fn type_matcher(object_type: &str, type_: &str) -> EventMatcher {
        EventMatcher::compile(
            ObjectType(object_type.into()),
            Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var("event.type"),
                rhs: str_(type_),
            },
        )
        .unwrap()
    }

    fn envelope(type_: &str, id: &str, event_id: &str) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId(event_id.into()),
            type_: EventType(type_.into()),
            schema_ver: 1,
            tenant: TenantId("t1".into()),
            region: Region("fr-par".into()),
            actor: Actor(principal()),
            subject: ArtifactRef(format!("myelin://t1/issues/issue/{id}")),
            aggregate: AggregateKey(format!("issue:{id}")),
            causation_id: None,
            correlation_id: CorrelationId("root".into()),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-20T00:00:00Z".into()),
            payload: serde_json::json!({}),
        }
    }

    /// The "see nothing relationally" oracle — the simple tests drive visibility via the SetExpr.
    fn no_rel(_m: &RelMembership) -> bool {
        false
    }

    fn emit_rule(rule_id: &str, on_type: &str) -> AutomationRule {
        register_automation(
            AutomationId(rule_id.into()),
            type_matcher("issue", on_type),
            Action {
                kind: ActionKind::Emit {
                    emit_type: "issues.issue.labelled".into(),
                    subject: ArtifactRef("myelin://t1/issues/issue/PROJ-1".into()),
                },
            },
            RunAs(PrincipalId("svc-bot".into())),
            Delegation::none(),
            Budget::unbounded_within(100),
            vec![],
        )
    }

    fn workflow_rule(rule_id: &str, on_type: &str) -> AutomationRule {
        register_automation(
            AutomationId(rule_id.into()),
            type_matcher("issue", on_type),
            Action {
                kind: ActionKind::Workflow {
                    workflow_ref: WorkflowRef("escalate_incident".into()),
                    input: serde_json::json!({ "ref": "myelin://t1/issues/issue/PROJ-1" }),
                },
            },
            RunAs(PrincipalId("svc-bot".into())),
            Delegation::none(),
            Budget::unbounded_within(100),
            vec![],
        )
    }

    /// Find the single non-`NoMatch` outcome for `rule_id` (the rule that actually decided).
    fn outcome_for<'a>(outs: &'a [Outcome], rule_id: &str) -> &'a Outcome {
        outs.iter()
            .find(|o| match o {
                // NoMatch is not a "deciding" outcome for the purposes of these tests — we want the
                // rule whose action actually ran / was suppressed / shed / held / deduped.
                Outcome::NoMatch { .. } => false,
                Outcome::GateFailed { rule_id: r }
                | Outcome::BudgetShed { rule_id: r }
                | Outcome::AwaitingApproval { rule_id: r }
                | Outcome::Emitted { rule_id: r, .. }
                | Outcome::WorkflowStarted { rule_id: r, .. }
                | Outcome::WorkflowStartFailed { rule_id: r, .. }
                | Outcome::AlreadyFired { rule_id: r } => r.0 == rule_id,
            })
            .expect("a deciding outcome for the rule")
    }

    /// **GATE: a matching event fires the automation (and a non-matching one does not).** The
    /// mandatory-core match-fires test.
    #[test]
    fn matching_event_fires_automation_non_matching_does_not() {
        let mut engine = AutomationEngine::new();
        engine.add_rule(emit_rule("label_on_create", "issues.issue.created"));
        let exec = InMemoryExecutor::new();

        // A matching event fires (Emit ⇒ a draft).
        let matching = envelope("issues.issue.created", "PROJ-1", "evt-1");
        let outs = engine.ingest(&matching, &SetExpr::All, &no_rel, &exec);
        assert!(
            matches!(
                outcome_for(&outs, "label_on_create"),
                Outcome::Emitted { .. }
            ),
            "a matching event fires the automation"
        );

        // A non-matching event does NOT fire.
        let other = envelope("issues.issue.transitioned", "PROJ-2", "evt-2");
        let outs2 = engine.ingest(&other, &SetExpr::All, &no_rel, &exec);
        assert!(
            matches!(&outs2[0], Outcome::NoMatch { .. }),
            "a non-matching event does not fire the automation"
        );
    }

    /// **GATE: `action.kind = workflow` DELEGATES to the `myelin-flow` `DurableExecutor` (invoked,
    /// not reinvented).** The engine calls `DurableExecutor::start`; the executor records the run.
    #[test]
    fn workflow_action_delegates_to_durable_executor() {
        let mut engine = AutomationEngine::new();
        engine.add_rule(workflow_rule("escalate", "issues.issue.created"));
        let exec = InMemoryExecutor::new();

        let env = envelope("issues.issue.created", "PROJ-1", "evt-wf-1");
        let outs = engine.ingest(&env, &SetExpr::All, &no_rel, &exec);

        // The action started a durable workflow (delegation succeeded).
        match outcome_for(&outs, "escalate") {
            Outcome::WorkflowStarted { handle, .. } => {
                assert_eq!(
                    handle,
                    &DurableHandle("wf:escalate:svc-bot:evt-wf-1".into())
                );
            }
            other => panic!("expected WorkflowStarted, got {other:?}"),
        }
        // The executor was actually invoked — the workflow was started exactly once with the
        // rule's workflow_ref + input (DELEGATED to myelin-flow, NOT reinvented in the engine).
        assert_eq!(exec.started_count(), 1, "exactly one durable run started");
        let run = exec
            .run_for("escalate:svc-bot:evt-wf-1")
            .expect("the run is recorded");
        assert_eq!(run.workflow_ref, WorkflowRef("escalate_incident".into()));
        assert_eq!(
            run.input,
            serde_json::json!({ "ref": "myelin://t1/issues/issue/PROJ-1" })
        );
    }

    /// **GATE: a matching event fires the automation EXACTLY ONCE per delivery (idempotent on
    /// `event_id`, the EB-06 dedup discipline).** A redelivered event is a NO-OP.
    #[test]
    fn fires_exactly_once_per_event_id_redelivery_is_a_noop() {
        let mut engine = AutomationEngine::new();
        engine.add_rule(workflow_rule("escalate", "issues.issue.created"));
        let exec = InMemoryExecutor::new();

        let env = envelope("issues.issue.created", "PROJ-1", "evt-dup");
        // First delivery → fires, starts the workflow.
        let first = engine.ingest(&env, &SetExpr::All, &no_rel, &exec);
        assert!(matches!(
            outcome_for(&first, "escalate"),
            Outcome::WorkflowStarted { .. }
        ));
        assert!(engine.has_fired(&AutomationId("escalate".into()), &EventId("evt-dup".into())));

        // Redelivery of the SAME event_id → NO-OP (already fired); the action runs ZERO more times.
        let second = engine.ingest(&env, &SetExpr::All, &no_rel, &exec);
        assert!(
            matches!(
                outcome_for(&second, "escalate"),
                Outcome::AlreadyFired { .. }
            ),
            "a redelivered event is a no-op (effectively-once on event_id)"
        );
        assert_eq!(
            exec.started_count(),
            1,
            "the workflow started exactly once across the redelivery"
        );
        assert_eq!(engine.firings(&AutomationId("escalate".into())), 1);
    }

    /// **The automation is permission-aware BY CONSTRUCTION (0-leak)**: an event for an artifact
    /// the rule's `run_as` cannot see (`visible = None`) NEVER fires the rule.
    #[test]
    fn unviewable_subject_never_fires_the_automation() {
        let mut engine = AutomationEngine::new();
        engine.add_rule(emit_rule("label_on_create", "issues.issue.created"));
        let exec = InMemoryExecutor::new();
        let env = envelope("issues.issue.created", "PROJ-1", "evt-hidden");
        let outs = engine.ingest(&env, &SetExpr::None, &no_rel, &exec);
        assert!(
            matches!(&outs[0], Outcome::NoMatch { .. }),
            "an unviewable subject never fires (0-leak, the permission compose rides through)"
        );
    }

    /// **The automation honours `run_as`**: the run-as identity is folded into the workflow
    /// idem_key + handle, so two rules with DISTINCT run-as identities firing on the SAME event
    /// are DISTINCT durable runs (a reflex acts as its project principal, never the user's).
    #[test]
    fn run_as_identity_scopes_the_firing() {
        let mut engine = AutomationEngine::new();
        let mut bot_rule = workflow_rule("escalate", "issues.issue.created");
        bot_rule.run_as = RunAs(PrincipalId("svc-bot".into()));
        let mut ops_rule = workflow_rule("escalate_ops", "issues.issue.created");
        ops_rule.run_as = RunAs(PrincipalId("svc-ops".into()));
        engine.add_rule(bot_rule);
        engine.add_rule(ops_rule);
        let exec = InMemoryExecutor::new();

        let env = envelope("issues.issue.created", "PROJ-1", "evt-runas");
        let outs = engine.ingest(&env, &SetExpr::All, &no_rel, &exec);
        // Both fired, but as DISTINCT runs (distinct run_as → distinct idem_key).
        assert!(matches!(
            outcome_for(&outs, "escalate"),
            Outcome::WorkflowStarted { .. }
        ));
        assert!(matches!(
            outcome_for(&outs, "escalate_ops"),
            Outcome::WorkflowStarted { .. }
        ));
        assert_eq!(
            exec.started_count(),
            2,
            "distinct run_as identities → distinct durable runs"
        );
        assert!(exec.run_for("escalate:svc-bot:evt-runas").is_some());
        assert!(exec.run_for("escalate_ops:svc-ops:evt-runas").is_some());
    }

    /// **The automation honours `budget`**: a runaway rule is SHED past `max_firings`, not run.
    #[test]
    fn budget_sheds_over_budget_firings() {
        let mut engine = AutomationEngine::new();
        let mut rule = emit_rule("label", "issues.issue.created");
        rule.budget = Budget {
            max_firings: 2,
            cost_units: 1,
        };
        engine.add_rule(rule);
        let exec = InMemoryExecutor::new();

        // Three DISTINCT events (distinct event_id → not deduped); the budget caps firings at 2.
        let mut emitted = 0;
        let mut shed = 0;
        for i in 0..3 {
            let env = envelope("issues.issue.created", "PROJ-1", &format!("evt-budget-{i}"));
            let outs = engine.ingest(&env, &SetExpr::All, &no_rel, &exec);
            match outcome_for(&outs, "label") {
                Outcome::Emitted { .. } => emitted += 1,
                Outcome::BudgetShed { .. } => shed += 1,
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(emitted, 2, "exactly max_firings firings ran");
        assert_eq!(shed, 1, "the over-budget firing was shed, not run");
        assert_eq!(engine.firings(&AutomationId("label".into())), 2);
    }

    /// **The automation honours `gates`**: a `RequireNoPersonalData` gate fail-closes a firing on a
    /// PII-flagged event (the reflex never propagates inline PII).
    #[test]
    fn gate_fail_closes_on_personal_data() {
        let mut engine = AutomationEngine::new();
        let mut rule = emit_rule("label", "issues.issue.created");
        rule.gates = vec![Gate::RequireNoPersonalData];
        engine.add_rule(rule);
        let exec = InMemoryExecutor::new();

        // A PII-flagged event → the gate fails → the action is SUPPRESSED.
        let mut env = envelope("issues.issue.created", "PROJ-1", "evt-pii");
        env.contains_personal_data = true;
        let outs = engine.ingest(&env, &SetExpr::All, &no_rel, &exec);
        assert!(
            matches!(outcome_for(&outs, "label"), Outcome::GateFailed { .. }),
            "a gate that does not hold suppresses the firing (fail-closed)"
        );

        // A non-PII event of the same type → the gate passes → the action fires.
        let clean = envelope("issues.issue.created", "PROJ-2", "evt-clean");
        let outs2 = engine.ingest(&clean, &SetExpr::All, &no_rel, &exec);
        assert!(matches!(
            outcome_for(&outs2, "label"),
            Outcome::Emitted { .. }
        ));
    }

    /// **A `RequireHumanApproval` gate routes the firing to the approval lane (held, not run
    /// inline)** — and a redelivery does not re-raise a second approval card.
    #[test]
    fn human_approval_gate_routes_to_approval_lane() {
        let mut engine = AutomationEngine::new();
        let mut rule = workflow_rule("escalate", "issues.issue.created");
        rule.gates = vec![Gate::RequireHumanApproval];
        engine.add_rule(rule);
        let exec = InMemoryExecutor::new();

        let env = envelope("issues.issue.created", "PROJ-1", "evt-approve");
        let outs = engine.ingest(&env, &SetExpr::All, &no_rel, &exec);
        assert!(
            matches!(
                outcome_for(&outs, "escalate"),
                Outcome::AwaitingApproval { .. }
            ),
            "a human-approval gate holds the action for a human decision"
        );
        // The action did NOT run inline — no durable workflow was started.
        assert_eq!(
            exec.started_count(),
            0,
            "the action is held, not run inline"
        );
        // A redelivery is a no-op (the firing was recorded so no second card is raised).
        let again = engine.ingest(&env, &SetExpr::All, &no_rel, &exec);
        assert!(matches!(
            outcome_for(&again, "escalate"),
            Outcome::AlreadyFired { .. }
        ));
    }

    /// **The causal-depth gate suppresses a too-deep firing (the D-6 self-trigger guard).**
    #[test]
    fn causal_depth_gate_suppresses_deep_firing() {
        let mut engine = AutomationEngine::new();
        let mut rule = emit_rule("label", "issues.issue.created");
        rule.gates = vec![Gate::MaxCausalDepth(3)];
        engine.add_rule(rule);
        let exec = InMemoryExecutor::new();

        let shallow = envelope("issues.issue.created", "PROJ-1", "evt-shallow");
        assert!(matches!(
            outcome_for(
                &engine.ingest(&shallow, &SetExpr::All, &no_rel, &exec),
                "label"
            ),
            Outcome::Emitted { .. }
        ));

        let mut deep = envelope("issues.issue.created", "PROJ-2", "evt-deep");
        deep.depth = 7;
        assert!(
            matches!(
                outcome_for(
                    &engine.ingest(&deep, &SetExpr::All, &no_rel, &exec),
                    "label"
                ),
                Outcome::GateFailed { .. }
            ),
            "a firing past the causal-depth ceiling is suppressed (the self-trigger guard)"
        );
    }

    /// **A `start` failure is SURFACED (never a silent no-op)** — a reflex whose workflow could
    /// not start is observable so the dispatch tier can retry/alert.
    #[test]
    fn workflow_start_failure_is_surfaced() {
        struct FailingExec;
        impl DurableExecutor for FailingExec {
            fn start(
                &self,
                _w: &WorkflowRef,
                _i: &serde_json::Value,
                _k: &str,
            ) -> Result<DurableHandle, ExecutorError> {
                Err(ExecutorError("myelin-flow unreachable".into()))
            }
        }
        let mut engine = AutomationEngine::new();
        engine.add_rule(workflow_rule("escalate", "issues.issue.created"));
        let env = envelope("issues.issue.created", "PROJ-1", "evt-fail");
        let outs = engine.ingest(&env, &SetExpr::All, &no_rel, &FailingExec);
        assert!(
            matches!(
                outcome_for(&outs, "escalate"),
                Outcome::WorkflowStartFailed { .. }
            ),
            "a start failure is surfaced, never swallowed"
        );
    }

    /// **The reflex is replay-deterministic**: the same event sequence yields the same outcomes
    /// (what BUS-D3 in EB-23 relies on). Two independent engines fed the same stream agree.
    #[test]
    fn ingest_is_replay_deterministic() {
        let stream: Vec<EventEnvelope> = (0..5)
            .map(|i| envelope("issues.issue.created", "PROJ-1", &format!("evt-{i}")))
            .collect();
        let run = || {
            let mut e = AutomationEngine::new();
            e.add_rule(emit_rule("label", "issues.issue.created"));
            let exec = InMemoryExecutor::new();
            let mut all = Vec::new();
            for env in &stream {
                all.extend(e.ingest(env, &SetExpr::All, &no_rel, &exec));
            }
            all
        };
        assert_eq!(
            run(),
            run(),
            "the same stream → the same outcomes (deterministic)"
        );
    }

    /// **`AutomationRule` round-trips stably (the wire contract — the durable `automation_rule`
    /// row).** The `matcher` field is the byte-identical `QueryAst` (no drift).
    #[test]
    fn automation_rule_round_trips_stably() {
        let rule = workflow_rule("escalate", "issues.issue.created");
        let json = serde_json::to_string(&rule).unwrap();
        let back: AutomationRule = serde_json::from_str(&json).unwrap();
        assert_eq!(rule, back);
    }

    /// **`InMemoryExecutor` is idempotent on `idem_key`** (the 9.1 per-effect idem_key rule): two
    /// `start`s with the same key return the SAME handle and start ONE run.
    #[test]
    fn executor_start_is_idempotent_on_idem_key() {
        let exec = InMemoryExecutor::new();
        let w = WorkflowRef("w".into());
        let i = serde_json::json!({});
        let h1 = exec.start(&w, &i, "k").unwrap();
        let h2 = exec.start(&w, &i, "k").unwrap();
        assert_eq!(h1, h2, "same idem_key → same handle (effectively-once)");
        assert_eq!(exec.started_count(), 1, "one run for one idem_key");
    }
}
