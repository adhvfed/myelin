//! Closing, reopening, and marking a draft ready for review. Each is its author's call, or that of
//! a person who can push to the repository; an agent changes only the pull requests it opened.

use super::mutation_support::branch_ref;
use super::*;
pub use myelin_git::pr_store::PrStateChange;

pub(super) const DRAFT_MERGE_REFUSAL: &str =
    "a draft pull request cannot be merged; mark it ready for review first";
pub(super) const CLOSED_MERGE_REFUSAL: &str =
    "a closed pull request cannot be merged; reopen it first";

/// Why a pull request in this state cannot merge, said before any merge work starts. The store
/// checks again under its row lock.
pub(super) fn merge_refusal(state: PrState) -> Option<&'static str> {
    match state {
        PrState::Draft => Some(DRAFT_MERGE_REFUSAL),
        PrState::Closed => Some(CLOSED_MERGE_REFUSAL),
        PrState::Open | PrState::Merged => None,
    }
}

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
        let me = Self::pseudonym(tenant, principal);
        // An author may undo their own close, never a maintainer's.
        let closed_by_someone_else = change == PrStateChange::Reopen
            && record.state == PrState::Closed
            && record
                .closed_by
                .as_deref()
                .is_some_and(|closer| closer != me);
        if record.author_pseudonym != me || closed_by_someone_else {
            let refusal = match (Self::is_agent(principal), closed_by_someone_else) {
                (true, false) => "an agent changes only the pull requests it opened".into(),
                (true, true) => {
                    "an agent cannot reopen a pull request that someone else closed".into()
                }
                (false, false) => format!(
                    "only its author or someone who can push to {slug} can {}",
                    what(change)
                ),
                (false, true) => format!(
                    "only someone who can push to {slug} can reopen a pull request that someone \
                     else closed"
                ),
            };
            if Self::is_agent(principal)
                || !self
                    .repo_authz
                    .authorize_repo_permission(principal, &loc, RepoPermission::Push)
            {
                return Err(DurableError::Forbidden(refusal));
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
        let mutation = match change {
            PrStateChange::Close => PrMutation::Close { by: me },
            PrStateChange::Reopen => PrMutation::Reopen,
            PrStateChange::MarkReady => PrMutation::MarkReady,
        };
        let changed = self.pr_mutate(&loc, number, mutation, operation_id, principal)?;
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
