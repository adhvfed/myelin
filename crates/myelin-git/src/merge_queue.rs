use crate::check_status::{
    is_acceptable_satisfaction, CheckContext, CheckKey, CheckStatusProjection, GitOid,
};
use crate::merge_gate::{evaluate_merge_gate, MergeGateOutcome, MergeGatePolicy};
use myelin_flow::{ActivityError, MergePerformer, MergeRequest};

pub struct GitMergePerformer<'a, F>
where
    F: Fn(&MergeRequest) -> Result<String, ActivityError>,
{
    projection: &'a CheckStatusProjection,
    head_oid: GitOid,
    policy: MergeGatePolicy,
    endorsed_contexts: Vec<CheckContext>,
    merge_fn: F,
}

impl<'a, F> GitMergePerformer<'a, F>
where
    F: Fn(&MergeRequest) -> Result<String, ActivityError>,
{
    pub fn new(
        projection: &'a CheckStatusProjection,
        head_oid: GitOid,
        policy: MergeGatePolicy,
        endorsed_contexts: Vec<CheckContext>,
        merge_fn: F,
    ) -> GitMergePerformer<'a, F> {
        GitMergePerformer {
            projection,
            head_oid,
            policy,
            endorsed_contexts,
            merge_fn,
        }
    }

    pub fn gate_outcome(&self) -> MergeGateOutcome {
        evaluate_merge_gate(
            &self.policy,
            self.projection,
            &self.head_oid,
            &self.endorsed_contexts,
        )
    }

    pub fn context_satisfied(&self, context: &CheckContext) -> bool {
        let key = CheckKey {
            commit_oid: self.head_oid.clone(),
            context: context.clone(),
        };
        match self.projection.current(&key) {
            None => false,
            Some(row) => is_acceptable_satisfaction(row, self.endorsed_contexts.contains(context)),
        }
    }
}

impl<F> MergePerformer for GitMergePerformer<'_, F>
where
    F: Fn(&MergeRequest) -> Result<String, ActivityError>,
{
    fn merge(&self, request: &MergeRequest) -> Result<String, ActivityError> {
        match self.gate_outcome() {
            MergeGateOutcome::Admitted => (self.merge_fn)(request),
            MergeGateOutcome::Blocked { unmet } => {
                let names: Vec<String> = unmet
                    .iter()
                    .map(|u| format!("{}/{}", provider_label(&u.context), u.context.name))
                    .collect();
                Err(ActivityError::retryable(format!(
                    "the merge gate did not admit: the required check(s) {} are not green-and-current \
                     with an acceptable trust posture (an un-endorsed fork success is neutral for \
                     gating). The pull request was not merged.",
                    names.join(", ")
                )))
            }
        }
    }
}

