use super::{DurableGitBackend, EnrichedPr, ObjectPromotion};
use myelin_git::check_status::{CheckContext, CheckState, CheckStatusRow, GitOid, TrustTier};
use myelin_git::core::RepoLoc;
use myelin_git::durable::{DurableError, DurableGitRepo};
use myelin_git::lifecycle::{BranchProtectionRuleset, ReviewState, ReviewVerdict};
use myelin_git::merge_gate::MergeGatePolicy;
use myelin_git::pr_store::{
    effective_ruleset, evaluate_merge, ChecksSummary, PrCrossListRecord, PrRecord,
};
use myelin_git::receive_pack::{
    evaluate_protected_ref_push, CrashPoint, PushOutcome, PushSession, RefStore, RejectReason,
};
use myelin_identity::Principal;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The ordinary durable-store provider plus the separately bounded protected-push admission lane.
pub struct GitDatabaseProviders {
    pub(super) primary: myelin_storage::SubstrateProvider,
    pub(super) check_admission: myelin_storage::SubstrateProvider,
}

impl GitDatabaseProviders {
    pub fn new(
        primary: myelin_storage::SubstrateProvider,
        check_admission: myelin_storage::SubstrateProvider,
    ) -> GitDatabaseProviders {
        GitDatabaseProviders {
            primary,
            check_admission,
        }
    }

    pub(super) fn into_projection(
        self,
        runtime: tokio::runtime::Handle,
    ) -> (
        myelin_storage::SubstrateProvider,
        myelin_git::check_status_store::PgCheckStatusProjection,
    ) {
        let projection = myelin_git::check_status_store::PgCheckStatusProjection::production(
            self.primary.clone(),
            self.check_admission,
            runtime,
        );
        (self.primary, projection)
    }
}

pub(super) enum ProtectedPushAdmissionError {
    Scope(DurableError),
    Policy(RejectReason),
    Projection(String),
}

pub(super) struct ProtectedPushMutation {
    repo: Arc<DurableGitRepo>,
    objects: Vec<(String, String, Vec<u8>)>,
    ref_store: RefStore,
    push: PushSession,
}

impl ProtectedPushMutation {
    pub(super) fn new(
        repo: Arc<DurableGitRepo>,
        objects: Vec<(String, String, Vec<u8>)>,
        ref_store: RefStore,
        push: PushSession,
    ) -> ProtectedPushMutation {
        ProtectedPushMutation {
            repo,
            objects,
            ref_store,
            push,
        }
    }
}

impl DurableGitBackend {
    /// Enrich one repository's bounded PR page from one protection-config read and one projected
    /// checks batch. Projection failure degrades summaries to unavailable without hiding PRs.
    pub(super) fn enrich_pr_records(
        &self,
        loc: &RepoLoc,
        principal: &Principal,
        viewer_pseudonym: &str,
        repo_slug: Option<&str>,
        records: Vec<PrRecord>,
    ) -> Vec<EnrichedPr> {
        let config = self.prs.get_protection(loc).ok();
        let config_readable = config.is_some();
        let config = config.flatten();
        let projected = self.projected_check_rows_for_records(loc, &records, principal);
        records
            .into_iter()
            .map(|mut rec| {
                let checks_readable = match &projected {
                    Ok(Some(rows)) => {
                        let head_oid = rec.head_oid.clone();
                        overlay_projected_check_rows(
                            &mut rec,
                            rows.get(&head_oid).map(Vec::as_slice).unwrap_or_default(),
                        );
                        true
                    }
                    Ok(None) => true,
                    Err(_) => false,
                };
                let summary = if config_readable && checks_readable {
                    let ruleset = effective_ruleset(config.as_ref(), &rec.base_ref);
                    rec.checks_summary(&ruleset)
                } else {
                    ChecksSummary::unavailable()
                };
                let you_requested = rec.is_review_requested_of(viewer_pseudonym);
                EnrichedPr {
                    rec,
                    summary,
                    you_requested,
                    repo_slug: repo_slug.map(str::to_string),
                }
            })
            .collect()
    }

