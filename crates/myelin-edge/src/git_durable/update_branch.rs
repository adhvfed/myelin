//! Update branch: Myelin merges a pull request's base into its branch, so a branch that other
//! merges left behind becomes mergeable again without a local checkout.

use super::*;
use myelin_git::durable::{PreparedMerge, ReflogCommitMessage};

/// What one server-side merge may write: its commit, rewritten trees, and merged blobs.
const UPDATE_BRANCH_MAX_OBJECTS: usize = 10_000;
const UPDATE_BRANCH_MAX_BYTES: usize = 64 * 1024 * 1024;
const ADOPTION_OPERATION_DOMAIN: &str = "myelin.git.update-branch-adoption.v1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BranchUpdate {
    /// The branch already contains its base.
    UpToDate { head_oid: String },
    /// A merge of the base landed on the branch, and the pull request follows it.
    Updated { head_oid: String, replayed: bool },
    /// Both sides changed these paths incompatibly; nothing was written.
    Conflicted { paths: Vec<String> },
}

struct UpdateTarget<'a> {
    loc: RepoLoc,
    principal: &'a Principal,
    number: u64,
    head_ref: String,
    operation_id: &'a PrOperationId,
    request_trailer: String,
}

impl DurableGitBackend {
    pub fn update_pr_branch_with_operation(
        &self,
        target: PrActorContext<'_>,
        expected_head_oid: Option<&str>,
        operation_id: &PrOperationId,
    ) -> Result<BranchUpdate, DurableError> {
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
        if record.head_repo_slug != slug {
            return Err(DurableError::Conflict(
                "a pull request from a fork is updated from its fork".into(),
            ));
        }
        let head_ref = branch_ref(&record.head_ref);
        let base_ref = branch_ref(&record.base_ref);
        let operation_trailer = format!("Myelin-Operation: {}", operation_id.digest());
        let target = UpdateTarget {
            request_trailer: format!(
                "Myelin-Request: {}",
                update_request_hash(actor, number, expected_head_oid)
            ),
            loc,
            principal,
            number,
            head_ref,
            operation_id,
        };
        let repo = Arc::new(self.store.open_repo(&target.loc)?);
        if let Some(previous) =
            repo.find_reflog_commit_by_trailer(&target.head_ref, &operation_trailer)?
        {
            return self.replayed_update(&target, previous);
        }
        if !matches!(record.state, PrState::Open | PrState::Draft) {
            return Err(DurableError::Conflict(
                "only an open pull request's branch can be updated".into(),
            ));
        }

        let head_tip = repo.read_ref(&target.head_ref)?.ok_or_else(|| {
            DurableError::NotFound("the pull request's branch no longer exists".into())
        })?;
        if expected_head_oid.is_some_and(|expected| expected != head_tip.as_str()) {
            return Err(DurableError::Conflict(
                "the pull request's branch moved; reload it and try again".into(),
            ));
        }
        let base_tip = repo.read_ref(&base_ref)?.ok_or_else(|| {
            DurableError::NotFound("the pull request's base branch no longer exists".into())
        })?;
        let psn = Self::pseudonym(tenant, principal);
        let message = format!(
            "Merge branch '{}' into {}\n\n{operation_trailer}\n{}",
            short_branch(&base_ref),
            short_branch(&target.head_ref),
            target.request_trailer,
        );
        let quarantine = FileEditQuarantine::new(&repo)?;
        let merge = match quarantine
            .repo
            .prepare_merge_commit(&head_tip, &base_tip, &message, &psn, &psn)?
        {
            PreparedMerge::UpToDate => {
                return Ok(BranchUpdate::UpToDate {
                    head_oid: head_tip.0,
                })
            }
            PreparedMerge::Conflicted { paths } => return Ok(BranchUpdate::Conflicted { paths }),
            PreparedMerge::Merged { commit } => commit,
        };
        let objects = merge_objects(&quarantine.repo)?;

        let actor_is_agent = Self::is_agent(principal);
        let ref_name = RefName::new(target.head_ref.clone());
        if let Some(reason) = self.protected_ref_refusal(
            &repo,
            &target.loc,
            principal,
            actor_is_agent,
            &ref_name,
            &merge,
        )? {
            return Err(DurableError::Conflict(format!(
                "branch protection refused updating `{}`: {reason}",
                ref_name.0
            )));
        }
        let ref_store =
            self.open_durable_refstore(repo.clone(), slug, tenant, region, principal)?;
        let push = PushSession {
            updates: vec![ProposedRefUpdate {
                ref_name,
                expected_old: PushOid::new(head_tip.0.clone()),
                new_oid: PushOid::new(merge.0.clone()),
                forced: false,
                commit_oids: vec![PushOid::new(merge.0.clone())],
            }],
            quarantine: objects
                .iter()
                .map(|(oid, _, bytes)| QuarantineObject {
                    oid: PushOid::new(oid.clone()),
                    bytes: bytes.clone(),
                })
                .collect(),
            pusher: Pusher::direct(psn, actor_is_agent),
        };
        let promotion = ObjectPromotion {
            repo: &repo,
            objects: &objects,
        };
        match ref_store
            .receive(&push, &promotion, CrashPoint::None)
            .map_err(|e| DurableError::Git(format!("ref-CAS: {e:?}")))?
        {
            PushOutcome::Accepted { .. } => {
                self.adopt_base_merge(&target, &head_tip.0, &merge.0);
                Ok(BranchUpdate::Updated {
                    head_oid: merge.0,
                    replayed: false,
                })
            }
            PushOutcome::Rejected(RejectReason::NonFastForward { .. }) => {
                // A concurrent retry of this same request may have landed first.
                if let Some(previous) =
                    repo.find_reflog_commit_by_trailer(&target.head_ref, &operation_trailer)?
                {
                    return self.replayed_update(&target, previous);
                }
                Err(DurableError::Conflict(
                    "the pull request's branch moved while it was being updated; try again".into(),
                ))
            }
            PushOutcome::Rejected(reason) => Err(DurableError::Conflict(format!(
                "the branch update was refused: {reason:?}"
            ))),
            PushOutcome::Crashed(_) => {
                Err(DurableError::Git("branch update ref-CAS crashed".into()))
            }
        }
    }

