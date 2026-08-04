use std::collections::BTreeSet;

use myelin_events::{EventEnvelope, EventId};
use myelin_git::check_status::TrustTier as GitTrustTier;
use myelin_identity::{Literal, ObjectType, SetExpr};
use myelin_query::{CmpOp, EventMatcher, Expr, Predicate, RelMembership};

pub use myelin_ci_sandbox::TrustTier;

pub const TRIGGER_CONSUMER: &str = "ci-dispatch.trigger";

pub const RUN_OBJECT_TYPE: &str = "run";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OnTrigger {
    Push,
    PullRequest,
    IssueTransitioned,
    Manual,
    Schedule,
    Agent,
}

impl OnTrigger {
    pub fn event_types(&self) -> &'static [&'static str] {
        match self {
            OnTrigger::Push => &[myelin_git::events::GIT_REF_UPDATED],
            OnTrigger::PullRequest => &[
                myelin_git::events::GIT_PR_OPENED,
                myelin_git::events::GIT_PR_SYNCHRONIZED,
            ],
            OnTrigger::IssueTransitioned => &["issue.transitioned"],
            OnTrigger::Manual => &["ci.run.requested"],
            OnTrigger::Schedule => &["ci.schedule.tick"],
            OnTrigger::Agent => &["agent.ci.requested"],
        }
    }
}

fn type_eq(ty: &str) -> Predicate {
    Predicate::Cmp {
        op: CmpOp::Eq,
        lhs: Expr::Var("event.type".into()),
        rhs: Expr::Lit(Literal::Str(ty.into())),
    }
}

pub fn compile_trigger(on: &OnTrigger) -> Result<EventMatcher, myelin_query::PredicateError> {
    let types = on.event_types();
    let predicate = if types.len() == 1 {
        type_eq(types[0])
    } else {
        Predicate::Or(types.iter().map(|t| type_eq(t)).collect())
    };
    EventMatcher::compile(ObjectType(RUN_OBJECT_TYPE.into()), predicate)
}

pub fn trigger_matches(
    matcher: &EventMatcher,
    envelope: &EventEnvelope,
    visible: &SetExpr,
    member_oracle: &dyn Fn(&RelMembership) -> bool,
) -> Result<bool, myelin_query::EvalError> {
    matcher.matches(envelope, visible, member_oracle)
}

#[derive(Clone, Debug, Default)]
pub struct DedupLedger {
    recorded: BTreeSet<(String, String)>,
}

impl DedupLedger {
    pub fn new() -> DedupLedger {
        DedupLedger::default()
    }

    pub fn record(&mut self, consumer: &str, event_id: &EventId) -> bool {
        self.recorded
            .insert((consumer.to_string(), event_id.0.clone()))
    }

    pub fn seen(&self, consumer: &str, event_id: &EventId) -> bool {
        self.recorded
            .contains(&(consumer.to_string(), event_id.0.clone()))
    }