    /// Cross-repository counterpart: batch projected heads per visible repository and fail static
    /// per repository if either protection or projection state is unreadable.
    pub(super) fn enrich_cross_pr_records(
        &self,
        tenant: &str,
        region: &str,
        principal: &Principal,
        viewer: &str,
        records: Vec<PrCrossListRecord>,
    ) -> Vec<EnrichedPr> {
        let mut configs = BTreeMap::new();
        let mut records_by_repo = BTreeMap::<String, Vec<PrRecord>>::new();
        for item in &records {
            configs.entry(item.repo_slug.clone()).or_insert_with(|| {
                self.prs
                    .get_protection(&Self::loc(tenant, region, &item.repo_slug))
                    .ok()
            });
            records_by_repo
                .entry(item.repo_slug.clone())
                .or_default()
                .push(item.record.clone());
        }
        let mut projected = BTreeMap::new();
        for (repo, records) in &records_by_repo {
            projected.insert(
                repo.clone(),
                self.projected_check_rows_for_records(
                    &Self::loc(tenant, region, repo),
                    records,
                    principal,
                ),
            );
        }
        records
            .into_iter()
            .map(|mut item| {
                let checks_readable = match projected.get(&item.repo_slug) {
                    Some(Ok(Some(rows))) => {
                        let head_oid = item.record.head_oid.clone();
                        overlay_projected_check_rows(
                            &mut item.record,
                            rows.get(&head_oid).map(Vec::as_slice).unwrap_or_default(),
                        );
                        true
                    }
                    Some(Ok(None)) => true,
                    _ => false,
                };
                let summary = match (configs.get(&item.repo_slug), checks_readable) {
                    (Some(Some(config)), true) => {
                        let ruleset = effective_ruleset(config.as_ref(), &item.record.base_ref);
                        item.record.checks_summary(&ruleset)
                    }
                    _ => ChecksSummary::unavailable(),
                };
                let you_requested = item.record.is_review_requested_of(viewer);
                EnrichedPr {
                    rec: item.record,
                    summary,
                    you_requested,
                    repo_slug: Some(item.repo_slug),
                }
            })
            .collect()
    }

    /// Evaluate production protected-ref updates and mutate the refs while holding the projection
    /// consumer's exact-commit admission locks. Check updates therefore serialize either before
    /// the admission read or after the ref mutation; no green-read → stale-CAS window exists.
    pub(super) fn receive_with_check_admission(
        &self,
        loc: &RepoLoc,
        principal: &Principal,
        mutation: ProtectedPushMutation,
        protected_updates: Vec<(usize, BranchProtectionRuleset)>,
        pusher_has_protected_push: bool,
    ) -> Result<Result<PushOutcome, myelin_events::OutboxError>, ProtectedPushAdmissionError> {
        let ProtectedPushMutation {
            repo,
            objects,
            ref_store,
            push,
        } = mutation;
        let Some(checks) = &self.checks else {
            let migration = ObjectPromotion {
                repo: &repo,
                objects: &objects,
            };
            return Ok(ref_store.receive(&push, &migration, CrashPoint::None));
        };
        if protected_updates.is_empty() {
            let migration = ObjectPromotion {
                repo: &repo,
                objects: &objects,
            };
            return Ok(ref_store.receive(&push, &migration, CrashPoint::None));
        }

        let scope =
            Self::verified_pr_scope(principal, loc).map_err(ProtectedPushAdmissionError::Scope)?;
        let repo_ref = format!("myelin://{}/git/repo/{}", loc.tenant, loc.repo);
        let heads = protected_updates
            .iter()
            .map(|(index, _)| GitOid(push.updates[*index].new_oid.0.clone()))
            .collect::<Vec<_>>();
        let updates = push.updates.clone();
        checks
            .with_admission_snapshot(&scope, &repo_ref, &heads, move |rows_by_commit| {
                for (index, ruleset) in &protected_updates {
                    let update = &updates[*index];
                    let rows = rows_by_commit
                        .get(&update.new_oid.0)
                        .map(Vec::as_slice)
                        .unwrap_or_default();
                    let (green, fork_unendorsed, endorsed) = check_facts_from_rows(rows);
                    evaluate_protected_ref_push(
                        &update.ref_name,
                        update.new_oid.is_zero(),
                        update.forced,
                        pusher_has_protected_push,
                        ruleset,
                        &GitOid(update.new_oid.0.clone()),
                        &green,
                        &fork_unendorsed,
                        &endorsed,
                    )
                    .map_err(ProtectedPushAdmissionError::Policy)?;
                }
                let migration = ObjectPromotion {
                    repo: &repo,
                    objects: &objects,
                };
                Ok(ref_store.receive(&push, &migration, CrashPoint::None))
            })
            .map_err(|error| ProtectedPushAdmissionError::Projection(error.to_string()))?
    }

