//! Closing, reopening, and marking a draft ready for review. Each is its author's call, or that of
//! a person who can push to the repository; an agent changes only the pull requests it opened.

use super::mutation_support::branch_ref;
use super::*;
pub use myelin_git::pr_store::PrStateChange;

pub(super) const DRAFT_MERGE_REFUSAL: &str =
    "a draft pull request cannot be merged; mark it ready for review first";

fn what(change: PrStateChange) -> &'static str {
    match change {
        PrStateChange::Close => "close this pull request",
        PrStateChange::Reopen => "reopen this pull request",
        PrStateChange::MarkReady => "mark this pull request ready for review",
    }
}

impl DurableGitBackend {
    pub fn change_pr_state_with_operation(
        &self,
        target: PrActorContext<'_>,
        change: PrStateChange,
        operation_id: &PrOperationId,
    ) -> Result<PrRecord, DurableError> {
        let PrActorContext {
            repo: actor,
            number,
        } = target;
        let RepoActorContext {
            tenant,
            region,
            slug,
            principal,
        } = actor;
        let loc = Self::loc(tenant, region, slug);
        let record = self
            .pr_get(&loc, number, principal)?
            .ok_or_else(|| DurableError::NotFound("pull request not found".into()))?;
        if record.author_pseudonym != Self::pseudonym(tenant, principal) {
            if Self::is_agent(principal) {
                return Err(DurableError::Forbidden(
                    "an agent changes only the pull requests it opened".into(),
                ));
            }
            if !self
                .repo_authz
                .authorize_repo_permission(principal, &loc, RepoPermission::Push)
            {
                return Err(DurableError::Forbidden(format!(
                    "only its author or someone who can push to {slug} can {}",
                    what(change)
                )));
            }
        }
        let head_ref = branch_ref(&record.head_ref);
        let same_repository = record.head_repo_slug == slug;
        if change == PrStateChange::Reopen && record.state == PrState::Closed {
            let repo = self.store.open_repo(&loc)?;
            if repo.read_ref(&branch_ref(&record.base_ref))?.is_none() {
                return Err(DurableError::Conflict(
                    "its base branch no longer exists".into(),
                ));
            }
            if same_repository && repo.read_ref(&head_ref)?.is_none() {
                return Err(DurableError::Conflict(format!(
                    "its branch {} no longer exists; push it again to reopen this pull request",
                    head_ref.trim_start_matches("refs/heads/")
                )));
            }
        }
        let changed = self.pr_mutate(&loc, number, change.into(), operation_id, principal)?;
        if change == PrStateChange::Reopen && same_repository {
            // The branch may have moved while the pull request was closed; reviews of the old
            // head do not carry over to new commits.
            self.follow_branch(&loc, principal, &head_ref);
            return Ok(self.pr_get(&loc, number, principal)?.unwrap_or(changed));
        }
        Ok(changed)
    }
}

#[cfg(test)]
mod tests;
