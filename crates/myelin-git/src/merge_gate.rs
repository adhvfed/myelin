use crate::check_status::{
    is_acceptable_satisfaction, CheckContext, CheckProvider, CheckState, CheckStatusProjection,
    CheckStatusRow, GitOid,
};

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct MergeGatePolicy {
    pub required: Vec<CheckContext>,
}

impl MergeGatePolicy {
    pub fn from_required_contexts<S: AsRef<str>>(
        contexts: &[S],
    ) -> Result<MergeGatePolicy, RequiredContextParseError> {
        let mut required = Vec::with_capacity(contexts.len());
        for c in contexts {
            required.push(parse_required_context(c.as_ref())?);
        }
        Ok(MergeGatePolicy { required })
    }

    pub fn requires(&self, context: &CheckContext) -> bool {
        self.required.contains(context)
    }

    pub fn is_empty(&self) -> bool {
        self.required.is_empty()
    }
}

pub fn parse_required_context(s: &str) -> Result<CheckContext, RequiredContextParseError> {
    let s = s.trim();
    if s.is_empty() {
        return Err(RequiredContextParseError::Empty);
    }
    match s.split_once('/') {
        Some(("ci", name)) if !name.is_empty() => Ok(CheckContext {
            provider: CheckProvider::Ci,
            name: name.to_string(),
        }),
        Some(("external", name)) if !name.is_empty() => Ok(CheckContext {
            provider: CheckProvider::External,
            name: name.to_string(),
        }),
        Some(("ci", "")) | Some(("external", "")) => {
            Err(RequiredContextParseError::EmptyName { raw: s.to_string() })
        }
        Some(_) => Ok(CheckContext {
            provider: CheckProvider::Ci,
            name: s.to_string(),
        }),
        None => Ok(CheckContext {
            provider: CheckProvider::Ci,
            name: s.to_string(),
        }),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RequiredContextParseError {
    Empty,
    EmptyName {
        raw: String,
    },
}

impl std::fmt::Display for RequiredContextParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RequiredContextParseError::Empty => {
                write!(f, "a required_contexts entry was empty")
            }
            RequiredContextParseError::EmptyName { raw } => {
                write!(
                    f,
                    "a required_contexts entry has a provider but no name: {raw:?}"
                )
            }
        }
    }
}

impl std::error::Error for RequiredContextParseError {}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MergeGateOutcome {
    Admitted,
    Blocked {
        unmet: Vec<UnmetContext>,
    },
}

