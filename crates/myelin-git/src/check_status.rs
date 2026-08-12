use myelin_events::taxonomy::new_tokens::CI_CHECK_UPDATED;
use myelin_events::{EventEnvelope, EventHandler, HandleOutcome, Reason, SubjectPattern};
use myelin_tenancy::{ArtifactRef, TenantId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Mutex;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct GitOid(pub String);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CheckContext {
    pub provider: CheckProvider,
    pub name: String,
}

impl CheckContext {
    pub fn ci(name: impl Into<String>) -> CheckContext {
        CheckContext {
            provider: CheckProvider::Ci,
            name: name.into(),
        }
    }

    pub fn external(name: impl Into<String>) -> CheckContext {
        CheckContext {
            provider: CheckProvider::External,
            name: name.into(),
        }
    }

    pub fn policy_token(&self) -> String {
        match self.provider {
            CheckProvider::Ci => format!("ci/{}", self.name),
            CheckProvider::External => format!("external/{}", self.name),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckProvider {
    Ci,
    External,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckState {
    Queued,
    InProgress,
    Success,
    Failure,
    Error,
    Neutral,
    Cancelled,
}

impl CheckState {
    pub fn is_success(self) -> bool {
        matches!(self, CheckState::Success)
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            CheckState::Success
                | CheckState::Failure
                | CheckState::Error
                | CheckState::Neutral
                | CheckState::Cancelled
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustTier {
    Trusted,
    UntrustedFork,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HumanisedRef {
    pub template_key: String,
    pub args: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timestamp(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckStatus {
    pub tenant: TenantId,
    pub repo: ArtifactRef,
    pub commit_oid: GitOid,
    pub context: CheckContext,
    pub state: CheckState,
    pub required: bool,
    pub run: ArtifactRef,
    pub run_attempt: u32,
    pub trust_tier: TrustTier,
    pub details_ref: ArtifactRef,
    pub summary: HumanisedRef,
    pub started_at: Timestamp,
    pub completed_at: Option<Timestamp>,
    pub cost_settled: bool,
}

impl CheckStatus {
    pub fn key(&self) -> CheckKey {
        CheckKey {
            commit_oid: self.commit_oid.clone(),
            context: self.context.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CheckKey {
    pub commit_oid: GitOid,
    pub context: CheckContext,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckStatusRow {
    pub tenant: TenantId,
    pub commit_oid: GitOid,
    pub context: CheckContext,
    pub state: CheckState,
    pub run: ArtifactRef,
    pub run_attempt: u32,
    pub trust_tier: TrustTier,
    pub details_ref: ArtifactRef,
    pub summary: HumanisedRef,
    pub cost_settled: bool,
}

impl CheckStatusRow {
    pub fn from_fact(fact: &CheckStatus) -> CheckStatusRow {
        CheckStatusRow {
            tenant: fact.tenant.clone(),
            commit_oid: fact.commit_oid.clone(),
            context: fact.context.clone(),
            state: fact.state,
            run: fact.run.clone(),
            run_attempt: fact.run_attempt,
            trust_tier: fact.trust_tier,
            details_ref: fact.details_ref.clone(),
            summary: fact.summary.clone(),
            cost_settled: fact.cost_settled,
        }
    }

    pub fn key(&self) -> CheckKey {
        CheckKey {
            commit_oid: self.commit_oid.clone(),
            context: self.context.clone(),
        }
    }
}

pub const CHECK_STATUS_PROJECTION_DDL: &str = crate::check_status_store::CREATE_CHECK_STATUS_DDL;

pub fn supersedes(incoming_attempt: u32, stored_attempt: u32) -> bool {
    incoming_attempt >= stored_attempt
}

#[derive(Debug, Default, Clone)]
pub struct CheckStatusProjection {
    rows: BTreeMap<CheckKey, CheckStatusRow>,
}

impl CheckStatusProjection {
    pub fn new() -> CheckStatusProjection {
        CheckStatusProjection::default()
    }

    pub fn apply(&mut self, fact: &CheckStatus) -> ApplyOutcome {
        let key = fact.key();
        match self.rows.get(&key) {
            Some(stored) if !supersedes(fact.run_attempt, stored.run_attempt) => {
                ApplyOutcome::DroppedStale {
                    incoming_attempt: fact.run_attempt,
                    current_attempt: stored.run_attempt,
                }
            }
            _ => {
                self.rows.insert(key, CheckStatusRow::from_fact(fact));
                ApplyOutcome::Superseded {
                    current_attempt: fact.run_attempt,
                }
            }
        }
    }

    pub fn current(&self, key: &CheckKey) -> Option<&CheckStatusRow> {
        self.rows.get(key)
    }

    pub fn rows_for_commit<'a>(
        &'a self,
        commit_oid: &'a GitOid,
    ) -> impl Iterator<Item = &'a CheckStatusRow> + 'a {
        self.rows
            .iter()
            .filter(move |(k, _)| &k.commit_oid == commit_oid)
            .map(|(_, v)| v)
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyOutcome {
    Superseded {
        current_attempt: u32,
    },
    DroppedStale {
        incoming_attempt: u32,
        current_attempt: u32,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RequiredSetPolicy {
    pub required_contexts: Vec<CheckContext>,
}

impl RequiredSetPolicy {
    pub fn requiring(required_contexts: Vec<CheckContext>) -> RequiredSetPolicy {
        RequiredSetPolicy { required_contexts }
    }

    pub fn requires(&self, context: &CheckContext) -> bool {
        self.required_contexts.contains(context)
    }
}

pub fn is_acceptable_satisfaction(row: &CheckStatusRow, fork_endorsed: bool) -> bool {
    if !row.state.is_success() || !row.cost_settled {
        return false;
    }
    match row.trust_tier {
        TrustTier::Trusted => true,
        TrustTier::UntrustedFork => fork_endorsed,
    }
}

pub fn gate_outcome(
    policy: &RequiredSetPolicy,
    projection: &CheckStatusProjection,
    commit_oid: &GitOid,
    endorsed_contexts: &[CheckContext],
) -> GateOutcome {
    let mut unmet: Vec<CheckContext> = Vec::new();
    for ctx in &policy.required_contexts {
        let key = CheckKey {
            commit_oid: commit_oid.clone(),
            context: ctx.clone(),
        };
        match projection.current(&key) {
            None => unmet.push(ctx.clone()),
            Some(row) => {
                let endorsed = endorsed_contexts.contains(ctx);
                if !is_acceptable_satisfaction(row, endorsed) {
                    unmet.push(ctx.clone());
                }
            }
        }
    }
    if unmet.is_empty() {
        GateOutcome::AllRequiredGreen
    } else {
        GateOutcome::Blocked { unmet }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GateOutcome {
    AllRequiredGreen,
    Blocked { unmet: Vec<CheckContext> },
}

fn check_status_subjects() -> &'static [SubjectPattern] {
    use std::sync::OnceLock;
    static SUBJECTS: OnceLock<Vec<SubjectPattern>> = OnceLock::new();
    SUBJECTS
        .get_or_init(|| vec![SubjectPattern("myelin://".to_string())])
        .as_slice()
}

#[derive(Debug, Default)]
pub struct CheckStatusConsumer {
    projection: Mutex<CheckStatusProjection>,
    applied: Mutex<u64>,
    dropped_stale: Mutex<u64>,
}

impl CheckStatusConsumer {
    pub fn new() -> CheckStatusConsumer {
        CheckStatusConsumer::default()
    }

    pub fn decode(payload: &serde_json::Value) -> Result<CheckStatus, Reason> {
        serde_json::from_value(payload.clone()).map_err(|e| {
            Reason(format!(
                "ci.check.updated payload is not a valid CheckStatus fact: {e}"
            ))
        })
    }

    pub fn projection(&self) -> CheckStatusProjection {
        self.projection
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn applied_count(&self) -> u64 {
        *self.applied.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn dropped_stale_count(&self) -> u64 {
        *self.dropped_stale.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl EventHandler for CheckStatusConsumer {
    fn subjects(&self) -> &'static [SubjectPattern] {
        check_status_subjects()
    }

    fn handle(&self, ev: &EventEnvelope, _tx: &mut myelin_events::HandlerTx<'_>) -> HandleOutcome {
        if ev.type_.0 != CI_CHECK_UPDATED {
            return HandleOutcome::NonRetryable(Reason(format!(
                "check_status consumer received a non-ci.check.updated event: {}",
                ev.type_.0
            )));
        }
        let fact = match Self::decode(&ev.payload) {
            Ok(f) => f,
            Err(reason) => return HandleOutcome::NonRetryable(reason),
        };
        let mut proj = self.projection.lock().unwrap_or_else(|e| e.into_inner());
        match proj.apply(&fact) {
            ApplyOutcome::Superseded { .. } => {
                *self.applied.lock().unwrap_or_else(|e| e.into_inner()) += 1;
            }
            ApplyOutcome::DroppedStale { .. } => {
                *self.dropped_stale.lock().unwrap_or_else(|e| e.into_inner()) += 1;
            }
        }
        HandleOutcome::Done
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(key: &str) -> HumanisedRef {
        HumanisedRef {
            template_key: key.into(),
            args: BTreeMap::new(),
        }
    }

    #[test]
    fn typed_contexts_render_the_full_branch_protection_token() {
        assert_eq!(CheckContext::ci("build").policy_token(), "ci/build");
        assert_eq!(CheckContext::ci("test/unit").policy_token(), "ci/test/unit");
        assert_eq!(
            CheckContext::external("sonarcloud").policy_token(),
            "external/sonarcloud"
        );
    }

    fn fact(
        commit: &str,
        ctx: CheckContext,
        attempt: u32,
        state: CheckState,
        trust: TrustTier,
    ) -> CheckStatus {
        CheckStatus {
            tenant: TenantId("acme".into()),
            repo: ArtifactRef("myelin://acme/git/repo/core".into()),
            commit_oid: GitOid(commit.into()),
            context: ctx,
            state,
            required: true,
            run: ArtifactRef("myelin://acme/ci/run/1".into()),
            run_attempt: attempt,
            trust_tier: trust,
            details_ref: ArtifactRef("myelin://acme/ci/run/1#step-3".into()),
            summary: h("ci.check.updated"),
            started_at: Timestamp("2026-06-21T00:00:00Z".into()),
            completed_at: Some(Timestamp("2026-06-21T00:01:00Z".into())),
            cost_settled: true,
        }
    }

    #[test]
    fn check_status_serialises_to_the_frozen_5_9_shape() {
        let f = fact(
            "abc123",
            CheckContext::ci("build"),
            1,
            CheckState::Success,
            TrustTier::Trusted,
        );
        let v = serde_json::to_value(&f).unwrap();
        assert_eq!(v["commit_oid"], "abc123");
        assert_eq!(v["context"]["provider"], "ci");
        assert_eq!(v["context"]["name"], "build");
        assert_eq!(v["state"], "success");
        assert_eq!(v["trust_tier"], "trusted");
        assert_eq!(v["run_attempt"], 1);
        assert_eq!(v["required"], true);
        assert_eq!(v["cost_settled"], true);
        assert_eq!(v["run"], "myelin://acme/ci/run/1");
        assert_eq!(v["details_ref"], "myelin://acme/ci/run/1#step-3");
        assert_eq!(v["summary"]["template_key"], "ci.check.updated");
        let back: CheckStatus = serde_json::from_value(v).unwrap();
        assert_eq!(back, f);
    }

    #[test]
    fn opaque_bus_payload_decodes_to_the_consumer_view() {
        let f = fact(
            "abc123",
            CheckContext::ci("test"),
            2,
            CheckState::Failure,
            TrustTier::Trusted,
        );
        let opaque: serde_json::Value = serde_json::to_value(&f).unwrap();
        let decoded: CheckStatus = serde_json::from_value(opaque).unwrap();
        assert_eq!(decoded, f);
        assert_eq!(
            decoded.key(),
            CheckKey {
                commit_oid: GitOid("abc123".into()),
                context: CheckContext::ci("test"),
            }
        );
    }

    #[test]
    fn supersession_is_monotonic_on_run_attempt() {
        assert!(supersedes(2, 1), "a higher attempt supersedes");
        assert!(
            supersedes(1, 1),
            "the same attempt is an idempotent re-apply (>=)"
        );
        assert!(
            !supersedes(1, 2),
            "a LOWER attempt is dropped (stale re-delivery)"
        );
    }

    #[test]
    fn late_lower_attempt_is_dropped_not_applied() {
        let mut proj = CheckStatusProjection::new();
        let build = CheckContext::ci("build");

        assert_eq!(
            proj.apply(&fact(
                "c1",
                build.clone(),
                1,
                CheckState::Failure,
                TrustTier::Trusted
            )),
            ApplyOutcome::Superseded { current_attempt: 1 }
        );
        assert_eq!(
            proj.apply(&fact(
                "c1",
                build.clone(),
                2,
                CheckState::Success,
                TrustTier::Trusted
            )),
            ApplyOutcome::Superseded { current_attempt: 2 }
        );

        assert_eq!(
            proj.apply(&fact(
                "c1",
                build.clone(),
                1,
                CheckState::Failure,
                TrustTier::Trusted
            )),
            ApplyOutcome::DroppedStale {
                incoming_attempt: 1,
                current_attempt: 2
            }
        );

        let key = CheckKey {
            commit_oid: GitOid("c1".into()),
            context: build,
        };
        let row = proj.current(&key).unwrap();
        assert_eq!(row.run_attempt, 2);
        assert_eq!(row.state, CheckState::Success);
    }

    #[test]
    fn one_current_row_per_key() {
        let mut proj = CheckStatusProjection::new();
        proj.apply(&fact(
            "c1",
            CheckContext::ci("build"),
            1,
            CheckState::Success,
            TrustTier::Trusted,
        ));
        proj.apply(&fact(
            "c1",
            CheckContext::ci("test"),
            1,
            CheckState::Success,
            TrustTier::Trusted,
        ));
        assert_eq!(proj.len(), 2, "two distinct contexts → two rows");
        proj.apply(&fact(
            "c1",
            CheckContext::ci("build"),
            5,
            CheckState::Success,
            TrustTier::Trusted,
        ));
        assert_eq!(
            proj.len(),
            2,
            "supersession is in-place, never a duplicate row"
        );
    }

    #[test]
    fn gate_green_when_all_required_contexts_succeed_trusted() {
        let mut proj = CheckStatusProjection::new();
        let build = CheckContext::ci("build");
        let test = CheckContext::ci("test");
        proj.apply(&fact(
            "c1",
            build.clone(),
            1,
            CheckState::Success,
            TrustTier::Trusted,
        ));
        proj.apply(&fact(
            "c1",
            test.clone(),
            1,
            CheckState::Success,
            TrustTier::Trusted,
        ));

        let policy = RequiredSetPolicy::requiring(vec![build, test]);
        assert_eq!(
            gate_outcome(&policy, &proj, &GitOid("c1".into()), &[]),
            GateOutcome::AllRequiredGreen
        );
    }

    #[test]
    fn gate_blocks_on_missing_or_failing_required_context() {
        let mut proj = CheckStatusProjection::new();
        let build = CheckContext::ci("build");
        let test = CheckContext::ci("test");
        proj.apply(&fact(
            "c1",
            build.clone(),
            1,
            CheckState::Success,
            TrustTier::Trusted,
        ));
        proj.apply(&fact(
            "c1",
            test.clone(),
            1,
            CheckState::Failure,
            TrustTier::Trusted,
        ));
        let lint = CheckContext::ci("lint");

        let policy = RequiredSetPolicy::requiring(vec![build, test.clone(), lint.clone()]);
        let outcome = gate_outcome(&policy, &proj, &GitOid("c1".into()), &[]);
        match outcome {
            GateOutcome::Blocked { unmet } => {
                assert!(unmet.contains(&test), "the failing context is unmet");
                assert!(unmet.contains(&lint), "the missing context is unmet");
                assert_eq!(unmet.len(), 2);
            }
            GateOutcome::AllRequiredGreen => panic!("must block"),
        }
    }

    #[test]
    fn untrusted_fork_success_is_neutral_until_endorsed() {
        let mut proj = CheckStatusProjection::new();
        let build = CheckContext::ci("build");
        proj.apply(&fact(
            "c1",
            build.clone(),
            1,
            CheckState::Success,
            TrustTier::UntrustedFork,
        ));

        let policy = RequiredSetPolicy::requiring(vec![build.clone()]);
        let commit = GitOid("c1".into());

        assert_eq!(
            gate_outcome(&policy, &proj, &commit, &[]),
            GateOutcome::Blocked {
                unmet: vec![build.clone()]
            }
        );

        assert_eq!(
            gate_outcome(&policy, &proj, &commit, std::slice::from_ref(&build)),
            GateOutcome::AllRequiredGreen
        );
    }

    #[test]
    fn rerun_trusted_supersedes_fork_and_greens_the_gate() {
        let mut proj = CheckStatusProjection::new();
        let build = CheckContext::ci("build");
        proj.apply(&fact(
            "c1",
            build.clone(),
            1,
            CheckState::Success,
            TrustTier::UntrustedFork,
        ));
        proj.apply(&fact(
            "c1",
            build.clone(),
            2,
            CheckState::Success,
            TrustTier::Trusted,
        ));

        let policy = RequiredSetPolicy::requiring(vec![build]);
        assert_eq!(
            gate_outcome(&policy, &proj, &GitOid("c1".into()), &[]),
            GateOutcome::AllRequiredGreen,
            "re-run trusted greens the gate with no explicit endorsement"
        );
    }

    #[test]
    fn projection_ddl_is_keyed_on_commit_oid_and_context() {
        assert!(CHECK_STATUS_PROJECTION_DDL.contains("CREATE TABLE check_status"));
        assert!(CHECK_STATUS_PROJECTION_DDL
            .contains("tenant_id, region, repo_ref, commit_oid, context_provider, context_name"));
        assert!(CHECK_STATUS_PROJECTION_DDL.contains("run_attempt"));
        assert!(CHECK_STATUS_PROJECTION_DDL.contains("trust_tier"));
    }

    #[test]
    fn row_materialises_from_fact() {
        let f = fact(
            "c1",
            CheckContext::ci("build"),
            3,
            CheckState::Success,
            TrustTier::Trusted,
        );
        let row = CheckStatusRow::from_fact(&f);
        assert_eq!(row.key(), f.key());
        assert_eq!(row.run_attempt, 3);
        assert_eq!(row.trust_tier, TrustTier::Trusted);
        assert!(row.cost_settled);
    }
}
