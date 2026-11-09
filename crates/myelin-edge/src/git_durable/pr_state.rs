//! Closing, reopening, marking a draft ready for review, and editing its title and description.
//! Each is its author's call, or that of a person who can push to the repository; an agent changes
//! only the pull requests it opened.

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

/// Refuses a description whose `Closes` lines exceed what a merge will act on.
pub(super) fn closes_targets_of(tenant: &str, body: &str) -> Result<(), DurableError> {
    myelin_git::typed_edges::closes_issue_targets(tenant, body)
        .map(|_| ())
        .map_err(|error| {
            DurableError::InvalidInput(format!(
                "the description names too many issues to close ({error})"
            ))
        })
}

/// The text a person or agent asks to change; at least one is present.
pub struct PrEdit<'a> {
    pub title: Option<&'a str>,
    pub body_md: Option<&'a str>,
}

impl DurableGitBackend {
    /// Whether `principal` may change `record`: its author, or a person who can push. Returns
    /// whether they can push, which also lets them undo someone else's close.
    fn authorize_pr_change(
        &self,
        loc: &RepoLoc,
        record: &PrRecord,
        principal: &Principal,
        reopening_someone_elses_close: bool,
        what: &str,
    ) -> Result<bool, DurableError> {
        let me = Self::pseudonym(&loc.tenant, principal);
        // Pushing to the repository is a person's authority over its pull requests; agents never
        // borrow it.
        let can_push = !Self::is_agent(principal)
            && self
                .repo_authz
                .authorize_repo_permission(principal, loc, RepoPermission::Push);
        if (record.author_pseudonym == me && !reopening_someone_elses_close) || can_push {
            return Ok(can_push);
        }
        let slug = &loc.repo;
        Err(DurableError::Forbidden(
            match (Self::is_agent(principal), reopening_someone_elses_close) {
                (true, false) => "an agent changes only the pull requests it opened".into(),
                (true, true) => {
                    "an agent cannot reopen a pull request that someone else closed".into()
                }
                (false, false) => {
                    format!("only its author or someone who can push to {slug} can {what}")
                }
                (false, true) => format!(
                    "only someone who can push to {slug} can reopen a pull request that someone \
                     else closed"
                ),
            },
        ))
    }

    /// Retitles a pull request or rewrites its description, sealed like the original text.
    pub fn edit_pr_with_operation(
        &self,
        target: PrActorContext<'_>,
        edit: PrEdit<'_>,
        operation_id: &PrOperationId,
    ) -> Result<PrRecord, DurableError> {
        let PrActorContext {
            repo: actor,
            number,
        } = target;
        let loc = Self::loc(actor.tenant, actor.region, actor.slug);
        let title = edit.title.map(str::trim);
        if title.is_some_and(|title| title.is_empty() || title.len() > MAX_PR_TITLE_BYTES) {
            return Err(DurableError::InvalidInput(format!(
                "a pull request title must be 1-{MAX_PR_TITLE_BYTES} bytes"
            )));
        }
        if edit
            .body_md
            .is_some_and(|body| body.len() > myelin_git::typed_edges::MAX_CLOSES_MESSAGE_BYTES)
        {
            return Err(DurableError::InvalidInput(
                "a pull request description must be at most 64 KiB".into(),
            ));
        }
        if title.is_none() && edit.body_md.is_none() {
            return Err(DurableError::InvalidInput(
                "an edit changes the title, the description, or both".into(),
            ));
        }
        if let Some(body) = edit.body_md {
            closes_targets_of(&loc.tenant, body)?;
        }
        let record = self
            .pr_get(&loc, number, actor.principal)?
            .ok_or_else(|| DurableError::NotFound("pull request not found".into()))?;
        self.authorize_pr_change(
            &loc,
            &record,
            actor.principal,
            false,
            "edit this pull request",
        )?;
        let same_title = title.is_none_or(|title| title == record.title);
        let same_body = edit
            .body_md
            .is_none_or(|body| (!body.is_empty()).then_some(body) == record.body_md.as_deref());
        if same_title && same_body {
            // Nothing to record: no event, and a scheduled merge keeps its place.
            return Ok(record);
        }
        self.pr_mutate(
            &loc,
            number,
            PrMutation::Edit {
                title: title.map(str::to_string),
                body_md: edit.body_md.map(str::to_string),
                by: actor.principal.principal_id.0.clone(),
            },
            operation_id,
            actor.principal,
        )
    }

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
        let can_push = self.authorize_pr_change(
            &loc,
            &record,
            principal,
            closed_by_someone_else,
            what(change),
        )?;
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
            // The store checks the closer again under its row lock.
            PrStateChange::Reopen => PrMutation::Reopen {
                by: me,
                overriding: can_push,
            },
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