    /// Resolve Git's recorded check facts for a direct protected-branch push. Production reads the
    /// per-commit projection and fails closed on any projection error; the PR-record fallback exists
    /// only for the in-memory test composition.
    pub(super) fn check_facts_for_head(
        &self,
        loc: &RepoLoc,
        head_oid: &str,
        principal: &Principal,
    ) -> (Vec<String>, Vec<String>, Vec<String>) {
        if let Some(checks) = &self.checks {
            let Ok(scope) = Self::verified_pr_scope(principal, loc) else {
                return (Vec::new(), Vec::new(), Vec::new());
            };
            let repo_ref = format!("myelin://{}/git/repo/{}", loc.tenant, loc.repo);
            let Ok(rows) = checks.rows_for_commit(&scope, &repo_ref, &GitOid(head_oid.to_string()))
            else {
                return (Vec::new(), Vec::new(), Vec::new());
            };
            return check_facts_from_rows(&rows);
        }

        let mut green = Vec::new();
        let mut fork_unendorsed = Vec::new();
        let mut endorsed = Vec::new();
        if let Ok(prs) = self.pr_list(loc, principal) {
            for rec in prs.into_iter().filter(|record| record.head_oid == head_oid) {
                green.extend(rec.green_contexts);
                fork_unendorsed.extend(rec.fork_unendorsed_contexts);
                endorsed.extend(rec.endorsed_contexts);
            }
        }
        (green, fork_unendorsed, endorsed)
    }

    /// Overlay the Git-owned per-commit projection onto the PR facts consumed by the policy
    /// evaluator. Stored check arrays are compatibility-only when the production projection exists.
    pub(super) fn record_with_projected_checks(
        &self,
        loc: &RepoLoc,
        record: &PrRecord,
        principal: &Principal,
    ) -> Result<PrRecord, DurableError> {
        let Some(checks) = &self.checks else {
            return Ok(record.clone());
        };
        let scope = Self::verified_pr_scope(principal, loc)?;
        let repo_ref = format!("myelin://{}/git/repo/{}", loc.tenant, loc.repo);
        let rows = checks
            .rows_for_commit(&scope, &repo_ref, &GitOid(record.head_oid.clone()))
            .map_err(|_| DurableError::Io("Git check projection is unavailable".into()))?;
        let mut projected = record.clone();
        overlay_projected_check_rows(&mut projected, &rows);
        Ok(projected)
    }

    pub(super) fn projected_check_rows_for_records(
        &self,
        loc: &RepoLoc,
        records: &[PrRecord],
        principal: &Principal,
    ) -> Result<Option<BTreeMap<String, Vec<CheckStatusRow>>>, DurableError> {
        let Some(checks) = &self.checks else {
            return Ok(None);
        };
        let scope = Self::verified_pr_scope(principal, loc)?;
        let repo_ref = format!("myelin://{}/git/repo/{}", loc.tenant, loc.repo);
        let mut heads = records
            .iter()
            .map(|record| GitOid(record.head_oid.clone()))
            .collect::<Vec<_>>();
        heads.sort_by(|left, right| left.0.cmp(&right.0));
        heads.dedup_by(|left, right| left.0 == right.0);
        checks
            .rows_for_commits(&scope, &repo_ref, &heads)
            .map(Some)
            .map_err(|_| DurableError::Io("Git check projection is unavailable".into()))
    }

    /// Authoritative checks and merge-gate view used by both the checks endpoint and a merge-conflict
    /// rerender. The UI consumes the decision; it does not recompute it.
    pub(super) fn pr_checks_json(
        &self,
        loc: &RepoLoc,
        record: &PrRecord,
        principal: &Principal,
    ) -> Result<Value, DurableError> {
        let record = self.record_with_projected_checks(loc, record, principal)?;
        let ruleset = self.prs.effective_ruleset_for(loc, &record.base_ref)?;
        let required_contexts = canonical_required_context_tokens(&ruleset.required_contexts)?;
        let evaluation = evaluate_merge(&ruleset, &record)
            .map_err(|error| DurableError::Git(error.to_string()))?;
        let has_blocking_review = record.reviews.iter().any(|review| {
            matches!(
                review.state,
                ReviewState::Submitted(ReviewVerdict::RequestChanges)
            )
        });
        let counting_approvals = record
            .reviews
            .iter()
            .filter(|review| {
                matches!(review.state, ReviewState::Submitted(ReviewVerdict::Approve))
                    && review.reviewer_pseudonym != record.author_pseudonym
            })
            .count() as u32;
        Ok(json!({
            "required_contexts": required_contexts,
            "required_approvals": ruleset.required_approvals,
            "green_contexts": record.green_contexts,
            "endorsed_contexts": record.endorsed_contexts,
            "fork_unendorsed_contexts": record.fork_unendorsed_contexts,
            "gate_admitted": evaluation.admitted(),
            "changes_requested": has_blocking_review,
            "current_approvals": counting_approvals,
            "durable": true,
        }))
    }
}