    pub fn effect_count(&self) -> usize {
        self.recorded.len()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RunProvenance {
    pub is_fork: bool,
    pub targets_self_hosted: bool,
    pub read_excludes_fork: bool,
}

pub fn classify_trust(provenance: &RunProvenance) -> TrustTier {
    if provenance.is_fork || !provenance.read_excludes_fork {
        return TrustTier::UntrustedFork;
    }
    if provenance.targets_self_hosted {
        return TrustTier::SelfHosted;
    }
    TrustTier::Trusted
}

pub fn git_trust_of(tier: TrustTier) -> GitTrustTier {
    match tier {
        TrustTier::UntrustedFork => GitTrustTier::UntrustedFork,
        TrustTier::Trusted | TrustTier::SelfHosted => GitTrustTier::Trusted,
    }
}

pub fn stamp_trust(provenance: &RunProvenance) -> TrustStamp {
    let tier = classify_trust(provenance);
    TrustStamp {
        job_tier: tier,
        check_tier: git_trust_of(tier),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrustStamp {
    pub job_tier: TrustTier,
    pub check_tier: GitTrustTier,
}

impl TrustStamp {
    pub fn is_consistent(&self) -> bool {
        let job_untrusted = self.job_tier == TrustTier::UntrustedFork;
        let check_untrusted = self.check_tier == GitTrustTier::UntrustedFork;
        job_untrusted == check_untrusted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{
        Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventType, Timestamp, Visibility,
    };
    use myelin_identity::{ObjectId, Principal, PrincipalKind};
    use myelin_tenancy::{Region, TenantId};

    fn principal() -> Principal {
        Principal::stub(
            myelin_identity::PrincipalId("alice".into()),
            PrincipalKind::Human,
            TenantId("t1".into()),
        )
    }

    fn envelope(type_: &str, run_id: &str) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId(format!("ev-{run_id}")),
            type_: EventType(type_.into()),
            schema_ver: 1,
            tenant: TenantId("t1".into()),
            region: Region("fr-par".into()),
            actor: Actor(principal()),
            subject: ArtifactRef(format!("myelin://t1/ci/run/{run_id}")),
            aggregate: AggregateKey("agg".into()),
            causation_id: None,
            correlation_id: CorrelationId("corr".into()),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-23T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-23T00:00:00Z".into()),
            payload: serde_json::json!({}),
        }
    }

    fn visible_all() -> SetExpr {
        SetExpr::All
    }

    fn no_member(_: &RelMembership) -> bool {
        false
    }

    #[test]
    fn pull_request_trigger_matches_the_right_events() {
        let m = compile_trigger(&OnTrigger::PullRequest).expect("compiles to QueryAst");
        for ty in [
            myelin_git::events::GIT_PR_OPENED,
            myelin_git::events::GIT_PR_SYNCHRONIZED,
        ] {
            assert!(
                trigger_matches(&m, &envelope(ty, "r1"), &visible_all(), &no_member).unwrap(),
                "the pull_request trigger fires on {ty}"
            );
        }
        assert!(
            !trigger_matches(
                &m,
                &envelope(myelin_git::events::GIT_REF_UPDATED, "r1"),
                &visible_all(),
                &no_member,
            )
            .unwrap(),
            "a push does NOT arm a pull_request trigger"
        );
    }

    #[test]
    fn push_trigger_matches_only_ref_updated() {
        let m = compile_trigger(&OnTrigger::Push).expect("compiles");
        assert!(trigger_matches(
            &m,
            &envelope(myelin_git::events::GIT_REF_UPDATED, "r1"),
            &visible_all(),
            &no_member,
        )
        .unwrap());
        assert!(!trigger_matches(
            &m,
            &envelope(myelin_git::events::GIT_PR_OPENED, "r1"),
            &visible_all(),
            &no_member,
        )
        .unwrap());
    }

    #[test]
    fn the_matcher_is_the_one_queryast_over_run() {
        let m = compile_trigger(&OnTrigger::Push).expect("compiles");
        assert_eq!(m.object_type().0, RUN_OBJECT_TYPE);
        assert_eq!(
            m.compile_subject_filter().as_deref(),
            Some(myelin_git::events::GIT_REF_UPDATED),
            "the type pin lowers to the NATS subject (the cheap prefilter, §4.5)"
        );
    }

    #[test]
    fn invisible_run_never_arms_even_on_a_type_hit() {
        let m = compile_trigger(&OnTrigger::Push).expect("compiles");
        let visible = SetExpr::Ids(vec![ObjectId("other".into())]);
        assert!(
            !trigger_matches(
                &m,
                &envelope(myelin_git::events::GIT_REF_UPDATED, "r1"),
                &visible,
                &no_member,
            )
            .unwrap(),
            "the predicate holds, but the run is invisible → 0 match (0-leak)"
        );
    }

    #[test]
    fn dedup_yields_one_effect_per_event_id() {
        let mut ledger = DedupLedger::new();
        let ev = EventId("ev-push-1".into());
        assert!(
            ledger.record(TRIGGER_CONSUMER, &ev),
            "first delivery fires the effect"
        );
        assert!(
            !ledger.record(TRIGGER_CONSUMER, &ev),
            "redelivery is absorbed (no second effect)"
        );
        assert!(ledger.seen(TRIGGER_CONSUMER, &ev));
        assert_eq!(ledger.effect_count(), 1, "exactly one effect recorded");
    }

    #[test]
    fn drill_deliver_twice_yields_exactly_one_run() {
        let m = compile_trigger(&OnTrigger::Push).expect("compiles");
        let mut ledger = DedupLedger::new();
        let env = envelope(myelin_git::events::GIT_REF_UPDATED, "r1");

        let mut runs_started = 0u32;
        for _ in 0..2 {
            let matched = trigger_matches(&m, &env, &visible_all(), &no_member).expect("eval ok");
            if matched && ledger.record(TRIGGER_CONSUMER, &env.event_id) {
                runs_started += 1;
            }
        }
        assert_eq!(
            runs_started, 1,
            "one push (one event_id) = exactly ONE run, even under double delivery"
        );
        assert_eq!(ledger.effect_count(), 1, "dedup-count = 0 duplicate runs");
    }

    #[test]
    fn member_push_is_trusted() {
        let prov = RunProvenance {
            is_fork: false,
            targets_self_hosted: false,
            read_excludes_fork: true,
        };
        assert_eq!(classify_trust(&prov), TrustTier::Trusted);
    }

    #[test]
    fn fork_pr_is_untrusted_fork() {
        let prov = RunProvenance {
            is_fork: true,
            targets_self_hosted: false,
            read_excludes_fork: false,
        };
        assert_eq!(classify_trust(&prov), TrustTier::UntrustedFork);
    }

    #[test]
    fn self_hosted_member_run_is_self_hosted() {
        let prov = RunProvenance {
            is_fork: false,
            targets_self_hosted: true,
            read_excludes_fork: true,
        };
        assert_eq!(classify_trust(&prov), TrustTier::SelfHosted);
    }

    #[test]
    fn edge_stamp_alone_forces_untrusted_fork() {
        let prov = RunProvenance {
            is_fork: false,
            targets_self_hosted: false,
            read_excludes_fork: false,
        };
        assert_eq!(classify_trust(&prov), TrustTier::UntrustedFork);
    }

    #[test]
    fn drill_fork_pr_stamps_both_halves_untrusted_zero_divergence() {
        let prov = RunProvenance {
            is_fork: true,
            targets_self_hosted: false,
            read_excludes_fork: false,
        };
        let stamp = stamp_trust(&prov);
        assert_eq!(stamp.job_tier, TrustTier::UntrustedFork, "JobSpec tier");
        assert_eq!(
            stamp.check_tier,
            GitTrustTier::UntrustedFork,
            "CheckStatus tier - the SAME fork verdict"
        );
        assert!(stamp.is_consistent(), "0 divergence between the two stamps");
    }

    #[test]
    fn every_tier_stamps_consistently() {
        for prov in [
            RunProvenance {
                is_fork: false,
                targets_self_hosted: false,
                read_excludes_fork: true,
            },
            RunProvenance {
                is_fork: false,
                targets_self_hosted: true,
                read_excludes_fork: true,
            },
        ] {
            let stamp = stamp_trust(&prov);
            assert_eq!(
                stamp.check_tier,
                GitTrustTier::Trusted,
                "a non-fork run is trusted CODE for the gate ({:?})",
                stamp.job_tier
            );
            assert!(stamp.is_consistent(), "0 divergence ({:?})", stamp.job_tier);
        }
    }

    #[test]
    fn stamp_is_consistent_for_all_provenance() {
        for is_fork in [false, true] {
            for targets_self_hosted in [false, true] {
                for read_excludes_fork in [false, true] {
                    let prov = RunProvenance {
                        is_fork,
                        targets_self_hosted,
                        read_excludes_fork,
                    };
                    let stamp = stamp_trust(&prov);
                    assert!(
                        stamp.is_consistent(),
                        "stamp diverged for {prov:?} (job={:?} check={:?})",
                        stamp.job_tier,
                        stamp.check_tier
                    );
                    let expect_untrusted = is_fork || !read_excludes_fork;
                    assert_eq!(
                        stamp.job_tier == TrustTier::UntrustedFork,
                        expect_untrusted,
                        "the fork verdict for {prov:?}"
                    );
                }
            }
        }
    }
}