    /// Whether the base moved past an open same-repository pull request, so it must be updated
    /// before it can merge. Unknown (`None`) for forks and when either side cannot be read.
    pub(super) fn pr_behind_base(&self, loc: &RepoLoc, record: &PrRecord) -> Option<bool> {
        if !matches!(record.state, PrState::Open | PrState::Draft)
            || record.head_repo_slug != loc.repo
        {
            return None;
        }
        let repo = self.store.open_repo(loc).ok()?;
        let base = repo.read_ref(&branch_ref(&record.base_ref)).ok()??;
        repo.is_fast_forward(Some(&base), &CoreOid::new(record.head_oid.clone()))
            .ok()
            .map(|contains_base| !contains_base)
    }

    /// Answers a retry from the commit its first attempt landed. Anyone who can push can put
    /// these trailers in a commit message, so a replay never keeps reviews: when a crash left the
    /// pull request behind, it follows the branch as after any push.
    fn replayed_update(
        &self,
        target: &UpdateTarget<'_>,
        previous: ReflogCommitMessage,
    ) -> Result<BranchUpdate, DurableError> {
        if !previous
            .message
            .lines()
            .any(|line| line == target.request_trailer)
        {
            return Err(DurableError::Conflict(
                "idempotency key is already bound to a different Git operation".into(),
            ));
        }
        self.follow_branch(&target.loc, target.principal, &target.head_ref);
        Ok(BranchUpdate::Updated {
            head_oid: previous.oid.0,
            replayed: true,
        })
    }

    /// Moves the pull request onto the merge made from its reviewed head, keeping its reviews.
    /// When a push moved the pull request first, ordinary branch following settles it instead,
    /// and that clears approvals as any push does.
    fn adopt_base_merge(&self, target: &UpdateTarget<'_>, reviewed_head: &str, merge: &str) {
        let adopted = PrOperationId::derive(
            ADOPTION_OPERATION_DOMAIN,
            &[target.operation_id.digest().as_bytes()],
        )
        .and_then(|adoption| {
            self.pr_mutate(
                &target.loc,
                target.number,
                PrMutation::AdoptBaseMerge {
                    expected_head_oid: reviewed_head.to_owned(),
                    head_oid: merge.to_owned(),
                },
                &adoption,
                target.principal,
            )
        });
        match adopted {
            Ok(_) | Err(DurableError::Conflict(_)) => {}
            Err(error) => eprintln!(
                "edge: pull request {}#{} could not adopt its updated branch: {error}",
                target.loc.repo, target.number
            ),
        }
        self.follow_branch(&target.loc, target.principal, &target.head_ref);
    }
}

/// Reads every object the merge wrote into its quarantine, within the size of a modest push.
fn merge_objects(
    quarantine: &DurableGitRepo,
) -> Result<Vec<(String, String, Vec<u8>)>, DurableError> {
    let too_large = || {
        DurableError::Conflict(
            "this merge is too large to make on the server; merge the base locally and push".into(),
        )
    };
    let written = quarantine.own_loose_objects()?;
    if written.len() > UPDATE_BRANCH_MAX_OBJECTS {
        return Err(too_large());
    }
    let mut remaining = UPDATE_BRANCH_MAX_BYTES;
    let mut objects = Vec::with_capacity(written.len());
    for oid in written {
        let (kind, size) = quarantine.object_header(&oid)?;
        remaining = remaining.checked_sub(size).ok_or_else(too_large)?;
        let bytes = quarantine.read_object_bounded(&oid, size)?;
        objects.push((oid.0, kind.to_owned(), bytes));
    }
    Ok(objects)
}

/// Names the first few conflicting paths for a person or an agent; the full bounded list stays
/// in the structured response.
pub(crate) fn describe_conflicts(paths: &[String]) -> String {
    const NAMED: usize = 5;
    const PATH_CHARS: usize = 200;
    let mut named = paths
        .iter()
        .take(NAMED)
        .map(|path| match path.char_indices().nth(PATH_CHARS) {
            Some((end, _)) => format!("{}…", &path[..end]),
            None => path.clone(),
        })
        .collect::<Vec<_>>()
        .join(", ");
    if paths.len() > NAMED {
        named.push_str(&format!(" and {} more", paths.len() - NAMED));
    }
    format!(
        "the base and this branch both changed {named}; merge the base locally, resolve the \
         conflicts, and push"
    )
}

fn update_request_hash(
    actor: RepoActorContext<'_>,
    number: u64,
    expected_head_oid: Option<&str>,
) -> String {
    let number = number.to_string();
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"myelin.git.update-branch-request.v1\0");
    for part in [
        actor.tenant,
        actor.principal.principal_id.0.as_str(),
        actor.slug,
        number.as_str(),
        expected_head_oid.unwrap_or(""),
    ] {
        hasher.update(&(part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

fn short_branch(full: &str) -> &str {
    full.strip_prefix("refs/heads/").unwrap_or(full)
}

#[cfg(test)]
#[path = "update_branch/tests.rs"]
mod tests;
