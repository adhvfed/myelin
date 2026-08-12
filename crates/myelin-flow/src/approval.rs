use crate::engine::SignalStore;
use crate::wfctx::{RetryPolicy, WaitOutcome, WfCtx, WfResult};
use myelin_refs::ArtifactRef;
use myelin_tenancy::TenantId;

pub const APPROVAL_SIGNAL_NAME: &str = "approval";

pub fn per_effect_idem_key(card_id: &str, effect_idx: usize, total_effects: usize) -> String {
    debug_assert!(
        total_effects >= 1,
        "a card gates at least one effect (total_effects >= 1)"
    );
    debug_assert!(
        effect_idx < total_effects,
        "effect_idx ({effect_idx}) must index into the card's {total_effects} effect(s)"
    );
    if total_effects == 1 {
        card_id.to_string()
    } else {
        format!("{card_id}:{effect_idx}")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalDecision {
    Approve,
    Decline,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GatedEffect {
    pub effect_ref: ArtifactRef,
    pub decision: ApprovalDecision,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApprovalCard {
    pub run_id: String,
    pub card_id: String,
    pub effects: Vec<GatedEffect>,
}

impl ApprovalCard {
    pub fn idem_key_for(&self, idx: usize) -> String {
        per_effect_idem_key(&self.card_id, idx, self.effects.len())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EffectOutcome {
    Applied(String),
    Withheld(String),
}

pub type GateResult = Result<EffectOutcome, ApplyError>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApplyError {
    EffectDenied(String),
}

impl std::fmt::Display for ApplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApplyError::EffectDenied(r) => write!(f, "effect denied by EffectApi: {r}"),
        }
    }
}

impl std::error::Error for ApplyError {}

pub type EffectApplier<'a> = dyn Fn(&ArtifactRef) -> Result<String, String> + 'a;

pub fn apply_approved_effects(
    signals: &SignalStore,
    tenant: &TenantId,
    card: &ApprovalCard,
    apply: &EffectApplier<'_>,
) -> Vec<Option<GateResult>> {
    let total = card.effects.len();
    card.effects
        .iter()
        .enumerate()
        .map(|(idx, effect)| {
            let key = per_effect_idem_key(&card.card_id, idx, total);
            let row = signals.get(tenant, &card.run_id, APPROVAL_SIGNAL_NAME, &key)?;

            let declined = row.payload_key_ref.as_deref() == Some(DECLINE_MARKER);

            Some(match (effect.decision, declined) {
                (ApprovalDecision::Decline, _) | (_, true) => {
                    Ok(EffectOutcome::Withheld(DECLINE_MARKER.to_string()))
                }
                (ApprovalDecision::Approve, false) => match apply(&effect.effect_ref) {
                    Ok(event_id) => Ok(EffectOutcome::Applied(event_id)),
                    Err(reason) => Err(ApplyError::EffectDenied(reason)),
                },
            })
        })
        .collect()
}

pub const DECLINE_MARKER: &str = "decline";

pub const APPROVAL_REQUESTED_EVENT: &str = "agent.approval.requested";

pub fn approval_wait_name(call_id: &str) -> String {
    format!("approval:{call_id}")
}

pub fn request_approval_and_wait<MkDraft>(
    ctx: &mut WfCtx,
    call_id: &str,
    request_refs: Vec<ArtifactRef>,
    timeout_secs: Option<i64>,
    make_request_draft: MkDraft,
) -> WfResult<WaitOutcome>
where
    MkDraft: Fn(&[ArtifactRef]) -> myelin_events::EventDraft,
{
    let refs = request_refs.clone();
    let draft = make_request_draft(&refs);
    let emitted_via = std::cell::RefCell::new(false);
    {
        let draft_cell = std::cell::RefCell::new(Some(draft));
        let request_refs2 = request_refs.clone();
        ctx_activity_emit(ctx, &emitted_via, &draft_cell, &request_refs2)?;
    }

    ctx.wait_for_signal(&approval_wait_name(call_id), timeout_secs)
}

fn ctx_activity_emit(
    ctx: &mut WfCtx,
    emitted: &std::cell::RefCell<bool>,
    draft: &std::cell::RefCell<Option<myelin_events::EventDraft>>,
    request_refs: &[ArtifactRef],
) -> WfResult<()> {
    ctx.activity(RetryPolicy { max_attempts: 1 }, |_idem, _attempt| {
        *emitted.borrow_mut() = true;
        Ok(request_refs.to_vec())
    })?;
    if *emitted.borrow() {
        if let Some(d) = draft.borrow_mut().take() {
            ctx.emit(d, None)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::{DurableExecutor, FlowExecutor, RunBudget, RunId, SignalSpec, StartSpec};
    use myelin_events::{IdMinter, MonotonicMinter};
    use myelin_tenancy::Region;
    use std::cell::RefCell;
    use std::sync::Arc;

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }
    fn minter() -> Arc<dyn IdMinter> {
        Arc::new(MonotonicMinter::new())
    }

    fn executor() -> FlowExecutor {
        let ex = FlowExecutor::new(minter(), tenant(), region());
        ex.register_definition("agent.run");
        ex
    }

    fn start_a_run(ex: &FlowExecutor) -> RunId {
        ex.start(StartSpec {
            wf_type: "agent.run".into(),
            input: vec![],
            budget: Some(RunBudget {
                minor_units: 10_000_000,
            }),
            idem_key: "k".into(),
        })
        .expect("start")
    }

    fn approve(ex: &FlowExecutor, run: &RunId, card_id: &str, idx: usize, total: usize) {
        let key = per_effect_idem_key(card_id, idx, total);
        ex.signal(SignalSpec {
            run: run.clone(),
            signal_name: APPROVAL_SIGNAL_NAME.into(),
            idem_key: key,
            payload: vec![ArtifactRef(format!(
                "myelin://acme/agent/effect/{card_id}-{idx}"
            ))],
            payload_key_ref: None,
        })
        .expect("approve");
    }

    fn decline(ex: &FlowExecutor, run: &RunId, card_id: &str, idx: usize, total: usize) {
        let key = per_effect_idem_key(card_id, idx, total);
        ex.signal(SignalSpec {
            run: run.clone(),
            signal_name: APPROVAL_SIGNAL_NAME.into(),
            idem_key: key,
            payload: vec![],
            payload_key_ref: Some(DECLINE_MARKER.into()),
        })
        .expect("decline");
    }

    #[test]
    fn per_effect_idem_key_follows_the_frozen_rule() {
        assert_eq!(per_effect_idem_key("card-7", 0, 1), "card-7");
        assert_eq!(per_effect_idem_key("card-7", 0, 3), "card-7:0");
        assert_eq!(per_effect_idem_key("card-7", 1, 3), "card-7:1");
        assert_eq!(per_effect_idem_key("card-7", 2, 3), "card-7:2");
    }

    fn three_effect_card(
        run: &RunId,
        d0: ApprovalDecision,
        d1: ApprovalDecision,
        d2: ApprovalDecision,
    ) -> ApprovalCard {
        ApprovalCard {
            run_id: run.0.clone(),
            card_id: "card-7".into(),
            effects: vec![
                GatedEffect {
                    effect_ref: ArtifactRef("myelin://acme/agent/effect/e0".into()),
                    decision: d0,
                },
                GatedEffect {
                    effect_ref: ArtifactRef("myelin://acme/agent/effect/e1".into()),
                    decision: d1,
                },
                GatedEffect {
                    effect_ref: ArtifactRef("myelin://acme/agent/effect/e2".into()),
                    decision: d2,
                },
            ],
        }
    }

    #[test]
    fn three_per_effect_keys_apply_and_decline_independently() {
        let ex = executor();
        let run = start_a_run(&ex);
        approve(&ex, &run, "card-7", 0, 3);
        decline(&ex, &run, "card-7", 1, 3);
        approve(&ex, &run, "card-7", 2, 3);
        assert_eq!(ex.signals().count_for_run(&tenant(), &run.0), 3);

        let applied = RefCell::new(Vec::<String>::new());
        let card = three_effect_card(
            &run,
            ApprovalDecision::Approve,
            ApprovalDecision::Decline,
            ApprovalDecision::Approve,
        );
        let outcomes =
            apply_approved_effects(ex.signals(), &tenant(), &card, &|eff: &ArtifactRef| {
                applied.borrow_mut().push(eff.0.clone());
                Ok(format!("evt-for-{}", eff.0))
            });

        assert_eq!(outcomes.len(), 3);
        assert!(
            matches!(outcomes[0], Some(Ok(EffectOutcome::Applied(_)))),
            "effect 0 approved → applied"
        );
        assert_eq!(
            outcomes[1],
            Some(Ok(EffectOutcome::Withheld(DECLINE_MARKER.to_string()))),
            "effect 1 declined → WITHHELD (Denied, zero mutation, AG-8)"
        );
        assert!(
            matches!(outcomes[2], Some(Ok(EffectOutcome::Applied(_)))),
            "effect 2 approved → applied"
        );

        let applied = applied.into_inner();
        assert_eq!(
            applied.len(),
            2,
            "exactly two effects applied (0 and 2); the declined effect 1 made 0 mutation"
        );
        assert_eq!(applied[0], "myelin://acme/agent/effect/e0");
        assert_eq!(applied[1], "myelin://acme/agent/effect/e2");
        assert!(
            !applied.contains(&"myelin://acme/agent/effect/e1".to_string()),
            "the DECLINED effect was WITHHELD - apply was NEVER reached for it (AG-8: 0 mutation on decline)"
        );
    }

    #[test]
    fn double_click_approve_all_applies_each_effect_once() {
        let ex = executor();
        let run = start_a_run(&ex);
        for idx in 0..3 {
            approve(&ex, &run, "card-7", idx, 3);
        }
        for idx in 0..3 {
            approve(&ex, &run, "card-7", idx, 3);
        }
        assert_eq!(
            ex.signals().count_for_run(&tenant(), &run.0),
            3,
            "a double-click on approve-all re-sends the same keys → 0 new buffered signals (ON CONFLICT DO NOTHING)"
        );

        let applies = RefCell::new(0usize);
        let card = three_effect_card(
            &run,
            ApprovalDecision::Approve,
            ApprovalDecision::Approve,
            ApprovalDecision::Approve,
        );
        let outcomes =
            apply_approved_effects(ex.signals(), &tenant(), &card, &|_eff: &ArtifactRef| {
                *applies.borrow_mut() += 1;
                Ok("evt".into())
            });
        assert_eq!(
            *applies.borrow(),
            3,
            "exactly 3 applies (the double-click did not double-apply)"
        );
        assert!(outcomes
            .iter()
            .all(|o| matches!(o, Some(Ok(EffectOutcome::Applied(_))))));
    }

    #[test]
    fn declined_single_effect_card_makes_zero_mutation() {
        let ex = executor();
        let run = start_a_run(&ex);
        decline(&ex, &run, "card-1", 0, 1);
        assert!(ex
            .signals()
            .get(&tenant(), &run.0, APPROVAL_SIGNAL_NAME, "card-1")
            .is_some());

        let applies = RefCell::new(0usize);
        let card = ApprovalCard {
            run_id: run.0.clone(),
            card_id: "card-1".into(),
            effects: vec![GatedEffect {
                effect_ref: ArtifactRef("myelin://acme/agent/effect/only".into()),
                decision: ApprovalDecision::Decline,
            }],
        };
        let outcomes =
            apply_approved_effects(ex.signals(), &tenant(), &card, &|_eff: &ArtifactRef| {
                *applies.borrow_mut() += 1;
                Ok("evt".into())
            });
        assert_eq!(
            outcomes[0],
            Some(Ok(EffectOutcome::Withheld(DECLINE_MARKER.to_string()))),
            "the declined single effect is WITHHELD (AG-8)"
        );
        assert_eq!(
            *applies.borrow(),
            0,
            "apply was NEVER reached - a declined effect makes 0 mutation (AG-8)"
        );
    }

    #[test]
    fn an_undecided_effect_is_skipped_pending_the_wait() {
        let ex = executor();
        let run = start_a_run(&ex);
        approve(&ex, &run, "card-7", 0, 3);

        let card = three_effect_card(
            &run,
            ApprovalDecision::Approve,
            ApprovalDecision::Approve,
            ApprovalDecision::Approve,
        );
        let outcomes =
            apply_approved_effects(ex.signals(), &tenant(), &card, &|_e: &ArtifactRef| {
                Ok("evt".into())
            });
        assert!(
            matches!(outcomes[0], Some(Ok(EffectOutcome::Applied(_)))),
            "effect 0 has a decision → applied"
        );
        assert_eq!(
            outcomes[1], None,
            "effect 1 has no buffered decision → skipped (the wait re-runs the loop)"
        );
        assert_eq!(
            outcomes[2], None,
            "effect 2 has no buffered decision → skipped"
        );
    }

    #[test]
    fn a_non_decline_apply_failure_is_surfaced() {
        let ex = executor();
        let run = start_a_run(&ex);
        approve(&ex, &run, "card-1", 0, 1);

        let card = ApprovalCard {
            run_id: run.0.clone(),
            card_id: "card-1".into(),
            effects: vec![GatedEffect {
                effect_ref: ArtifactRef("myelin://acme/agent/effect/only".into()),
                decision: ApprovalDecision::Approve,
            }],
        };
        let outcomes =
            apply_approved_effects(ex.signals(), &tenant(), &card, &|_e: &ArtifactRef| {
                Err("capability denied".into())
            });
        assert_eq!(
            outcomes[0],
            Some(Err(ApplyError::EffectDenied("capability denied".into()))),
            "an EffectApi denial is surfaced, distinct from the AG-8 withhold of a decline"
        );
    }

    use crate::engine::{drive_full, run_state, DriveOutcome, RunRow, WorkflowBody};
    use crate::wfctx::{WfCtx, WfJournal};
    use myelin_events::{
        Actor, AggregateKey, ArtifactRef as EvArtifactRef, DataRole, EmitContextBase, EventDraft,
        EventType, OutboxStore, Timestamp, Visibility,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn ctx_base() -> EmitContextBase {
        EmitContextBase {
            tenant: tenant(),
            region: region(),
            actor: Actor(Principal::stub(
                PrincipalId("p".into()),
                PrincipalKind::Human,
                tenant(),
            )),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
            caused_by: None,
        }
    }

    fn approval_request_draft(refs: &[ArtifactRef]) -> EventDraft {
        EventDraft {
            type_: EventType(APPROVAL_REQUESTED_EVENT.into()),
            subject: EvArtifactRef("myelin://acme/agent/run/R1".into()),
            aggregate: AggregateKey("run:R1".into()),
            payload: serde_json::json!({ "refs": refs.iter().map(|r| r.0.clone()).collect::<Vec<_>>() }),
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        }
    }

    fn gated_tool_body() -> Box<WorkflowBody> {
        Box::new(|ctx: &mut WfCtx| {
            let outcome = request_approval_and_wait(
                ctx,
                "call-1",
                vec![ArtifactRef("myelin://acme/agent/tool/merge".into())],
                Some(86_400),
                approval_request_draft,
            )
            .map_err(|e| format!("{e:?}"))?;
            match outcome {
                WaitOutcome::Signalled {
                    payload_key_ref, ..
                } if payload_key_ref.as_deref() == Some(DECLINE_MARKER) => Ok(vec![]),
                WaitOutcome::Signalled { .. } => {
                    let eff = ctx
                        .activity(RetryPolicy { max_attempts: 1 }, |_i, _a| {
                            Ok(vec![ArtifactRef(
                                "myelin://acme/agent/effect/merged".into(),
                            )])
                        })
                        .map_err(|e| format!("{e:?}"))?;
                    Ok(eff)
                }
                WaitOutcome::TimedOut => Ok(vec![]),
                WaitOutcome::Parked => Ok(vec![]),
            }
        })
    }

    #[test]
    fn approval_round_trip_requests_once_parks_then_approve_resumes_and_runs() {
        let ex = executor();
        let run = start_a_run(&ex);
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let part = 0i16;
        let run_row = RunRow::new_runnable(tenant(), region(), run.0.clone(), "agent.run", part);
        ex.runs().put(run_row.clone());
        let body = gated_tool_body();
        let tele = crate::engine::FlowTelemetry::new();

        let o1 = drive_full(
            ex.runs(),
            &outbox,
            &journal,
            &tele,
            minter(),
            ctx_base(),
            &run_row,
            "2026-06-21T00:00:00Z",
            7,
            body.as_ref(),
            1,
            1,
            None,
            Some(ex.signals().clone()),
            1_000,
            None,
            None,
        );
        assert_eq!(
            o1,
            DriveOutcome::Waiting,
            "drive 1 parks on the approval wait (state=waiting)"
        );
        assert_eq!(
            outbox.committed_count(),
            1,
            "the agent.approval.requested card request was emitted ONCE"
        );
        assert_eq!(
            ex.runs().get(&tenant(), &run.0).unwrap().state,
            run_state::WAITING,
            "the run holds no runtime while it waits (FLOW-D4)"
        );

        ex.signal(SignalSpec {
            run: run.clone(),
            signal_name: approval_wait_name("call-1"),
            idem_key: "card-7".into(),
            payload: vec![ArtifactRef("myelin://acme/agent/decision/approve".into())],
            payload_key_ref: None,
        })
        .expect("approve");

        ex.runs().wake(&tenant(), &run.0);
        let run_row2 = ex.runs().get(&tenant(), &run.0).unwrap();
        let o2 = drive_full(
            ex.runs(),
            &outbox,
            &journal,
            &tele,
            minter(),
            ctx_base(),
            &run_row2,
            "2026-06-21T00:00:00Z",
            7,
            body.as_ref(),
            1,
            1,
            None,
            Some(ex.signals().clone()),
            200_000,
            None,
            None,
        );
        match o2 {
            DriveOutcome::Completed(refs) => assert_eq!(
                refs,
                vec![ArtifactRef("myelin://acme/agent/effect/merged".into())],
                "drive 2 resumed + RAN the approved tool (one effect)"
            ),
            other => panic!("expected Completed, got {other:?}"),
        }
        assert_eq!(
            outbox.committed_count(),
            1,
            "the card request was emitted EXACTLY once (NO re-emit on the resume)"
        );
        assert_eq!(
            ex.signals().buffered_depth(),
            0,
            "the approval was consumed EXACTLY once (FLOW-D4: 1 consume)"
        );
    }

    #[test]
    fn approval_round_trip_deny_withholds_zero_mutation() {
        let ex = executor();
        let run = start_a_run(&ex);
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let run_row = RunRow::new_runnable(tenant(), region(), run.0.clone(), "agent.run", 0);
        ex.runs().put(run_row.clone());
        let body = gated_tool_body();
        let tele = crate::engine::FlowTelemetry::new();

        drive_full(
            ex.runs(),
            &outbox,
            &journal,
            &tele,
            minter(),
            ctx_base(),
            &run_row,
            "2026-06-21T00:00:00Z",
            7,
            body.as_ref(),
            1,
            1,
            None,
            Some(ex.signals().clone()),
            1_000,
            None,
            None,
        );
        let emits_after_park = outbox.committed_count();

        ex.signal(SignalSpec {
            run: run.clone(),
            signal_name: approval_wait_name("call-1"),
            idem_key: "card-7".into(),
            payload: vec![],
            payload_key_ref: Some(DECLINE_MARKER.into()),
        })
        .expect("decline");

        ex.runs().wake(&tenant(), &run.0);
        let run_row2 = ex.runs().get(&tenant(), &run.0).unwrap();
        let o2 = drive_full(
            ex.runs(),
            &outbox,
            &journal,
            &tele,
            minter(),
            ctx_base(),
            &run_row2,
            "2026-06-21T00:00:00Z",
            7,
            body.as_ref(),
            1,
            1,
            None,
            Some(ex.signals().clone()),
            2_000,
            None,
            None,
        );
        assert_eq!(
            o2,
            DriveOutcome::Completed(vec![]),
            "a DENY completes with NO effect (withheld)"
        );
        assert_eq!(
            outbox.committed_count(),
            emits_after_park,
            "the declined tool made 0 mutation - no effect emitted past the card request (AG-8)"
        );
        assert_eq!(
            ex.signals().buffered_depth(),
            0,
            "the decline was consumed once"
        );
    }

    #[test]
    fn card_idem_key_for_uses_the_per_effect_rule() {
        let multi = ApprovalCard {
            run_id: "r".into(),
            card_id: "c".into(),
            effects: vec![
                GatedEffect {
                    effect_ref: ArtifactRef("a".into()),
                    decision: ApprovalDecision::Approve,
                },
                GatedEffect {
                    effect_ref: ArtifactRef("b".into()),
                    decision: ApprovalDecision::Approve,
                },
            ],
        };
        assert_eq!(multi.idem_key_for(0), "c:0");
        assert_eq!(multi.idem_key_for(1), "c:1");

        let single = ApprovalCard {
            run_id: "r".into(),
            card_id: "c".into(),
            effects: vec![GatedEffect {
                effect_ref: ArtifactRef("a".into()),
                decision: ApprovalDecision::Approve,
            }],
        };
        assert_eq!(
            single.idem_key_for(0),
            "c",
            "single-effect card keys on the bare card id"
        );
    }
}
