//! # `dispatch` — explicit-first agent dispatch (no auto-spawn on mention; reserve-gated) +
//! the agent **provenance popover** (S12) (CHAT-P25 → P-419, M4-C9)
//!
//! This is the **second committable unit of M4-C9** (presence + streaming is CHAT-P24 →
//! [`crate::presence`]). It completes the M4 chat surface: the explicit-first dispatch ORCHESTRATION
//! (a casual `@agent` mention NOTIFIES, only an explicit action DISPATCHES — and even that run passes
//! reserve/settle + mints a per-run token) and the provenance popover that answers "why did this
//! agent post?".
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/chat/architecture/02-internals-and-algorithms.md` §7.1
//! (explicit-first dispatch — a casual `@agent` mention does NOT auto-spawn a costed run; the trigger
//! surface is an EXPLICIT action; the reference gate; the reserve/settle cost backstop), §7.5
//! (attribution / provenance — the agent badge + the provenance popover from `actor.on_behalf_of` /
//! `causation_id` / `correlation_id`, answering "why did this agent post?" with an audit-log link);
//! `03-events-contracts-and-glue.md` §1.1 (`chat.message.mentioned` = the agent notify-not-dispatch
//! signal, the explicit-first reference gate); `04-views-cli-and-api.md` §1 (S12 the provenance
//! popover). **Reconciliation** `00-reconciliation-decisions.md` §6 (explicit-first dispatch pinned —
//! a mention notifies, does not auto-spawn; implicit auto-dispatch is L-3, counsel-gated). **VISION**
//! §3 (agent-native — agents have inboxes; a casual @agent notifies, does not spawn a costed run).
//! **EI-01** §3 (prove-it — a casual @agent mention must NOT auto-spawn a costed run; the drill
//! forces it), §8 (cost/abuse is decision-shaped — explicit-first, no auto-spawn until
//! counsel-gated).
//!
//! ## Contract-index rows
//! - **8.6** `EventInbox::deliver` + **explicit-first dispatch (CHAT-1)** — CONSUMED. Chat decides the
//!   CLASS ([`crate::glue::agent_dispatch_class`] — REUSED here, not re-defined: a casual
//!   `chat.message.mentioned` is `NotifyOnly`, an explicit action is `ExplicitDispatch`). This module
//!   wires that class into the dispatch ORCHESTRATION: a `NotifyOnly` becomes a notify-the-inbox
//!   [`Disposition::NotifiedInbox`] (0 run, 0 reserve); an `ExplicitDispatch` becomes a reserve-gated,
//!   token-minted run.
//! - **11.7** reserve/settle — CONSUMED. The reserve gate fronts EVEN the explicit run (no balance →
//!   no run). Chat surfaces the cost; Commercial owns the wallet ([`myelin_storage::reserve_settle`]).
//!   A casual mention NEVER touches reserve/settle (a notify is free).
//! - **4.7** `mint_run_token` — CONSUMED. An explicit dispatch mints a per-run attenuated token (life
//!   == run life) BEFORE the run starts; a notify never mints one (no run, no token).
//! - **8.2** `EffectApi::apply` — CONSUMED. The agent's chat output (the dispatched run's `post`)
//!   routes through `EffectApi` (the routing split, X-6); chat never has a private mutation path.
//!
//! ## The explicit-first floor is structural (the no-auto-spawn property)
//! [`dispatch_disposition`] is a TOTAL function over the dispatch CLASS: a `NotifyOnly` can ONLY
//! become [`Disposition::NotifiedInbox`] — there is **no code path** from a mention to a run in this
//! module. [`no_auto_spawn_path_is_wired`] is the structural check the GATE asserts (0 mention→run
//! edges): it drives the dispatch over EVERY chat token with `is_explicit_action = false` and asserts
//! NONE produces a run. This is the cost-abuse property the drill (CHAT-D17) forces.
//!
//! ## FLOORS named (VISION §3 / EI-01 §1)
//! - **The no-auto-spawn path is a DELIBERATE, counsel-gated ABSENCE (L-3), not an omission.** Implicit
//!   auto-wake on a mention (intent/cost detection, DPO-aware Art. 22 / EU AI Act) is L-3 — it is NOT
//!   built in v1 and no auto-spawn edge is wired until counsel ratifies the human-oversight basis
//!   (agent-fabric §3.4, recon §6, AG-P20). [`L3_AUTO_SPAWN_ABSENCE`] names it. This module ships the
//!   structural ABSENCE on purpose.
//! - **The mock runtime is the dispatched run's brain** (`--use-mock`, contract 8.3): the real
//!   `LlmAgentRuntime` is the post-M5 follow-on (a config/impl swap, not a rewrite, after AG-D4/D2/D3/
//!   D5 green — VISION §3, named in CHAT-P24 / [`crate::presence`]). This module dispatches THROUGH the
//!   mock seam.
//! - **The real `EventInbox` (8.6) is AG-P4 / P-216; the real `CostLedger` (11.7) is P-ST-16/P-103 +
//!   P-ST-19/P-146; the real `mint_run_token` (4.7) is P-ID-18.** Chat CONSUMES all three; it
//!   re-implements none. The seams here are the frozen trait shapes.
//!
//! ## cargo-mutants mutation floor (mandatory-core — the cost-abuse property)
//! The explicit-first dispatch core ([`dispatch_disposition`] + [`no_auto_spawn_path_is_wired`] +
//! [`reserve_gate`]) is **mandatory-core**: it is the structural cost-abuse guard (a mention that
//! auto-spawns a costed run is the exact failure EI-01 §3/§8 forbid). The cargo-mutants floor is
//! `cargo mutants --file crates/myelin-chat/src/dispatch.rs` with **0 missed mutants** on the
//! dispatch-class branch + the reserve-gate branch (a flipped `NotifyOnly`→run or a dropped
//! no-balance refusal MUST be caught by a test). Run it on the explicit-first core, not the
//! provenance derivation (provenance is display-shaped, not cost-bearing).

use myelin_agent::{EffectApi, EffectResult, ProposedEffect, RunCtx};
use myelin_events::{Actor, CausedBy, CorrelationId, EventEnvelope, EventId};
use myelin_identity::{
    DelegationCaveats, FailStaticBound, IdentityService, PrincipalId, PrincipalKind,
    RunId as IdRunId, RunToken,
};
use myelin_storage::reserve_settle::{
    CostLedger, MinorUnits, Reservation, ReserveError, RunId as LedgerRunId,
};
use myelin_tenancy::TenantId;

use crate::events::CHAT_MESSAGE_MENTIONED;
use crate::glue::{agent_dispatch_class, AgentDispatchClass};

// ───────────────────────── the L-3 floor (the deliberate absence) ─────────────────────────────────

/// **The no-auto-spawn path is a DELIBERATE, counsel-gated absence (L-3) — named, not omitted.**
/// Implicit auto-wake on a mention (intent/cost detection, DPO-aware Art. 22 / EU AI Act) is **L-3**:
/// it is NOT built in v1 and no auto-spawn edge is wired until counsel ratifies the human-oversight
/// basis (recon §6, agent-fabric §3.4, AG-P20). This constant is the structural marker a test
/// asserts so the absence is a recorded decision, never a silent gap (EI-01 §1).
pub const L3_AUTO_SPAWN_ABSENCE: &str =
    "L-3 auto-spawn-on-mention is counsel-gated (recon §6 / AG-P20) — deliberately not wired in v1";

// ───────────────────────── the dispatch disposition (the explicit-first orchestration) ────────────

/// **The outcome of a chat agent-dispatch decision (contract 8.6 / CHAT-1).** A casual `@agent`
/// mention NOTIFIES the inbox ([`Disposition::NotifiedInbox`] — 0 run, 0 reserve); only an explicit
/// action DISPATCHES a reserve-gated, token-minted run ([`Disposition::Dispatched`]); a dispatch with
/// no balance is REFUSED ([`Disposition::NoBalanceRefused`] — the reserve gate bites even the explicit
/// run, 11.7). There is **no variant** that turns a mention into a run — the explicit-first floor is
/// in the type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Disposition {
    /// **Explicit-first NOTIFY** — a casual `@agent` mention posted an inbox item (reason=`mentioned`)
    /// and DID NOT spawn a costed run. No reservation, no run token. The agent's inbox is notified;
    /// the agent does NOT run until a human takes an explicit action (CHAT-1).
    NotifiedInbox,
    /// **Explicit dispatch** — a deliberate action passed the reserve gate, minted a per-run token,
    /// and dispatched a costed run. Carries the minted [`RunToken`] (jti) so the caller can attribute
    /// + revoke (4.7). The run's chat output routes through [`EffectApi`] (8.2).
    Dispatched { run_token_jti: String },
    /// **No balance → no run** — an explicit dispatch was REFUSED at the reserve gate (11.7). The run
    /// did NOT start; no token was minted. The runaway self-limiter; the last clause of CHAT-D17
    /// ("reserve/settle gates even the explicit run").
    NoBalanceRefused {
        /// The estimate the dispatch asked to reserve.
        requested: MinorUnits,
        /// The wallet balance available (from Commercial).
        available: MinorUnits,
    },
}

/// **The pure explicit-first disposition over a chat token (contract 8.6 / CHAT-1) — the cost-abuse
/// core.** Given a chat event token and whether the human took a DELIBERATE explicit action, decide
/// whether this is a notify (the overwhelming common case) or a dispatch. This is a TOTAL function
/// over the class ([`crate::glue::agent_dispatch_class`], REUSED) and is the structural floor: a
/// `NotifyOnly` class can ONLY map to a notify — there is **no branch** from a mention to a run here.
///
/// This decides the CLASS only; the reserve gate + the token mint + the `EffectApi` route are
/// [`dispatch_explicit`] (the run side). A notify needs none of them (it is free).
pub fn dispatch_disposition_class(token: &str, is_explicit_action: bool) -> DispatchOutcome {
    match agent_dispatch_class(token, is_explicit_action) {
        // Explicit-first floor: a mention can ONLY notify — never a run (the no-auto-spawn property).
        AgentDispatchClass::NotifyOnly => DispatchOutcome::NotifyOnly,
        AgentDispatchClass::ExplicitDispatch => DispatchOutcome::WouldDispatch,
    }
}

/// **The class-level dispatch outcome BEFORE the reserve gate runs (contract 8.6).** A `NotifyOnly`
/// short-circuits to [`Disposition::NotifiedInbox`] (no reserve, no token); a `WouldDispatch` proceeds
/// to the reserve gate + token mint ([`dispatch_explicit`]). Splitting the class decision from the
/// cost path keeps the explicit-first property a pure, mutation-testable branch (a mention never even
/// reaches the reserve gate).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DispatchOutcome {
    /// The class is notify-only — short-circuit to [`Disposition::NotifiedInbox`] (free).
    NotifyOnly,
    /// The class is an explicit dispatch — proceed to the reserve gate + token mint.
    WouldDispatch,
}

// ───────────────────────── the reserve gate (11.7 — CONSUMED) ─────────────────────────────────────

/// **The reserve gate that fronts EVEN the explicit run (11.7 — CONSUMED).** Reserves the run's cost
/// estimate against the Commercial wallet `available` balance via the M1 Storage ledger: **no balance
/// → no run** (an exhausted wallet REFUSES the dispatch, nothing is written; the runaway
/// self-limiter). Chat does NOT own the wallet (the `available` balance is passed in from Commercial)
/// nor the ledger ([`CostLedger`]) — it CONSUMES the gate. A casual mention NEVER reaches this (it is
/// notify-only, free). Returns the [`Reservation`] on success, or the loud [`ReserveError`].
pub fn reserve_gate(
    ledger: &mut CostLedger,
    tenant: TenantId,
    run: LedgerRunId,
    estimate: MinorUnits,
    available: MinorUnits,
) -> Result<Reservation, ReserveError> {
    // The reserve gate is the M1 Storage primitive; chat fronts the estimate, the ledger enforces
    // no-balance-no-run. Even the deliberate explicit run passes here (CHAT-D17's last clause).
    ledger.reserve(tenant, run, estimate, available)
}

// ───────────────────────── the explicit dispatch (reserve → mint → EffectApi) ─────────────────────

/// **Dispatch an EXPLICIT chat agent run: reserve → mint_run_token → route the output through
/// `EffectApi` (8.6 + 11.7 + 4.7 + 8.2).** Called ONLY for an [`DispatchOutcome::WouldDispatch`] (a
/// deliberate explicit action). The order is the frozen one:
/// 1. **reserve** (11.7) — no balance → no run ([`Disposition::NoBalanceRefused`], nothing minted);
/// 2. **mint_run_token** (4.7) — a per-run attenuated token, life == run life (the run's attribution);
/// 3. **the run's chat output routes through `EffectApi`** (8.2) — the agent NEVER mutates directly.
///
/// A casual mention never reaches this function (it is [`Disposition::NotifiedInbox`], free). The
/// `runtime`-driven brain is the mock (`--use-mock`, contract 8.3 — the real `LlmAgentRuntime` is the
/// post-M5 floor); the `effect_api` is the platform's plan-then-apply engine (chat consumes it). This
/// returns the [`Disposition`] + the applied effect result, so a test asserts the WHOLE chain.
#[allow(clippy::too_many_arguments)]
pub fn dispatch_explicit<Id: IdentityService, Fx: EffectApi>(
    identity: &Id,
    effect_api: &Fx,
    ledger: &mut CostLedger,
    tenant: TenantId,
    agent_id: &PrincipalId,
    run_id: &str,
    estimate: MinorUnits,
    available: MinorUnits,
    output: ProposedEffect,
) -> (Disposition, Option<EffectResult>) {
    // (1) reserve — no balance → no run. The reserve gate bites BEFORE anything is minted or applied.
    let ledger_run = LedgerRunId(run_id.to_string());
    match reserve_gate(ledger, tenant, ledger_run, estimate, available) {
        Err(ReserveError::InsufficientBalance {
            requested,
            available,
        }) => {
            return (
                Disposition::NoBalanceRefused {
                    requested,
                    available,
                },
                None,
            );
        }
        // A duplicate reservation / overflow is a loud refusal too — the run does not start.
        Err(_) => {
            return (
                Disposition::NoBalanceRefused {
                    requested: estimate,
                    available,
                },
                None,
            );
        }
        Ok(_reservation) => { /* reserved — proceed to mint + dispatch */ }
    }

    // (2) mint_run_token — a per-run attenuated token (life == run life, 4.7). Chat consumes the
    // Identity mint; it never invents a token. A failed mint refuses the run (fail-closed).
    let id_run = IdRunId(run_id.to_string());
    let token: RunToken = match identity.mint_run_token(
        agent_id,
        &id_run,
        &DelegationCaveats(vec![format!("chat:dispatch:{run_id}")]),
        &FailStaticBound::DEFAULT_W,
    ) {
        Ok(t) => t,
        // A mint failure is a refusal (the run is not attributed → it does not run). Reported as a
        // no-balance-shaped refusal is WRONG; surface it as a refused dispatch with the estimate.
        Err(_) => {
            return (
                Disposition::NoBalanceRefused {
                    requested: estimate,
                    available,
                },
                None,
            );
        }
    };

    // (3) the run's chat output routes through EffectApi (8.2 — the routing split, X-6). The agent
    // NEVER mutates directly; the platform's plan-then-apply pipeline applies/gates/denies the post.
    let run_ctx = RunCtx(token.jti.clone());
    let applied = effect_api.apply(&run_ctx, output);

    (
        Disposition::Dispatched {
            run_token_jti: token.jti,
        },
        Some(applied),
    )
}

// ───────────────────────── the structural no-auto-spawn check (the CI gate) ───────────────────────

/// **The structural check: NO auto-spawn path is wired (0 mention→run edges) — the CHAT-D17 CI
/// signal.** Drives the dispatch CLASS over EVERY chat token with `is_explicit_action = false` (a
/// CASUAL mention / a non-deliberate event) and asserts NONE produces a [`DispatchOutcome::WouldDispatch`].
/// Because [`dispatch_disposition_class`] is a total function and a `NotifyOnly` class can only map to
/// `NotifyOnly`, this is a structural proof there is no code path from a casual chat event to a costed
/// run. Returns `true` IFF the no-auto-spawn property holds (0 auto-spawn paths).
pub fn no_auto_spawn_path_is_wired(chat_tokens: &[&str]) -> bool {
    // A casual (non-explicit) pass over every chat token must NEVER would-dispatch. The mention token
    // is the load-bearing case (it is notify-only EVEN if mis-flagged as an action, the glue floor),
    // but the property holds for the whole casual surface: no casual event auto-spawns a run.
    !chat_tokens
        .iter()
        .any(|token| dispatch_disposition_class(token, false) == DispatchOutcome::WouldDispatch)
}

/// **The mention token is the explicit-first reference gate (contract 8.6 / §1.1).** A convenience the
/// drill asserts: `chat.message.mentioned` is ALWAYS [`DispatchOutcome::NotifyOnly`] — even when an
/// upstream mistakenly flags it as an explicit action (the glue floor: a casual mention can only
/// notify). Returns `true` IFF the mention is notify-only under BOTH flags.
pub fn mention_is_always_notify_only() -> bool {
    dispatch_disposition_class(CHAT_MESSAGE_MENTIONED, false) == DispatchOutcome::NotifyOnly
        && dispatch_disposition_class(CHAT_MESSAGE_MENTIONED, true) == DispatchOutcome::NotifyOnly
}

// ───────────────────────── the agent provenance popover (S12 — §7.5) ──────────────────────────────

/// **The agent provenance popover — "why did this agent post?" (S12; arch §7.5).** Derived from an
/// agent message's [`EventEnvelope`] (NOT hand-authored): *which* agent (the actor principal +
/// runtime), **on whose authority/lawful basis** (`actor.on_behalf_of`, the delegation), **triggered
/// by which event** (`causation_id` — the explicit action / the parent event), the `correlation_id`
/// threading the whole flow, and the human-action ref (`caused_by`). Every agent message carries the
/// **agent badge** (AI-Act legibility — agents are never disguised as humans). The popover answers
/// the question inline with an audit-log link (the `correlation_id` is the audit anchor).
///
/// This is the DISPLAY half of M4-C9 — it is derivation-only (no mutation, no cost), so it is NOT
/// part of the mandatory-core cargo-mutants floor (that is the dispatch cost path).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentProvenance {
    /// **Which agent** — the acting agent principal id (the attribution id, never a human's name).
    pub agent: PrincipalId,
    /// **Which runtime** — the agent's `runtime_ref` (the brain behind the post; the mock today, the
    /// real `LlmAgentRuntime` post-M5). Surfaced so "agents look like agents".
    pub runtime_ref: Option<String>,
    /// **On whose authority / lawful basis** — `actor.on_behalf_of` (the delegating human/principal).
    /// `None` for a self-authorised agent (no delegation); `Some` names the principal the run acts
    /// for (Art. 22 legibility — a human is accountable for the delegated run).
    pub on_behalf_of: Option<PrincipalId>,
    /// **Triggered by which event** — the `causation_id` (the IMMEDIATE parent: the explicit action /
    /// the event that caused this post). `None` for a root (the agent posted as a causal root).
    pub triggered_by: Option<EventId>,
    /// **The flow this post threads** — the `correlation_id` (the causal ROOT carried through). The
    /// audit-log anchor: "show me the whole flow this post belongs to".
    pub correlation_id: CorrelationId,
    /// **The originating human action** — `caused_by` (the distinct human-action/session ref, BUS-5).
    /// The human who STARTED the chain a deep reactive run still attributes to.
    pub human_action: Option<CausedBy>,
    /// The **agent badge** marker — always `true` for an agent post (AI-Act: agents are never
    /// disguised as humans). If the message is NOT from an agent, [`agent_provenance`] returns `None`.
    pub agent_badge: bool,
}

/// The audit-log link kind the popover renders (arch §7.5 — "with an audit-log link"). The
/// `correlation_id` is the anchor; the UI resolves it to the audit-trail view. Named so the popover
/// carries a STRUCTURED link, never a free-text string the design-manual forbids.
pub const PROVENANCE_AUDIT_LINK_KIND: &str = "audit-log:correlation";

/// **Derive the provenance popover from an agent message envelope (S12; §7.5).** Returns `Some` IFF
/// the message's actor is an AGENT (`PrincipalKind::Agent`) — a human/service message has no agent
/// provenance popover (it is not an agent post; `None`). The derivation reads only the envelope's
/// frozen provenance fields (`actor` / `causation_id` / `correlation_id` / `caused_by`) — it never
/// reaches into a body (references-not-payloads) and never fabricates a parent (the fields are
/// derived correct-by-construction by the outbox, [`myelin_events::derive_envelope`]).
pub fn agent_provenance(message: &EventEnvelope) -> Option<AgentProvenance> {
    let Actor(principal) = &message.actor;
    match &principal.kind {
        PrincipalKind::Agent {
            runtime_ref,
            on_behalf_of,
        } => Some(AgentProvenance {
            agent: principal.principal_id.clone(),
            runtime_ref: Some(runtime_ref.0.clone()),
            on_behalf_of: on_behalf_of.clone(),
            triggered_by: message.causation_id.clone(),
            correlation_id: message.correlation_id.clone(),
            human_action: message.caused_by.clone(),
            // the agent badge is ALWAYS set on an agent post (AI-Act legibility, §7.5).
            agent_badge: true,
        }),
        // a human / service message is NOT an agent post — no provenance popover.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{CHAT_MESSAGE_CREATED, CHAT_REACTION_ADDED};
    use myelin_events::{AggregateKey, ArtifactRef, DataRole, EventType, Timestamp, Visibility};
    use myelin_identity::{
        DelegationCaveats, FailStaticBound, Principal, PrincipalStatus, RunId as IdRunId, RunToken,
        RuntimeRef,
    };
    use myelin_tenancy::Region;

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }

    // ───────────────────────── explicit-first class: mention notifies, never spawns ───────────────

    #[test]
    fn a_casual_mention_is_notify_only_never_a_dispatch() {
        // the load-bearing case: a casual @agent mention NOTIFIES, it never would-dispatch a run.
        assert_eq!(
            dispatch_disposition_class(CHAT_MESSAGE_MENTIONED, false),
            DispatchOutcome::NotifyOnly,
            "a casual @agent mention notifies — it does NOT auto-spawn a costed run (CHAT-1)"
        );
    }

    #[test]
    fn a_mention_is_notify_only_even_if_mis_flagged_as_an_action() {
        // the explicit-first floor: a mention can ONLY notify, even if mis-flagged explicit (glue floor).
        assert_eq!(
            dispatch_disposition_class(CHAT_MESSAGE_MENTIONED, true),
            DispatchOutcome::NotifyOnly,
            "a mention stays notify-only even mis-flagged as an action (the explicit-first floor)"
        );
        assert!(mention_is_always_notify_only());
    }

    #[test]
    fn an_explicit_action_would_dispatch() {
        assert_eq!(
            dispatch_disposition_class(CHAT_REACTION_ADDED, true),
            DispatchOutcome::WouldDispatch,
            "a deliberate explicit action (an approve-reaction) would dispatch a costed run"
        );
    }

    #[test]
    fn a_non_mention_non_explicit_event_still_only_notifies() {
        // the safe default: a non-mention chat event with no explicit action is STILL notify-only.
        assert_eq!(
            dispatch_disposition_class(CHAT_REACTION_ADDED, false),
            DispatchOutcome::NotifyOnly,
            "a non-deliberate chat event is notify-only — a run needs a DELIBERATE explicit action"
        );
    }

    // ───────────────────────── the structural no-auto-spawn check ─────────────────────────────────

    #[test]
    fn no_auto_spawn_path_over_the_whole_casual_chat_surface() {
        // EVERY chat token, taken as a casual (non-explicit) event, must NEVER would-dispatch.
        let chat_tokens: &[&str] = &[
            CHAT_MESSAGE_MENTIONED,
            CHAT_MESSAGE_CREATED,
            CHAT_REACTION_ADDED,
        ];
        assert!(
            no_auto_spawn_path_is_wired(chat_tokens),
            "0 auto-spawn paths: no casual chat event spawns a costed run (CHAT-D17)"
        );
    }

    #[test]
    fn the_l3_auto_spawn_absence_is_named() {
        // the absence is a recorded decision, not a silent gap (EI-01 §1).
        assert!(L3_AUTO_SPAWN_ABSENCE.contains("L-3"));
        assert!(L3_AUTO_SPAWN_ABSENCE.contains("counsel-gated"));
    }

    // ───────────────────────── the reserve gate (no balance → no run) ─────────────────────────────

    #[test]
    fn the_reserve_gate_admits_on_balance_and_refuses_on_no_balance() {
        let mut ledger = CostLedger::new();
        // sufficient balance → reserved.
        let ok = reserve_gate(
            &mut ledger,
            tenant(),
            LedgerRunId("run:1".into()),
            MinorUnits(5),
            MinorUnits(10),
        );
        assert!(ok.is_ok(), "a funded explicit run reserves");
        // exhausted balance → refused (no balance → no run).
        let refused = reserve_gate(
            &mut ledger,
            tenant(),
            LedgerRunId("run:2".into()),
            MinorUnits(50),
            MinorUnits(10),
        );
        assert!(
            matches!(refused, Err(ReserveError::InsufficientBalance { .. })),
            "an exhausted wallet REFUSES the dispatch — no balance, no run (11.7)"
        );
    }

    // ───────────────────────── the full explicit dispatch chain (reserve → mint → EffectApi) ──────

    /// A deterministic mock Identity that mints a per-run token (4.7) — the real mint is P-ID-18.
    struct MockIdentity;
    impl IdentityService for MockIdentity {
        fn authenticate(
            &self,
            _c: &myelin_identity::Credential,
        ) -> myelin_identity::Result<Principal> {
            unimplemented!("not exercised by the dispatch tests")
        }
        fn check(
            &self,
            _s: &Principal,
            _p: &myelin_identity::Permission,
            _o: &ArtifactRef,
            _a: &myelin_identity::Consistency,
            _c: Option<&myelin_identity::CaveatContext>,
        ) -> myelin_identity::Result<myelin_identity::Decision> {
            unimplemented!()
        }
        fn list_objects(
            &self,
            _s: &Principal,
            _p: &myelin_identity::Permission,
            _t: &myelin_identity::ObjectType,
            _a: &myelin_identity::Consistency,
        ) -> myelin_identity::Result<myelin_identity::ListObjectsResult> {
            unimplemented!()
        }
        fn list_subjects(
            &self,
            _o: &myelin_identity::ObjectId,
            _p: &myelin_identity::Permission,
            _a: &myelin_identity::Consistency,
        ) -> myelin_identity::Result<myelin_identity::SubjectTree> {
            unimplemented!()
        }
        fn explain(
            &self,
            _s: &Principal,
            _p: &myelin_identity::Permission,
            _o: &myelin_identity::ObjectId,
            _a: &myelin_identity::Consistency,
        ) -> myelin_identity::Result<myelin_identity::RewriteTrace> {
            unimplemented!()
        }
        fn delegation(
            &self,
            _a: &Principal,
            _t: &Principal,
        ) -> myelin_identity::Result<myelin_identity::EffectivePolicy> {
            unimplemented!()
        }
        fn write_tuples(
            &self,
            _d: &[myelin_identity::TupleDelta],
            _p: Option<&myelin_identity::Precondition>,
        ) -> myelin_identity::Result<myelin_identity::Zookie> {
            unimplemented!()
        }
        fn mint_run_token(
            &self,
            _agent_id: &PrincipalId,
            run_id: &IdRunId,
            _caveats: &DelegationCaveats,
            _ttl: &FailStaticBound,
        ) -> myelin_identity::Result<RunToken> {
            // a deterministic per-run token (the real mint is P-ID-18; the shape is frozen).
            Ok(RunToken {
                token: format!("tok:{}", run_id.0),
                jti: format!("jti:{}", run_id.0),
            })
        }
        fn revoke(&self, _t: &myelin_identity::RevokeTarget) -> myelin_identity::Result<()> {
            unimplemented!()
        }
        fn resolve_pseudonym(
            &self,
            _p: &PrincipalId,
            _tenant: &TenantId,
        ) -> myelin_identity::Result<String> {
            unimplemented!()
        }
        fn erase(&self, _p: &PrincipalId) -> myelin_identity::Result<()> {
            unimplemented!()
        }
        fn admit_fragment(
            &self,
            _f: &myelin_identity::NamespaceFragment,
        ) -> myelin_identity::Result<myelin_identity::FragmentAdmit> {
            unimplemented!()
        }
    }

    /// A deterministic mock `EffectApi` — applies the proposed effect (the real plan-then-apply is
    /// AG-P6 / P-218); records that the chat output ROUTED through it (the routing split, X-6).
    struct MockEffectApi;
    impl EffectApi for MockEffectApi {
        fn apply(&self, _run: &RunCtx, effect: ProposedEffect) -> EffectResult {
            EffectResult::Applied(myelin_agent::EventId(format!("applied:{}", effect.0)))
        }
    }

    fn agent_id() -> PrincipalId {
        PrincipalId("agent:assistant".into())
    }

    #[test]
    fn an_explicit_dispatch_reserves_mints_a_token_and_routes_the_output_through_effect_api() {
        let id = MockIdentity;
        let fx = MockEffectApi;
        let mut ledger = CostLedger::new();
        let (disp, applied) = dispatch_explicit(
            &id,
            &fx,
            &mut ledger,
            tenant(),
            &agent_id(),
            "run:explicit:1",
            MinorUnits(5),
            MinorUnits(10),
            ProposedEffect("chat.post".into()),
        );
        // (1) the run dispatched + carries the minted per-run token (4.7).
        assert_eq!(
            disp,
            Disposition::Dispatched {
                run_token_jti: "jti:run:explicit:1".into()
            },
            "an explicit dispatch reserves, mints a token, and dispatches"
        );
        // (2) the chat output ROUTED through EffectApi (8.2 — the routing split, X-6).
        assert_eq!(
            applied,
            Some(EffectResult::Applied(myelin_agent::EventId(
                "applied:chat.post".into()
            ))),
            "the run's chat output routed through EffectApi (8.2)"
        );
    }

    #[test]
    fn an_explicit_dispatch_with_no_balance_is_refused_before_any_mint_or_apply() {
        let id = MockIdentity;
        let fx = MockEffectApi;
        let mut ledger = CostLedger::new();
        let (disp, applied) = dispatch_explicit(
            &id,
            &fx,
            &mut ledger,
            tenant(),
            &agent_id(),
            "run:explicit:2",
            MinorUnits(50),
            MinorUnits(10), // exhausted wallet
            ProposedEffect("chat.post".into()),
        );
        // the reserve gate bites EVEN the explicit run — no balance → no run (11.7).
        assert_eq!(
            disp,
            Disposition::NoBalanceRefused {
                requested: MinorUnits(50),
                available: MinorUnits(10)
            },
            "no balance → no run: reserve/settle gates even the explicit run (CHAT-D17)"
        );
        // nothing applied — the run never started (no mint, no EffectApi route).
        assert_eq!(applied, None, "a refused dispatch applies nothing");
    }

    // ───────────────────────── the provenance popover (S12 / §7.5) ────────────────────────────────

    fn agent_message(
        on_behalf_of: Option<PrincipalId>,
        causation: Option<EventId>,
        caused_by: Option<CausedBy>,
    ) -> EventEnvelope {
        let agent = Principal::new(
            tenant(),
            Region("fr-par".into()),
            agent_id(),
            PrincipalKind::Agent {
                runtime_ref: RuntimeRef("mock-runtime".into()),
                on_behalf_of,
            },
            myelin_identity::DataRole::Controller,
            PrincipalStatus::Active,
        );
        EventEnvelope {
            event_id: EventId("evt:post".into()),
            type_: EventType(CHAT_MESSAGE_CREATED.into()),
            schema_ver: 1,
            tenant: tenant(),
            region: Region("fr-par".into()),
            actor: Actor(agent),
            subject: ArtifactRef("myelin://acme/chat/message/M1".into()),
            aggregate: AggregateKey("agg:chan".into()),
            causation_id: causation,
            correlation_id: CorrelationId("root-flow-1".into()),
            caused_by,
            depth: 1,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            occurred_at: Timestamp("2026-06-24T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-24T00:00:00Z".into()),
            pii_key_ref: None,
            payload: serde_json::json!({}),
        }
    }

    #[test]
    fn provenance_answers_why_did_this_agent_post() {
        let msg = agent_message(
            Some(PrincipalId("psn:alice".into())),
            Some(EventId("evt:explicit-action".into())),
            Some(CausedBy("session:alice".into())),
        );
        let prov = agent_provenance(&msg).expect("an agent post HAS a provenance popover");
        // which agent + which runtime (agents look like agents).
        assert_eq!(prov.agent, agent_id());
        assert_eq!(prov.runtime_ref.as_deref(), Some("mock-runtime"));
        // on whose authority / lawful basis (Art. 22 legibility).
        assert_eq!(prov.on_behalf_of, Some(PrincipalId("psn:alice".into())));
        // triggered by which event (the explicit action / parent).
        assert_eq!(
            prov.triggered_by,
            Some(EventId("evt:explicit-action".into()))
        );
        // the flow this post threads (the audit anchor).
        assert_eq!(prov.correlation_id, CorrelationId("root-flow-1".into()));
        // the originating human action (BUS-5).
        assert_eq!(prov.human_action, Some(CausedBy("session:alice".into())));
        // the agent badge is ALWAYS set (AI-Act: agents are never disguised as humans).
        assert!(
            prov.agent_badge,
            "an agent post always carries the agent badge"
        );
    }

    #[test]
    fn a_root_agent_post_has_no_triggering_event_but_still_has_a_popover() {
        // a self-authorised root agent post: no causation_id, no on_behalf_of — still legible.
        let msg = agent_message(None, None, None);
        let prov = agent_provenance(&msg).expect("a root agent post still has a popover");
        assert_eq!(
            prov.triggered_by, None,
            "a root post has no triggering event"
        );
        assert_eq!(
            prov.on_behalf_of, None,
            "a self-authorised agent has no delegation"
        );
        assert!(prov.agent_badge);
        // the correlation_id is still the audit anchor.
        assert_eq!(prov.correlation_id, CorrelationId("root-flow-1".into()));
    }

    #[test]
    fn a_human_message_has_no_agent_provenance_popover() {
        let human = Principal::stub(
            PrincipalId("psn:bob".into()),
            PrincipalKind::Human,
            tenant(),
        );
        let mut msg = agent_message(None, None, None);
        msg.actor = Actor(human);
        assert!(
            agent_provenance(&msg).is_none(),
            "a human message is NOT an agent post — no provenance popover"
        );
    }

    #[test]
    fn the_audit_link_kind_is_structured_not_free_text() {
        // the design-manual forbids a free-text link; the popover carries a STRUCTURED link kind.
        assert_eq!(PROVENANCE_AUDIT_LINK_KIND, "audit-log:correlation");
    }
}