fn canonical_required_context_tokens(raw: &[String]) -> Result<Vec<String>, DurableError> {
    MergeGatePolicy::from_required_contexts(raw)
        .map(|policy| {
            policy
                .required
                .iter()
                .map(CheckContext::policy_token)
                .collect()
        })
        .map_err(|error| DurableError::Git(error.to_string()))
}

pub(super) fn check_facts_from_rows(
    rows: &[CheckStatusRow],
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut green = Vec::new();
    let mut fork_unendorsed = Vec::new();
    for row in rows {
        if row.state != CheckState::Success || !row.cost_settled {
            continue;
        }
        let context = check_context_token(&row.context);
        match row.trust_tier {
            TrustTier::Trusted => green.push(context),
            TrustTier::UntrustedFork => fork_unendorsed.push(context),
        }
    }
    (green, fork_unendorsed, Vec::new())
}

fn check_context_token(context: &CheckContext) -> String {
    context.policy_token()
}

pub(super) fn overlay_projected_check_rows(record: &mut PrRecord, rows: &[CheckStatusRow]) {
    record.green_contexts.clear();
    record.fork_unendorsed_contexts.clear();
    for row in rows {
        if row.state != CheckState::Success || !row.cost_settled {
            continue;
        }
        let context = check_context_token(&row.context);
        match row.trust_tier {
            TrustTier::Trusted => record.green_contexts.push(context),
            TrustTier::UntrustedFork => record.fork_unendorsed_contexts.push(context),
        }
    }
    record.green_contexts.sort();
    record.green_contexts.dedup();
    record.fork_unendorsed_contexts.sort();
    record.fork_unendorsed_contexts.dedup();
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_git::check_status::{HumanisedRef, Timestamp};
    use myelin_tenancy::{ArtifactRef, TenantId};

    fn row(context: CheckContext, trust_tier: TrustTier) -> CheckStatusRow {
        let fact = myelin_git::check_status::CheckStatus {
            tenant: TenantId("acme".into()),
            repo: ArtifactRef("myelin://acme/git/repo/core".into()),
            commit_oid: GitOid("a".repeat(40)),
            context,
            state: CheckState::Success,
            required: true,
            run: ArtifactRef("myelin://acme/ci/run/1".into()),
            run_attempt: 1,
            trust_tier,
            details_ref: ArtifactRef("myelin://acme/ci/run/1#step-build".into()),
            summary: HumanisedRef {
                template_key: "ci.check.success".into(),
                args: BTreeMap::new(),
            },
            started_at: Timestamp("2026-07-24T00:00:00Z".into()),
            completed_at: Some(Timestamp("2026-07-24T00:00:01Z".into())),
            cost_settled: true,
        };
        CheckStatusRow::from_fact(&fact)
    }

    #[test]
    fn projected_check_facts_keep_the_provider_prefix_used_by_policy() {
        let rows = [
            row(CheckContext::ci("build"), TrustTier::Trusted),
            row(CheckContext::external("scan"), TrustTier::UntrustedFork),
        ];
        let (green, fork_unendorsed, endorsed) = check_facts_from_rows(&rows);
        assert_eq!(green, ["ci/build"]);
        assert_eq!(fork_unendorsed, ["external/scan"]);
        assert!(endorsed.is_empty());
    }

    #[test]
    fn checks_api_canonicalizes_legacy_bare_policy_contexts() {
        assert_eq!(
            canonical_required_context_tokens(&[
                "build".into(),
                "ci/test/unit".into(),
                "external/scan".into(),
            ])
            .unwrap(),
            ["ci/build", "ci/test/unit", "external/scan"],
        );
    }
}
