use myelin_agent::{EffectApi, EffectResult, ProposedEffect, RunCtx};
use myelin_events::{Actor, CausedBy, CorrelationId, EventEnvelope, EventId};
use myelin_identity::{
    DelegationCaveats, FailStaticBound, IdentityService, PrincipalId, PrincipalKind,
    RunId as IdRunId, RunToken,
};
use myelin_storage::reserve_settle::{
    CostLedger, MicroUsd, Reservation, ReserveError, RunId as LedgerRunId,
};
use myelin_tenancy::TenantId;

use crate::events::CHAT_MESSAGE_MENTIONED;
use crate::glue::{agent_dispatch_class, AgentDispatchClass};

pub const L3_AUTO_SPAWN_ABSENCE: &str =
    "L-3 auto-spawn-on-mention is counsel-gated (recon §6 / AG-P20) - deliberately not wired in v1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Disposition {
    NotifiedInbox,
    Dispatched {
        run_token_jti: String,
    },
    NoBalanceRefused {
        requested: MicroUsd,
        available: MicroUsd,
    },
}

pub fn dispatch_disposition_class(token: &str, is_explicit_action: bool) -> DispatchOutcome {
    match agent_dispatch_class(token, is_explicit_action) {
        AgentDispatchClass::NotifyOnly => DispatchOutcome::NotifyOnly,
        AgentDispatchClass::ExplicitDispatch => DispatchOutcome::WouldDispatch,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DispatchOutcome {
    NotifyOnly,
    WouldDispatch,
}

pub fn reserve_gate(
    ledger: &mut CostLedger,
    tenant: TenantId,
    run: LedgerRunId,
    estimate: MicroUsd,
    available: MicroUsd,
) -> Result<Reservation, ReserveError> {
    ledger.reserve(tenant, run, estimate, available)
}

#[allow(clippy::too_many_arguments)]
pub fn dispatch_explicit<Id: IdentityService, Fx: EffectApi>(
    identity: &Id,
    effect_api: &Fx,
    ledger: &mut CostLedger,
    tenant: TenantId,
    agent_id: &PrincipalId,
    run_id: &str,
    estimate: MicroUsd,
    available: MicroUsd,
    output: ProposedEffect,
) -> (Disposition, Option<EffectResult>) {
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
        Err(_) => {
            return (
                Disposition::NoBalanceRefused {
                    requested: estimate,
                    available,
                },
                None,
            );
        }
        Ok(_reservation) => {}
    }

    let id_run = IdRunId(run_id.to_string());
    let token: RunToken = match identity.mint_run_token(
        agent_id,
        &id_run,
        &DelegationCaveats(vec![format!("chat:dispatch:{run_id}")]),
        &FailStaticBound::DEFAULT_W,
    ) {
        Ok(t) => t,
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

    let run_ctx = RunCtx(token.jti.clone());
    let applied = effect_api.apply(&run_ctx, output);

    (
        Disposition::Dispatched {
            run_token_jti: token.jti.clone(),
        },
        Some(applied),
    )
}

pub fn no_auto_spawn_path_is_wired(chat_tokens: &[&str]) -> bool {
    !chat_tokens
        .iter()
        .any(|token| dispatch_disposition_class(token, false) == DispatchOutcome::WouldDispatch)
}

pub fn mention_is_always_notify_only() -> bool {
    dispatch_disposition_class(CHAT_MESSAGE_MENTIONED, false) == DispatchOutcome::NotifyOnly
        && dispatch_disposition_class(CHAT_MESSAGE_MENTIONED, true) == DispatchOutcome::NotifyOnly
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentProvenance {
    pub agent: PrincipalId,
    pub runtime_ref: Option<String>,
    pub on_behalf_of: Option<PrincipalId>,
    pub triggered_by: Option<EventId>,
    pub correlation_id: CorrelationId,
    pub human_action: Option<CausedBy>,
    pub agent_badge: bool,
}

pub const PROVENANCE_AUDIT_LINK_KIND: &str = "audit-log:correlation";

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
            agent_badge: true,
        }),
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

    #[test]
    fn a_casual_mention_is_notify_only_never_a_dispatch() {
        assert_eq!(
            dispatch_disposition_class(CHAT_MESSAGE_MENTIONED, false),
            DispatchOutcome::NotifyOnly,
            "a casual @agent mention notifies - it does NOT auto-spawn a costed run (CHAT-1)"
        );
    }

    #[test]
    fn a_mention_is_notify_only_even_if_mis_flagged_as_an_action() {
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
        assert_eq!(
            dispatch_disposition_class(CHAT_REACTION_ADDED, false),
            DispatchOutcome::NotifyOnly,
            "a non-deliberate chat event is notify-only - a run needs a DELIBERATE explicit action"
        );
    }

    #[test]
    fn no_auto_spawn_path_over_the_whole_casual_chat_surface() {
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
        assert!(L3_AUTO_SPAWN_ABSENCE.contains("L-3"));
        assert!(L3_AUTO_SPAWN_ABSENCE.contains("counsel-gated"));
    }

    #[test]
    fn the_reserve_gate_admits_on_balance_and_refuses_on_no_balance() {
        let mut ledger = CostLedger::new();
        let ok = reserve_gate(
            &mut ledger,
            tenant(),
            LedgerRunId("run:1".into()),
            MicroUsd(5),
            MicroUsd(10),
        );
        assert!(ok.is_ok(), "a funded explicit run reserves");
        let refused = reserve_gate(
            &mut ledger,
            tenant(),
            LedgerRunId("run:2".into()),
            MicroUsd(50),
            MicroUsd(10),
        );
        assert!(
            matches!(refused, Err(ReserveError::InsufficientBalance { .. })),
            "an exhausted wallet REFUSES the dispatch - no balance, no run (11.7)"
        );
    }

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
            MicroUsd(5),
            MicroUsd(10),
            ProposedEffect("chat.post".into()),
        );
        assert_eq!(
            disp,
            Disposition::Dispatched {
                run_token_jti: "jti:run:explicit:1".into()
            },
            "an explicit dispatch reserves, mints a token, and dispatches"
        );
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
            MicroUsd(50),
            MicroUsd(10),
            ProposedEffect("chat.post".into()),
        );
        assert_eq!(
            disp,
            Disposition::NoBalanceRefused {
                requested: MicroUsd(50),
                available: MicroUsd(10)
            },
            "no balance → no run: reserve/settle gates even the explicit run (CHAT-D17)"
        );
        assert_eq!(applied, None, "a refused dispatch applies nothing");
    }

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
        assert_eq!(prov.agent, agent_id());
        assert_eq!(prov.runtime_ref.as_deref(), Some("mock-runtime"));
        assert_eq!(prov.on_behalf_of, Some(PrincipalId("psn:alice".into())));
        assert_eq!(
            prov.triggered_by,
            Some(EventId("evt:explicit-action".into()))
        );
        assert_eq!(prov.correlation_id, CorrelationId("root-flow-1".into()));
        assert_eq!(prov.human_action, Some(CausedBy("session:alice".into())));
        assert!(
            prov.agent_badge,
            "an agent post always carries the agent badge"
        );
    }

    #[test]
    fn a_root_agent_post_has_no_triggering_event_but_still_has_a_popover() {
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
            "a human message is NOT an agent post - no provenance popover"
        );
    }

    #[test]
    fn the_audit_link_kind_is_structured_not_free_text() {
        assert_eq!(PROVENANCE_AUDIT_LINK_KIND, "audit-log:correlation");
    }
}