fn provider_label(context: &CheckContext) -> &'static str {
    use crate::check_status::CheckProvider;
    match context.provider {
        CheckProvider::Ci => "ci",
        CheckProvider::External => "external",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check_status::{
        CheckState, CheckStatus, CheckStatusProjection, HumanisedRef, Timestamp, TrustTier,
    };
    use myelin_tenancy::{ArtifactRef, TenantId};
    use std::cell::Cell;
    use std::collections::BTreeMap;

    const HEAD: &str = "deadbeefcafe";
    const REPO: &str = "myelin://acme/git/repo/core";

    fn fact(context: &str, attempt: u32, state: CheckState, trust: TrustTier) -> CheckStatus {
        CheckStatus {
            tenant: TenantId("acme".into()),
            repo: ArtifactRef(REPO.into()),
            commit_oid: GitOid(HEAD.into()),
            context: CheckContext::ci(context),
            state,
            required: true,
            run: ArtifactRef(format!("myelin://acme/ci/run/{attempt}")),
            run_attempt: attempt,
            trust_tier: trust,
            details_ref: ArtifactRef(format!("myelin://acme/ci/run/{attempt}#step-2")),
            summary: HumanisedRef {
                template_key: "ci.check.updated".into(),
                args: BTreeMap::new(),
            },
            started_at: Timestamp("2026-06-22T00:00:00Z".into()),
            completed_at: Some(Timestamp("2026-06-22T00:01:00Z".into())),
            cost_settled: true,
        }
    }

    fn request() -> MergeRequest {
        MergeRequest {
            pr_ref: format!("{REPO}#pr-7"),
            target_ref: "refs/heads/main".into(),
            speculative_commit_oid: HEAD.into(),
            required_contexts: vec!["ci/build".into(), "ci/test".into()],
        }
    }

    fn policy() -> MergeGatePolicy {
        MergeGatePolicy::from_required_contexts(&["ci/build", "ci/test"]).unwrap()
    }

    #[test]
    fn admits_and_merges_when_all_required_trusted_green() {
        let mut proj = CheckStatusProjection::new();
        proj.apply(&fact("build", 1, CheckState::Success, TrustTier::Trusted));
        proj.apply(&fact("test", 1, CheckState::Success, TrustTier::Trusted));

        let merges = Cell::new(0u32);
        let perf = GitMergePerformer::new(&proj, GitOid(HEAD.into()), policy(), vec![], |r| {
            merges.set(merges.get() + 1);
            Ok(format!("merged-{}", r.speculative_commit_oid))
        });
        assert!(matches!(perf.gate_outcome(), MergeGateOutcome::Admitted));
        let oid = perf.merge(&request()).expect("admitted → merge");
        assert_eq!(oid, "merged-deadbeefcafe");
        assert_eq!(merges.get(), 1, "the actual merge ran EXACTLY once");
    }

    #[test]
    fn refuses_un_endorsed_fork_success() {
        let mut proj = CheckStatusProjection::new();
        proj.apply(&fact("build", 1, CheckState::Success, TrustTier::Trusted));
        proj.apply(&fact(
            "test",
            1,
            CheckState::Success,
            TrustTier::UntrustedFork,
        ));

        let merges = Cell::new(0u32);
        let perf = GitMergePerformer::new(&proj, GitOid(HEAD.into()), policy(), vec![], |_r| {
            merges.set(merges.get() + 1);
            Ok("should-not-run".into())
        });
        assert!(matches!(
            perf.gate_outcome(),
            MergeGateOutcome::Blocked { .. }
        ));
        let err = perf
            .merge(&request())
            .expect_err("an un-endorsed fork success must refuse the merge");
        assert!(
            err.detail().contains("the merge gate did not admit"),
            "humanised: {}",
            err
        );
        assert!(
            err.detail().contains("ci/test"),
            "names the unmet context: {}",
            err
        );
        assert!(
            !err.detail().contains("Blocked"),
            "no raw gate struct in the reason: {}",
            err
        );
        assert_eq!(merges.get(), 0, "0 forks self-green their gate at merge");
    }

    #[test]
    fn endorsed_fork_success_admits_and_merges() {
        let mut proj = CheckStatusProjection::new();
        proj.apply(&fact("build", 1, CheckState::Success, TrustTier::Trusted));
        proj.apply(&fact(
            "test",
            1,
            CheckState::Success,
            TrustTier::UntrustedFork,
        ));

        let merges = Cell::new(0u32);
        let perf = GitMergePerformer::new(
            &proj,
            GitOid(HEAD.into()),
            policy(),
            vec![CheckContext::ci("test")],
            |_r| {
                merges.set(merges.get() + 1);
                Ok("merged".into())
            },
        );
        assert!(matches!(perf.gate_outcome(), MergeGateOutcome::Admitted));
        perf.merge(&request())
            .expect("an endorsed fork success admits");
        assert_eq!(merges.get(), 1, "the endorsed fork context merges once");
    }

    #[test]
    fn refuses_on_a_missing_required_context() {
        let mut proj = CheckStatusProjection::new();
        proj.apply(&fact("build", 1, CheckState::Success, TrustTier::Trusted));

        let perf = GitMergePerformer::new(&proj, GitOid(HEAD.into()), policy(), vec![], |_r| {
            panic!("must not merge with a missing required context")
        });
        assert!(
            !perf.context_satisfied(&CheckContext::ci("test")),
            "missing → unsatisfied"
        );
        let err = perf
            .merge(&request())
            .expect_err("a missing required context must refuse the merge");
        assert!(
            err.detail().contains("ci/test"),
            "names the missing context: {}",
            err
        );
    }

    #[test]
    fn admitted_gate_propagates_a_merge_conflict() {
        let mut proj = CheckStatusProjection::new();
        proj.apply(&fact("build", 1, CheckState::Success, TrustTier::Trusted));
        proj.apply(&fact("test", 1, CheckState::Success, TrustTier::Trusted));

        let perf = GitMergePerformer::new(&proj, GitOid(HEAD.into()), policy(), vec![], |_r| {
            Err(ActivityError::retryable("merge conflict"))
        });
        let err = perf.merge(&request()).expect_err("the conflict propagates");
        assert_eq!(err.detail(), "merge conflict");
    }

    #[test]
    fn context_satisfied_reads_trust_off_the_fact() {
        let mut proj = CheckStatusProjection::new();
        proj.apply(&fact("build", 1, CheckState::Success, TrustTier::Trusted));
        proj.apply(&fact(
            "fork",
            1,
            CheckState::Success,
            TrustTier::UntrustedFork,
        ));

        let unendorsed =
            GitMergePerformer::new(&proj, GitOid(HEAD.into()), policy(), vec![], |_r| {
                Ok(String::new())
            });
        assert!(
            unendorsed.context_satisfied(&CheckContext::ci("build")),
            "trusted success satisfies"
        );
        assert!(
            !unendorsed.context_satisfied(&CheckContext::ci("fork")),
            "an un-endorsed fork success does not satisfy"
        );

        let endorsed = GitMergePerformer::new(
            &proj,
            GitOid(HEAD.into()),
            policy(),
            vec![CheckContext::ci("fork")],
            |_r| Ok(String::new()),
        );
        assert!(
            endorsed.context_satisfied(&CheckContext::ci("fork")),
            "an endorsed fork success satisfies"
        );
    }
}