impl MergeGateOutcome {
    pub fn is_admitted(&self) -> bool {
        matches!(self, MergeGateOutcome::Admitted)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UnmetContext {
    pub context: CheckContext,
    pub reason: UnmetReason,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum UnmetReason {
    Missing,
    NotGreen {
        state: CheckState,
    },
    CostUnsettled,
    UntrustedForkNeutral,
}

pub fn evaluate_merge_gate(
    policy: &MergeGatePolicy,
    projection: &CheckStatusProjection,
    head_oid: &GitOid,
    endorsed_contexts: &[CheckContext],
) -> MergeGateOutcome {
    let mut unmet: Vec<UnmetContext> = Vec::new();
    for ctx in &policy.required {
        match resolve_context(projection, head_oid, ctx, endorsed_contexts) {
            None => {}
            Some(reason) => unmet.push(UnmetContext {
                context: ctx.clone(),
                reason,
            }),
        }
    }
    if unmet.is_empty() {
        MergeGateOutcome::Admitted
    } else {
        MergeGateOutcome::Blocked { unmet }
    }
}

fn resolve_context(
    projection: &CheckStatusProjection,
    head_oid: &GitOid,
    ctx: &CheckContext,
    endorsed_contexts: &[CheckContext],
) -> Option<UnmetReason> {
    let key = crate::check_status::CheckKey {
        commit_oid: head_oid.clone(),
        context: ctx.clone(),
    };
    match projection.current(&key) {
        None => Some(UnmetReason::Missing),
        Some(row) => classify_row(row, endorsed_contexts.contains(ctx)),
    }
}

fn classify_row(row: &CheckStatusRow, endorsed: bool) -> Option<UnmetReason> {
    if is_acceptable_satisfaction(row, endorsed) {
        return None;
    }
    if !row.state.is_success() {
        Some(UnmetReason::NotGreen { state: row.state })
    } else if !row.cost_settled {
        Some(UnmetReason::CostUnsettled)
    } else {
        Some(UnmetReason::UntrustedForkNeutral)
    }
}

pub fn evaluate_merge_gate_row(
    row: Option<&CheckStatusRow>,
    endorsed: bool,
) -> Option<UnmetReason> {
    match row {
        None => Some(UnmetReason::Missing),
        Some(r) => classify_row(r, endorsed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check_status::{CheckStatus, HumanisedRef, Timestamp, TrustTier};
    use myelin_tenancy::{ArtifactRef, TenantId};
    use std::collections::BTreeMap;

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
            summary: HumanisedRef {
                template_key: "ci.check.updated".into(),
                args: BTreeMap::new(),
            },
            started_at: Timestamp("2026-06-22T00:00:00Z".into()),
            completed_at: Some(Timestamp("2026-06-22T00:01:00Z".into())),
            cost_settled: true,
        }
    }

    #[test]
    fn parse_ci_provider_prefixed_context() {
        assert_eq!(
            parse_required_context("ci/build").unwrap(),
            CheckContext::ci("build")
        );
    }

    #[test]
    fn parse_external_provider_prefixed_context() {
        assert_eq!(
            parse_required_context("external/sonarcloud").unwrap(),
            CheckContext::external("sonarcloud")
        );
    }

    #[test]
    fn parse_bare_name_defaults_to_ci() {
        assert_eq!(
            parse_required_context("build").unwrap(),
            CheckContext::ci("build")
        );
    }

    #[test]
    fn parse_name_with_slash_keeps_provider_prefix() {
        assert_eq!(
            parse_required_context("ci/test/unit").unwrap(),
            CheckContext::ci("test/unit")
        );
    }

    #[test]
    fn parse_unknown_first_segment_is_a_ci_name() {
        assert_eq!(
            parse_required_context("team/foo").unwrap(),
            CheckContext::ci("team/foo")
        );
    }

    #[test]
    fn parse_empty_is_a_loud_error() {
        assert_eq!(
            parse_required_context("").unwrap_err(),
            RequiredContextParseError::Empty
        );
        assert_eq!(
            parse_required_context("   ").unwrap_err(),
            RequiredContextParseError::Empty
        );
    }

    #[test]
    fn parse_provider_without_name_is_a_loud_error() {
        assert_eq!(
            parse_required_context("ci/").unwrap_err(),
            RequiredContextParseError::EmptyName { raw: "ci/".into() }
        );
        assert_eq!(
            parse_required_context("external/").unwrap_err(),
            RequiredContextParseError::EmptyName {
                raw: "external/".into()
            }
        );
        assert_eq!(
            parse_required_context("ci/").unwrap_err().to_string(),
            "a required_contexts entry has a provider but no name: \"ci/\""
        );
        assert_eq!(
            RequiredContextParseError::Empty.to_string(),
            "a required_contexts entry was empty"
        );
    }

    #[test]
    fn outcome_predicates_distinguish_admit_from_block() {
        assert!(MergeGateOutcome::Admitted.is_admitted());
        assert!(!MergeGateOutcome::Blocked {
            unmet: vec![UnmetContext {
                context: CheckContext::ci("build"),
                reason: UnmetReason::Missing,
            }]
        }
        .is_admitted());
    }

    #[test]
    fn policy_is_empty_distinguishes_empty_from_non_empty() {
        assert!(MergeGatePolicy::default().is_empty());
        assert!(!MergeGatePolicy::from_required_contexts(&["ci/build"])
            .unwrap()
            .is_empty());
    }

    #[test]
    fn policy_from_ruleset_strings() {
        let policy =
            MergeGatePolicy::from_required_contexts(&["ci/build", "ci/test", "external/scan"])
                .unwrap();
        assert_eq!(policy.required.len(), 3);
        assert!(policy.requires(&CheckContext::ci("build")));
        assert!(policy.requires(&CheckContext::external("scan")));
        assert!(!policy.requires(&CheckContext::ci("lint")));
    }

    #[test]
    fn policy_from_ruleset_propagates_parse_error() {
        assert!(MergeGatePolicy::from_required_contexts(&["ci/build", ""]).is_err());
    }

    #[test]
    fn gate_admits_when_all_required_green_trusted() {
        let mut proj = CheckStatusProjection::new();
        let head = GitOid("h1".into());
        proj.apply(&fact(
            "h1",
            CheckContext::ci("build"),
            1,
            CheckState::Success,
            TrustTier::Trusted,
        ));
        proj.apply(&fact(
            "h1",
            CheckContext::ci("test"),
            1,
            CheckState::Success,
            TrustTier::Trusted,
        ));

        let policy = MergeGatePolicy::from_required_contexts(&["ci/build", "ci/test"]).unwrap();
        assert_eq!(
            evaluate_merge_gate(&policy, &proj, &head, &[]),
            MergeGateOutcome::Admitted
        );
    }

    #[test]
    fn gate_blocks_on_missing_required_context() {
        let mut proj = CheckStatusProjection::new();
        let head = GitOid("h1".into());
        proj.apply(&fact(
            "h1",
            CheckContext::ci("build"),
            1,
            CheckState::Success,
            TrustTier::Trusted,
        ));

        let policy = MergeGatePolicy::from_required_contexts(&["ci/build", "ci/test"]).unwrap();
        match evaluate_merge_gate(&policy, &proj, &head, &[]) {
            MergeGateOutcome::Blocked { unmet } => {
                assert_eq!(unmet.len(), 1);
                assert_eq!(unmet[0].context, CheckContext::ci("test"));
                assert_eq!(unmet[0].reason, UnmetReason::Missing);
            }
            MergeGateOutcome::Admitted => panic!("a missing required context must BLOCK"),
        }
    }

    #[test]
    fn gate_blocks_on_failing_required_context() {
        let mut proj = CheckStatusProjection::new();
        let head = GitOid("h1".into());
        proj.apply(&fact(
            "h1",
            CheckContext::ci("build"),
            1,
            CheckState::Failure,
            TrustTier::Trusted,
        ));

        let policy = MergeGatePolicy::from_required_contexts(&["ci/build"]).unwrap();
        match evaluate_merge_gate(&policy, &proj, &head, &[]) {
            MergeGateOutcome::Blocked { unmet } => {
                assert_eq!(
                    unmet[0].reason,
                    UnmetReason::NotGreen {
                        state: CheckState::Failure
                    }
                );
            }
            MergeGateOutcome::Admitted => panic!("a failing required context must BLOCK"),
        }
    }

    #[test]
    fn gate_blocks_on_stale_pending_required_context() {
        let mut proj = CheckStatusProjection::new();
        let head = GitOid("h1".into());
        proj.apply(&fact(
            "h1",
            CheckContext::ci("build"),
            1,
            CheckState::InProgress,
            TrustTier::Trusted,
        ));

        let policy = MergeGatePolicy::from_required_contexts(&["ci/build"]).unwrap();
        match evaluate_merge_gate(&policy, &proj, &head, &[]) {
            MergeGateOutcome::Blocked { unmet } => {
                assert_eq!(
                    unmet[0].reason,
                    UnmetReason::NotGreen {
                        state: CheckState::InProgress
                    }
                );
            }
            MergeGateOutcome::Admitted => panic!("a pending required context must BLOCK"),
        }
    }

    #[test]
    fn gate_blocks_un_endorsed_untrusted_fork_success() {
        let mut proj = CheckStatusProjection::new();
        let head = GitOid("h1".into());
        proj.apply(&fact(
            "h1",
            CheckContext::ci("build"),
            1,
            CheckState::Success,
            TrustTier::UntrustedFork,
        ));

        let policy = MergeGatePolicy::from_required_contexts(&["ci/build"]).unwrap();
        match evaluate_merge_gate(&policy, &proj, &head, &[]) {
            MergeGateOutcome::Blocked { unmet } => {
                assert_eq!(unmet[0].reason, UnmetReason::UntrustedForkNeutral);
            }
            MergeGateOutcome::Admitted => panic!("an un-endorsed fork success must BLOCK"),
        }
        assert_eq!(
            evaluate_merge_gate(&policy, &proj, &head, &[CheckContext::ci("build")]),
            MergeGateOutcome::Admitted,
            "a maintainer-endorsed fork success admits"
        );
    }

    #[test]
    fn empty_required_set_admits_on_checks_alone() {
        let proj = CheckStatusProjection::new();
        let policy = MergeGatePolicy::default();
        assert!(policy.is_empty());
        assert_eq!(
            evaluate_merge_gate(&policy, &proj, &GitOid("h1".into()), &[]),
            MergeGateOutcome::Admitted
        );
    }

    #[test]
    fn the_in_memory_and_row_paths_agree() {
        let f = fact(
            "h1",
            CheckContext::ci("build"),
            1,
            CheckState::Success,
            TrustTier::Trusted,
        );
        let row = CheckStatusRow::from_fact(&f);
        assert_eq!(
            evaluate_merge_gate_row(Some(&row), false),
            None,
            "trusted success satisfies"
        );
        assert_eq!(
            evaluate_merge_gate_row(None, false),
            Some(UnmetReason::Missing),
            "absent → missing"
        );

        let fork = CheckStatusRow::from_fact(&fact(
            "h1",
            CheckContext::ci("build"),
            1,
            CheckState::Success,
            TrustTier::UntrustedFork,
        ));
        assert_eq!(
            evaluate_merge_gate_row(Some(&fork), false),
            Some(UnmetReason::UntrustedForkNeutral),
            "un-endorsed fork success → neutral"
        );
        assert_eq!(
            evaluate_merge_gate_row(Some(&fork), true),
            None,
            "endorsed fork success satisfies"
        );
    }
}
