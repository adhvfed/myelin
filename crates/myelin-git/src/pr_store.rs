use std::io::Read as _;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::check_status::{
    CheckContext, CheckState, CheckStatus, CheckStatusProjection, GitOid, HumanisedRef, Timestamp,
    TrustTier,
};
use crate::core::{Oid as CoreOid, RepoLoc};
use crate::durable::{DurableError, DurableGitRepo};
use crate::gix_backend::{RepoPathResolver, RootedResolver};
use crate::lifecycle::{
    evaluate_ruleset, BranchProtectionRuleset, MergeContext, PrState, PrTransition, PullRequest,
    ReviewState, ReviewVerdict, RulesetOutcome,
};
use crate::merge_gate::{evaluate_merge_gate, MergeGateOutcome, MergeGatePolicy};
use crate::receive_pack::{
    CrashPoint, InMemoryObjectDb, Oid as PushOid, ProposedRefUpdate, PushOutcome, PushProvenance,
    PushSession, Pusher, RefName, RefStore,
};

pub(crate) const PR_RECORD_MAX_BYTES: usize = 2 * 1024 * 1024;
const BRANCH_PROTECTION_MAX_BYTES: usize = 256 * 1024;

pub(crate) fn ensure_pr_record_size(size: usize) -> Result<(), DurableError> {
    if size > PR_RECORD_MAX_BYTES {
        return Err(DurableError::Git(
            "pull request record limit exceeded: serialized bytes".into(),
        ));
    }
    Ok(())
}

pub(crate) fn accepted_merge_update_seq(
    moved: &[(RefName, PushOid, u64)],
    expected_ref: &RefName,
    expected_oid: &PushOid,
) -> Result<u64, DurableError> {
    match moved {
        [(ref_name, oid, update_seq)]
            if ref_name == expected_ref && oid == expected_oid && *update_seq > 0 =>
        {
            Ok(*update_seq)
        }
        _ => Err(DurableError::Git(
            "merge ref adapter returned an invalid committed-move witness".into(),
        )),
    }
}

fn ensure_branch_protection_size(size: usize) -> Result<(), DurableError> {
    if size > BRANCH_PROTECTION_MAX_BYTES {
        return Err(DurableError::Git(
            "branch protection limit exceeded: serialized bytes".into(),
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchProtectionConfig {
    pub rulesets: Vec<BranchProtectionRuleset>,
}

impl BranchProtectionConfig {
    pub fn resolve(&self, base_ref: &str) -> Option<&BranchProtectionRuleset> {
        self.rulesets.iter().find(|r| r.matches(base_ref))
    }
}

pub fn effective_ruleset(
    config: Option<&BranchProtectionConfig>,
    base_ref: &str,
    default_ref: &RefName,
) -> BranchProtectionRuleset {
    if let Some(rs) = config.and_then(|c| c.resolve(base_ref)) {
        return rs.clone();
    }
    if RefName::new(base_ref).has_baseline_protection(default_ref) {
        return BranchProtectionRuleset {
            ref_pattern: base_ref.to_string(),
            required_contexts: Vec::new(),
            required_approvals: 1,
            require_codeowner_review: false,
            require_conversation_resolution: false,
            allow_force_push: false,
        };
    }
    BranchProtectionRuleset {
        ref_pattern: base_ref.to_string(),
        required_contexts: Vec::new(),
        required_approvals: 0,
        require_codeowner_review: false,
        require_conversation_resolution: false,
        allow_force_push: false,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewRecord {
    pub reviewer_pseudonym: String,
    pub state: ReviewState,
    pub is_agent: bool,
}

impl ReviewRecord {
    fn is_current_approval(&self) -> bool {
        matches!(self.state, ReviewState::Submitted(ReviewVerdict::Approve))
    }
    fn is_blocking(&self) -> bool {
        matches!(
            self.state,
            ReviewState::Submitted(ReviewVerdict::RequestChanges)
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrRecord {
    pub number: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body_md: Option<String>,
    #[serde(default)]
    pub author_is_agent: bool,
    #[serde(default)]
    pub updated_at: Option<i64>,
    #[serde(default)]
    pub created_at: Option<i64>,
    pub state: PrState,
    pub base_ref: String,
    pub head_ref: String,
    #[serde(default)]
    pub head_repo_slug: String,
    pub head_oid: String,
    pub author_pseudonym: String,
    #[serde(default)]
    pub author_subject_id: String,
    pub reviews: Vec<ReviewRecord>,
    pub green_contexts: Vec<String>,
    pub fork_unendorsed_contexts: Vec<String>,
    pub endorsed_contexts: Vec<String>,
    pub codeowner_review_satisfied: bool,
    pub outstanding_conversations: u32,
}

impl PrRecord {
    pub fn open(pr: &PullRequest, head_oid: impl Into<String>) -> PrRecord {
        PrRecord {
            number: pr.number,
            title: String::new(),
            body_md: None,
            author_is_agent: false,
            updated_at: None,
            created_at: None,
            state: pr.state,
            base_ref: pr.base_ref.clone(),
            head_ref: pr.head_ref.clone(),
            head_repo_slug: String::new(),
            head_oid: head_oid.into(),
            author_pseudonym: pr.author_pseudonym.clone(),
            author_subject_id: String::new(),
            reviews: Vec::new(),
            green_contexts: Vec::new(),
            fork_unendorsed_contexts: Vec::new(),
            endorsed_contexts: Vec::new(),
            codeowner_review_satisfied: false,
            outstanding_conversations: 0,
        }
    }

    fn as_pull_request(&self) -> PullRequest {
        let mut pr = PullRequest::open(
            self.number,
            self.base_ref.clone(),
            self.head_ref.clone(),
            self.author_pseudonym.clone(),
            matches!(self.state, PrState::Draft),
        );
        pr.state = self.state;
        pr
    }

    fn latest_reviews(&self) -> std::collections::BTreeMap<&str, &ReviewRecord> {
        let mut reviews = std::collections::BTreeMap::new();
        for review in &self.reviews {
            reviews.insert(review.reviewer_pseudonym.as_str(), review);
        }
        reviews
    }

    pub fn counting_approvals(&self) -> u32 {
        let mut approvers = std::collections::BTreeSet::new();
        for r in self.latest_reviews().into_values() {
            if r.is_current_approval()
                && !r.is_agent
                && r.reviewer_pseudonym != self.author_pseudonym
            {
                approvers.insert(r.reviewer_pseudonym.as_str());
            }
        }
        approvers.len() as u32
    }

    pub fn has_blocking_review(&self) -> bool {
        self.latest_reviews()
            .into_values()
            .any(ReviewRecord::is_blocking)
    }

    pub fn review_state_label(&self) -> &'static str {
        let current = self.latest_reviews();
        if current.values().any(|review| review.is_blocking()) {
            "changes"
        } else if self.counting_approvals() > 0 {
            "approved"
        } else if current
            .values()
            .any(|review| matches!(review.state, ReviewState::Requested))
        {
            "requested"
        } else {
            "none"
        }
    }

    pub fn is_review_requested_of(&self, viewer_pseudonym: &str) -> bool {
        self.reviews
            .iter()
            .rev()
            .find(|review| review.reviewer_pseudonym == viewer_pseudonym)
            .is_some_and(|review| matches!(review.state, ReviewState::Requested))
    }

    pub fn has_review_relationship_with(&self, viewer_pseudonym: &str) -> bool {
        self.reviews
            .iter()
            .any(|review| review.reviewer_pseudonym == viewer_pseudonym)
    }

    pub fn checks_summary(&self, ruleset: &BranchProtectionRuleset) -> ChecksSummary {
        let total = ruleset.required_contexts.len() as u32;
        let passing = ruleset
            .required_contexts
            .iter()
            .filter(|c| self.green_contexts.iter().any(|g| g == *c))
            .count() as u32;
        let verdict = if total == 0 {
            if self.green_contexts.is_empty() {
                ChecksVerdict::None
            } else {
                ChecksVerdict::Pass
            }
        } else if passing >= total {
            ChecksVerdict::Pass
        } else {
            ChecksVerdict::Running
        };
        ChecksSummary {
            verdict,
            passing,
            failing: 0,
            total,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChecksVerdict {
    Pass,
    Fail,
    Running,
    None,
    Unavailable,
}

impl ChecksVerdict {
    pub fn as_str(self) -> &'static str {
        match self {
            ChecksVerdict::Pass => "pass",
            ChecksVerdict::Fail => "fail",
            ChecksVerdict::Running => "running",
            ChecksVerdict::None => "none",
            ChecksVerdict::Unavailable => "unavailable",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChecksSummary {
    pub verdict: ChecksVerdict,
    pub passing: u32,
    pub failing: u32,
    pub total: u32,
}

impl ChecksSummary {
    pub fn unavailable() -> ChecksSummary {
        ChecksSummary {
            verdict: ChecksVerdict::Unavailable,
            passing: 0,
            failing: 0,
            total: 0,
        }
    }
}

pub const PR_LIST_PAGE_MAX: usize = 100;

pub const PR_LIST_OFFSET_MAX: usize = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrListState {
    Open,
    Merged,
    Closed,
    All,
}

impl PrListState {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "open" => Some(Self::Open),
            "merged" => Some(Self::Merged),
            "closed" => Some(Self::Closed),
            "all" => Some(Self::All),
            _ => None,
        }
    }

    fn matches(self, state: PrState) -> bool {
        match self {
            Self::Open => matches!(state, PrState::Open | PrState::Draft),
            Self::Merged => state == PrState::Merged,
            Self::Closed => state == PrState::Closed,
            Self::All => true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrListSort {
    Updated,
    Created,
}

impl PrListSort {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "updated" => Some(Self::Updated),
            "created" => Some(Self::Created),
            _ => None,
        }
    }
}

fn pr_record_before_key(
    record: &PrRecord,
    key: &crate::pr_list_pagination::PrListKey,
    sort: PrListSort,
) -> bool {
    if sort == PrListSort::Created {
        return record.number > key.number;
    }
    match (record.updated_at, key.updated_at) {
        (Some(record_time), Some(key_time)) => {
            record_time > key_time || (record_time == key_time && record.number > key.number)
        }
        (Some(_), None) => true,
        (None, Some(_)) => false,
        (None, None) => record.number > key.number,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrListQuery {
    pub state: PrListState,
    pub sort: PrListSort,
    pub page: crate::pr_list_pagination::PrListPage,
    pub limit: usize,
    pub viewer_pseudonym: String,
}

impl PrListQuery {
    pub fn new(
        state: PrListState,
        sort: PrListSort,
        offset: usize,
        limit: usize,
        viewer_pseudonym: impl Into<String>,
    ) -> Result<Self, DurableError> {
        let query = Self {
            state,
            sort,
            page: crate::pr_list_pagination::PrListPage::LegacyOffset(offset),
            limit,
            viewer_pseudonym: viewer_pseudonym.into(),
        };
        query.validate()?;
        Ok(query)
    }

    pub fn initial(
        state: PrListState,
        sort: PrListSort,
        limit: usize,
        viewer_pseudonym: impl Into<String>,
    ) -> Result<Self, DurableError> {
        Self::from_page(
            state,
            sort,
            crate::pr_list_pagination::PrListPage::Initial,
            limit,
            viewer_pseudonym,
        )
    }

    pub fn from_page(
        state: PrListState,
        sort: PrListSort,
        page: crate::pr_list_pagination::PrListPage,
        limit: usize,
        viewer_pseudonym: impl Into<String>,
    ) -> Result<Self, DurableError> {
        let query = Self {
            state,
            sort,
            page,
            limit,
            viewer_pseudonym: viewer_pseudonym.into(),
        };
        query.validate()?;
        Ok(query)
    }

    pub fn validate(&self) -> Result<(), DurableError> {
        if matches!(self.page, crate::pr_list_pagination::PrListPage::LegacyOffset(offset) if offset > PR_LIST_OFFSET_MAX)
        {
            return Err(DurableError::Git(format!(
                "pull request page offset must be at most {PR_LIST_OFFSET_MAX}"
            )));
        }
        if !(1..=PR_LIST_PAGE_MAX).contains(&self.limit) {
            return Err(DurableError::Git(
                "pull request page limit must be between 1 and 100".into(),
            ));
        }
        if let crate::pr_list_pagination::PrListPage::Keyset(cursor) = &self.page {
            if cursor.endpoint()
                != crate::pr_list_pagination::PrListCursorEndpoint::Repository(self.state)
                || cursor.sort() != self.sort
                || cursor.limit() != self.limit
            {
                return Err(DurableError::Git(
                    "pull request keyset does not match repository list query".into(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PrListCounts {
    pub open: usize,
    pub merged: usize,
    pub closed: usize,
    pub all: usize,
    pub yours: usize,
    pub needs_review: usize,
}

impl PrListCounts {
    fn from_records(records: &[PrRecord], viewer_pseudonym: &str) -> Self {
        let count = |predicate: &dyn Fn(&PrRecord) -> bool| {
            records.iter().filter(|record| predicate(record)).count()
        };
        Self {
            open: count(&|record| matches!(record.state, PrState::Open | PrState::Draft)),
            merged: count(&|record| record.state == PrState::Merged),
            closed: count(&|record| record.state == PrState::Closed),
            all: records.len(),
            yours: count(&|record| record.author_pseudonym == viewer_pseudonym),
            needs_review: count(&|record| record.is_review_requested_of(viewer_pseudonym)),
        }
    }

    pub fn filtered_total(self, state: PrListState) -> usize {
        match state {
            PrListState::Open => self.open,
            PrListState::Merged => self.merged,
            PrListState::Closed => self.closed,
            PrListState::All => self.all,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrListSlice {
    pub records: Vec<PrRecord>,
    pub counts: PrListCounts,
    pub total: usize,
    pub offset: usize,
    pub has_newer: bool,
    pub has_older: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrListBucket {
    Yours,
    NeedsReview,
}

impl PrListBucket {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "yours" => Some(Self::Yours),
            "needs-review" => Some(Self::NeedsReview),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrCrossListQuery {
    pub bucket: PrListBucket,
    pub sort: PrListSort,
    pub page: crate::pr_list_pagination::PrListPage,
    pub limit: usize,
    pub viewer_pseudonym: String,
}

impl PrCrossListQuery {
    pub fn new(
        bucket: PrListBucket,
        sort: PrListSort,
        offset: usize,
        limit: usize,
        viewer_pseudonym: impl Into<String>,
    ) -> Result<Self, DurableError> {
        let query = Self {
            bucket,
            sort,
            page: crate::pr_list_pagination::PrListPage::LegacyOffset(offset),
            limit,
            viewer_pseudonym: viewer_pseudonym.into(),
        };
        query.validate()?;
        Ok(query)
    }

    pub fn initial(
        bucket: PrListBucket,
        sort: PrListSort,
        limit: usize,
        viewer_pseudonym: impl Into<String>,
    ) -> Result<Self, DurableError> {
        Self::from_page(
            bucket,
            sort,
            crate::pr_list_pagination::PrListPage::Initial,
            limit,
            viewer_pseudonym,
        )
    }

    pub fn from_page(
        bucket: PrListBucket,
        sort: PrListSort,
        page: crate::pr_list_pagination::PrListPage,
        limit: usize,
        viewer_pseudonym: impl Into<String>,
    ) -> Result<Self, DurableError> {
        let query = Self {
            bucket,
            sort,
            page,
            limit,
            viewer_pseudonym: viewer_pseudonym.into(),
        };
        query.validate()?;
        Ok(query)
    }

    pub fn validate(&self) -> Result<(), DurableError> {
        if matches!(self.page, crate::pr_list_pagination::PrListPage::LegacyOffset(offset) if offset > PR_LIST_OFFSET_MAX)
        {
            return Err(DurableError::Git(format!(
                "pull request page offset must be at most {PR_LIST_OFFSET_MAX}"
            )));
        }
        if !(1..=PR_LIST_PAGE_MAX).contains(&self.limit) {
            return Err(DurableError::Git(
                "pull request page limit must be between 1 and 100".into(),
            ));
        }
        if let crate::pr_list_pagination::PrListPage::Keyset(cursor) = &self.page {
            if cursor.endpoint()
                != crate::pr_list_pagination::PrListCursorEndpoint::CrossRepository(self.bucket)
                || cursor.sort() != self.sort
                || cursor.limit() != self.limit
            {
                return Err(DurableError::Git(
                    "pull request keyset does not match cross-repository list query".into(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrCrossListRecord {
    pub repo_slug: String,
    pub record: PrRecord,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrCrossListSlice {
    pub records: Vec<PrCrossListRecord>,
    pub total: usize,
    pub offset: usize,
    pub has_newer: bool,
    pub has_older: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MergeEval {
    pub gate: MergeGateOutcome,
    pub ruleset: RulesetOutcome,
}

impl MergeEval {
    pub fn admitted(&self) -> bool {
        self.gate.is_admitted() && self.ruleset.is_satisfied()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GateInputError(pub String);

impl std::fmt::Display for GateInputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "merge-gate input error: {}", self.0)
    }
}
impl std::error::Error for GateInputError {}

fn synthetic_fact(
    head: &GitOid,
    ctx: CheckContext,
    state: CheckState,
    trust: TrustTier,
) -> CheckStatus {
    use myelin_tenancy::{ArtifactRef, TenantId};
    CheckStatus {
        tenant: TenantId("_gate".into()),
        repo: ArtifactRef("myelin://_gate/git/repo/_".into()),
        commit_oid: head.clone(),
        context: ctx,
        state,
        required: true,
        run: ArtifactRef("myelin://_gate/ci/run/_".into()),
        run_attempt: 1,
        trust_tier: trust,
        details_ref: ArtifactRef("myelin://_gate/ci/run/_#s".into()),
        summary: HumanisedRef {
            template_key: "ci.check.updated".into(),
            args: Default::default(),
        },
        started_at: Timestamp("2026-06-29T00:00:00Z".into()),
        completed_at: Some(Timestamp("2026-06-29T00:01:00Z".into())),
        cost_settled: true,
    }
}

pub fn evaluate_merge(
    ruleset: &BranchProtectionRuleset,
    rec: &PrRecord,
) -> Result<MergeEval, GateInputError> {
    let policy = MergeGatePolicy::from_required_contexts(&ruleset.required_contexts)
        .map_err(|e| GateInputError(e.to_string()))?;
    let head = GitOid(rec.head_oid.clone());

    let mut proj = CheckStatusProjection::new();
    let parse = |s: &str| {
        crate::merge_gate::parse_required_context(s).map_err(|e| GateInputError(e.to_string()))
    };
    for c in &rec.green_contexts {
        proj.apply(&synthetic_fact(
            &head,
            parse(c)?,
            CheckState::Success,
            TrustTier::Trusted,
        ));
    }
    for c in &rec.fork_unendorsed_contexts {
        proj.apply(&synthetic_fact(
            &head,
            parse(c)?,
            CheckState::Success,
            TrustTier::UntrustedFork,
        ));
    }
    let endorsed: Vec<CheckContext> = rec
        .endorsed_contexts
        .iter()
        .map(|c| parse(c))
        .collect::<Result<_, _>>()?;

    let gate = evaluate_merge_gate(&policy, &proj, &head, &endorsed);

    let ruleset_def = BranchProtectionRuleset {
        ref_pattern: ruleset.ref_pattern.clone(),
        required_contexts: Vec::new(),
        required_approvals: ruleset.required_approvals,
        require_codeowner_review: ruleset.require_codeowner_review,
        require_conversation_resolution: ruleset.require_conversation_resolution,
        allow_force_push: ruleset.allow_force_push,
    };
    let mctx = MergeContext {
        green_contexts: Vec::new(),
        current_approvals: rec.counting_approvals(),
        codeowner_review_satisfied: rec.codeowner_review_satisfied,
        has_blocking_review: rec.has_blocking_review(),
        outstanding_conversations: rec.outstanding_conversations,
    };
    let ruleset_outcome = evaluate_ruleset(&ruleset_def, &mctx);

    Ok(MergeEval {
        gate,
        ruleset: ruleset_outcome,
    })
}

pub struct DurablePrStore<P: RepoPathResolver = RootedResolver> {
    resolver: P,
    write_lock: Mutex<()>,
}

impl DurablePrStore<RootedResolver> {
    pub fn rooted(root: impl Into<PathBuf>) -> Self {
        Self {
            resolver: RootedResolver::new(root),
            write_lock: Mutex::new(()),
        }
    }
}

impl<P: RepoPathResolver> DurablePrStore<P> {
    pub fn new(resolver: P) -> Self {
        Self {
            resolver,
            write_lock: Mutex::new(()),
        }
    }

    fn meta_dir(&self, repo: &RepoLoc) -> Result<PathBuf, DurableError> {
        let repo_path = self
            .resolver
            .repo_path(repo)
            .map_err(|e| DurableError::Git(e.to_string()))?;
        Ok(repo_path.join("myelin"))
    }

    fn prs_dir(&self, repo: &RepoLoc) -> Result<PathBuf, DurableError> {
        Ok(self.meta_dir(repo)?.join("prs"))
    }

    fn pr_path(&self, repo: &RepoLoc, number: u64) -> Result<PathBuf, DurableError> {
        Ok(self.prs_dir(repo)?.join(format!("{number}.json")))
    }

    fn protection_path(&self, repo: &RepoLoc) -> Result<PathBuf, DurableError> {
        Ok(self.meta_dir(repo)?.join("branch-protection.json"))
    }

    fn write_atomic(
        &self,
        dir: &std::path::Path,
        file: &std::path::Path,
        bytes: &[u8],
    ) -> Result<(), DurableError> {
        crate::durable::write_file_atomic(dir, file, bytes)
    }

    fn read_bounded_file(
        path: &std::path::Path,
        maximum_bytes: usize,
        limit_error: &'static str,
    ) -> Result<Option<Vec<u8>>, DurableError> {
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(DurableError::Io(format!(
                    "open {}: {error}",
                    path.display()
                )))
            }
        };
        let mut bytes = Vec::new();
        file.take((maximum_bytes as u64).saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|error| DurableError::Io(format!("read {}: {error}", path.display())))?;
        if bytes.len() > maximum_bytes {
            return Err(DurableError::Git(limit_error.into()));
        }
        Ok(Some(bytes))
    }

    pub fn put_protection(
        &self,
        repo: &RepoLoc,
        config: &BranchProtectionConfig,
    ) -> Result<(), DurableError> {
        let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        let dir = self.meta_dir(repo)?;
        let bytes = serde_json::to_vec_pretty(config)
            .map_err(|e| DurableError::Io(format!("serialize branch-protection: {e}")))?;
        ensure_branch_protection_size(bytes.len())?;
        self.write_atomic(&dir, &self.protection_path(repo)?, &bytes)
    }

    pub fn get_protection(
        &self,
        repo: &RepoLoc,
    ) -> Result<Option<BranchProtectionConfig>, DurableError> {
        let path = self.protection_path(repo)?;
        match Self::read_bounded_file(
            &path,
            BRANCH_PROTECTION_MAX_BYTES,
            "branch protection limit exceeded: serialized bytes",
        )? {
            Some(bytes) => Ok(Some(serde_json::from_slice(&bytes).map_err(|e| {
                DurableError::Io(format!("parse {}: {e}", path.display()))
            })?)),
            None => Ok(None),
        }
    }

    pub fn effective_ruleset_for(
        &self,
        repo: &RepoLoc,
        base_ref: &str,
        default_ref: &RefName,
    ) -> Result<BranchProtectionRuleset, DurableError> {
        let config = self.get_protection(repo)?;
        Ok(effective_ruleset(config.as_ref(), base_ref, default_ref))
    }

    pub fn put(&self, repo: &RepoLoc, rec: &PrRecord) -> Result<(), DurableError> {
        let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        self.put_unlocked(repo, rec)
    }

    fn put_unlocked(&self, repo: &RepoLoc, rec: &PrRecord) -> Result<(), DurableError> {
        let dir = self.prs_dir(repo)?;
        let bytes = serde_json::to_vec_pretty(rec)
            .map_err(|e| DurableError::Io(format!("serialize PR {}: {e}", rec.number)))?;
        ensure_pr_record_size(bytes.len())?;
        self.write_atomic(&dir, &self.pr_path(repo, rec.number)?, &bytes)
    }

    pub fn get(&self, repo: &RepoLoc, number: u64) -> Result<Option<PrRecord>, DurableError> {
        let path = self.pr_path(repo, number)?;
        match Self::read_bounded_file(
            &path,
            PR_RECORD_MAX_BYTES,
            "pull request record limit exceeded: serialized bytes",
        )? {
            Some(bytes) => {
                let record: PrRecord = serde_json::from_slice(&bytes)
                    .map_err(|e| DurableError::Io(format!("parse {}: {e}", path.display())))?;
                if record.number != number {
                    return Err(DurableError::Git(format!(
                        "PR record identity mismatch: requested #{number} but {} stores #{}",
                        path.display(),
                        record.number
                    )));
                }
                Ok(Some(record))
            }
            None => Ok(None),
        }
    }

    pub fn open_pr(&self, repo: &RepoLoc, rec: &PrRecord) -> Result<(), DurableError> {
        let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        if self.get(repo, rec.number)?.is_some() {
            return Err(DurableError::Git(format!(
                "PR #{} already exists (conflict)",
                rec.number
            )));
        }
        self.put_unlocked(repo, rec)
    }

    pub fn update<R>(
        &self,
        repo: &RepoLoc,
        number: u64,
        mutate: impl FnOnce(&mut PrRecord) -> Result<R, DurableError>,
    ) -> Result<R, DurableError> {
        let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut record = self
            .get(repo, number)?
            .ok_or_else(|| DurableError::NotFound(format!("PR #{number}")))?;
        let output = mutate(&mut record)?;
        self.put_unlocked(repo, &record)?;
        Ok(output)
    }

    pub fn list_bounded(
        &self,
        repo: &RepoLoc,
        maximum_records: usize,
        maximum_bytes: usize,
    ) -> Result<Vec<PrRecord>, DurableError> {
        let dir = self.prs_dir(repo)?;
        let mut out = Vec::new();
        let mut total_bytes = 0usize;
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(DurableError::Io(format!("read_dir {}: {e}", dir.display()))),
        };
        for entry in rd {
            let entry = entry.map_err(|e| DurableError::Io(format!("dir entry: {e}")))?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                if out.len() >= maximum_records {
                    return Err(DurableError::Git(
                        "pull request list limit exceeded: record count".into(),
                    ));
                }
                let file_number = path
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .and_then(crate::coordinate::parse_positive_decimal)
                    .ok_or_else(|| {
                        DurableError::Io(format!("invalid PR record filename {}", path.display()))
                    })?;
                let mut bytes = Vec::new();
                std::fs::File::open(&path)
                    .and_then(|file| {
                        file.take((PR_RECORD_MAX_BYTES as u64).saturating_add(1))
                            .read_to_end(&mut bytes)
                    })
                    .map_err(|error| {
                        DurableError::Io(format!("read {}: {error}", path.display()))
                    })?;
                ensure_pr_record_size(bytes.len())?;
                total_bytes = total_bytes.checked_add(bytes.len()).ok_or_else(|| {
                    DurableError::Git("pull request list limit exceeded: serialized bytes".into())
                })?;
                if total_bytes > maximum_bytes {
                    return Err(DurableError::Git(
                        "pull request list limit exceeded: serialized bytes".into(),
                    ));
                }
                let rec = serde_json::from_slice::<PrRecord>(&bytes)
                    .map_err(|e| DurableError::Io(format!("parse {}: {e}", path.display())))?;
                if rec.number != file_number {
                    return Err(DurableError::Git(format!(
                        "PR record identity mismatch: {} names #{file_number} but stores #{}",
                        path.display(),
                        rec.number
                    )));
                }
                out.push(rec);
            }
        }
        out.sort_by_key(|r| r.number);
        Ok(out)
    }

    pub fn list_page_bounded(
        &self,
        repo: &RepoLoc,
        query: &PrListQuery,
        maximum_records: usize,
        maximum_bytes: usize,
    ) -> Result<PrListSlice, DurableError> {
        query.validate()?;
        let mut records = self.list_bounded(repo, maximum_records, maximum_bytes)?;
        let counts = PrListCounts::from_records(&records, &query.viewer_pseudonym);
        records.retain(|record| query.state.matches(record.state));
        match query.sort {
            PrListSort::Created => {
                records.sort_by_key(|record| std::cmp::Reverse(record.number));
            }
            PrListSort::Updated => records.sort_by(|a, b| {
                b.updated_at
                    .cmp(&a.updated_at)
                    .then(b.number.cmp(&a.number))
            }),
        }
        let total = records.len();
        let mut selected: Vec<(usize, PrRecord)> = match &query.page {
            crate::pr_list_pagination::PrListPage::Initial => {
                records.into_iter().enumerate().collect()
            }
            crate::pr_list_pagination::PrListPage::LegacyOffset(offset) => {
                records.into_iter().enumerate().skip(*offset).collect()
            }
            crate::pr_list_pagination::PrListPage::Keyset(cursor) => {
                let direction = cursor.direction();
                let key = cursor.key();
                let mut rows: Vec<_> = records
                    .into_iter()
                    .enumerate()
                    .filter(|(_, record)| {
                        let before = pr_record_before_key(record, key, query.sort);
                        let equal = record.number == key.number
                            && (query.sort == PrListSort::Created
                                || record.updated_at == key.updated_at);
                        match direction {
                            crate::pr_list_pagination::PrListDirection::Newer => before,
                            crate::pr_list_pagination::PrListDirection::Older => !before && !equal,
                        }
                    })
                    .collect();
                if direction == crate::pr_list_pagination::PrListDirection::Newer {
                    rows.reverse();
                }
                rows
            }
        };
        selected.truncate(query.limit);
        if matches!(
            query.page,
            crate::pr_list_pagination::PrListPage::Keyset(ref cursor)
                if cursor.direction() == crate::pr_list_pagination::PrListDirection::Newer
        ) {
            selected.reverse();
        }
        let first_position = selected.first().map(|(position, _)| *position);
        let last_position = selected.last().map(|(position, _)| *position);
        let has_newer = first_position.is_some_and(|position| position > 0);
        let has_older = last_position.is_some_and(|position| position.saturating_add(1) < total);
        let page = selected.into_iter().map(|(_, record)| record).collect();
        Ok(PrListSlice {
            records: page,
            counts,
            total,
            offset: query.page.display_offset(),
            has_newer,
            has_older,
        })
    }

    pub fn max_pr_number(&self, repo: &RepoLoc) -> Result<Option<u64>, DurableError> {
        let dir = self.prs_dir(repo)?;
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(DurableError::Io(format!("read_dir {}: {e}", dir.display()))),
        };
        let mut max: Option<u64> = None;
        for entry in rd {
            let entry = entry.map_err(|e| DurableError::Io(format!("dir entry: {e}")))?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let number = path
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(crate::coordinate::parse_positive_decimal)
                .ok_or_else(|| {
                    DurableError::Io(format!("invalid PR record filename {}", path.display()))
                })?;
            max = Some(max.map_or(number, |current| current.max(number)));
        }
        Ok(max)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MergeAttempt {
    Merged {
        base_ref: String,
        new_oid: String,
        update_seq: u64,
    },
    Blocked(MergeEval),
    InvalidHead(String),
    RefRefused(crate::receive_pack::RejectReason),
}

pub fn merge_pr<P: RepoPathResolver>(
    store: &DurablePrStore<P>,
    repo_loc: &RepoLoc,
    number: u64,
    ref_store: &RefStore,
    repo: &DurableGitRepo,
    merger_pseudonym: &str,
    provenance: PushProvenance,
) -> Result<MergeAttempt, DurableError> {
    let merged_at_unix = myelin_events::clock::system_clock_reading()
        .map_err(|error| DurableError::Io(format!("Git merge clock unavailable: {error}")))?
        .unix_seconds();
    let mut rec = store
        .get(repo_loc, number)?
        .ok_or_else(|| DurableError::NotFound(format!("PR #{number}")))?;

    let default_ref = RefName::new(repo.default_branch_ref()?);
    let ruleset = store.effective_ruleset_for(repo_loc, &rec.base_ref, &default_ref)?;

    let eval = evaluate_merge(&ruleset, &rec).map_err(|e| DurableError::Git(e.to_string()))?;
    if !eval.admitted() {
        return Ok(MergeAttempt::Blocked(eval));
    }

    let base = RefName::new(rec.base_ref.clone());
    let cur_tip: Option<CoreOid> = ref_store.try_tip(&base)?.map(|o| CoreOid::new(o.0));
    let head_core = CoreOid::new(rec.head_oid.clone());
    if !repo.object_is_commit(&head_core) {
        return Ok(MergeAttempt::InvalidHead(format!(
            "head_oid {} is not a commit in the repo",
            rec.head_oid
        )));
    }
    if !repo.is_fast_forward(cur_tip.as_ref(), &head_core)? {
        return Ok(MergeAttempt::InvalidHead(format!(
            "head_oid {} is not a fast-forward of {}",
            rec.head_oid, rec.base_ref
        )));
    }

    let expected_old = cur_tip
        .map(|o| PushOid::new(o.0))
        .unwrap_or_else(PushOid::zero);
    let head = PushOid::new(rec.head_oid.clone());
    let push = PushSession {
        updates: vec![ProposedRefUpdate {
            ref_name: base.clone(),
            expected_old,
            new_oid: head.clone(),
            forced: false,
            commit_oids: vec![head.clone()],
        }],
        quarantine: Vec::new(),
        pusher: Pusher::new(merger_pseudonym, provenance),
    };
    let outcome = ref_store
        .receive(&push, &InMemoryObjectDb::new(), CrashPoint::None)
        .map_err(|e| DurableError::Git(format!("merge ref advance failed: {e:?}")))?;

    match outcome {
        PushOutcome::Accepted { moved, .. } => {
            let update_seq = accepted_merge_update_seq(&moved, &base, &head)?;
            let mut pr = rec.as_pull_request();
            pr.transition(PrTransition::Merge, true)
                .map_err(|e| DurableError::Git(format!("PR merge transition: {e}")))?;
            rec.state = pr.state;
            rec.updated_at = Some(merged_at_unix);
            store.put(repo_loc, &rec)?;
            Ok(MergeAttempt::Merged {
                base_ref: rec.base_ref,
                new_oid: head.0,
                update_seq,
            })
        }
        PushOutcome::Rejected(reason) => Ok(MergeAttempt::RefRefused(reason)),
        PushOutcome::Crashed(_) => Err(DurableError::Git("merge ref advance crashed".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::durable::DurableGitStore;
    use myelin_events::{
        Actor, CausedBy, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Region, TenantId,
        Timestamp as EvTimestamp,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use std::sync::Arc;

    fn temp_root(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        p.push(format!("myelin-prstore-{tag}-{nanos}"));
        p
    }

    fn loc() -> RepoLoc {
        RepoLoc::new("acme", "fr-par", "core")
    }

    fn ctx_base() -> EmitContextBase {
        EmitContextBase {
            tenant: TenantId("acme".into()),
            region: Region("fr-par".into()),
            actor: Actor(Principal::stub(
                PrincipalId("p".into()),
                PrincipalKind::Human,
                TenantId("acme".into()),
            )),
            schema_ver: 1,
            occurred_at: EvTimestamp("2026-06-29T00:00:00Z".into()),
            recorded_at: EvTimestamp("2026-06-29T00:00:01Z".into()),
            caused_by: Some(CausedBy("session:merge".into())),
        }
    }

    fn seed_main_then_descendant(repo: &DurableGitRepo) -> (CoreOid, CoreOid) {
        let (c1, _b1, _p1) = repo
            .build_file_commit(
                "refs/heads/main",
                "a.txt",
                b"v1\n",
                "c1",
                "psn@acme.noreply",
                "psn@acme.noreply",
            )
            .unwrap();
        repo.update_ref_cas(
            "refs/heads/main",
            None,
            Some(&c1),
            "create",
            "psn@acme.noreply",
        )
        .unwrap();
        let (c2, _b2, _p2) = repo
            .build_file_commit(
                "refs/heads/main",
                "a.txt",
                b"v2\n",
                "c2",
                "psn@acme.noreply",
                "psn@acme.noreply",
            )
            .unwrap();
        (c1, c2)
    }

    fn open_record(number: u64, base: &str, head_oid: &str, author: &str) -> PrRecord {
        let pr = PullRequest::open(number, base, "refs/heads/feature", author, false);
        PrRecord::open(&pr, head_oid)
    }

    fn durable_ref_store(repo: Arc<DurableGitRepo>) -> RefStore {
        let outbox = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        RefStore::open_durable(repo, "core", ctx_base(), outbox, minter)
    }

    #[test]
    fn the_repository_default_branch_gets_the_baseline_review_policy() {
        let trunk = RefName::new("refs/heads/trunk");

        assert_eq!(
            effective_ruleset(None, "refs/heads/trunk", &trunk).required_approvals,
            1
        );
        assert_eq!(
            effective_ruleset(None, "refs/heads/feature", &trunk).required_approvals,
            0
        );
        assert_eq!(
            effective_ruleset(None, "refs/heads/main", &trunk).required_approvals,
            1,
            "renaming the default branch must not silently relax an established main branch"
        );
    }

    #[test]
    fn branch_protection_config_survives_a_fresh_store() {
        let root = temp_root("prot");
        let gitstore = DurableGitStore::rooted(&root);
        gitstore.create_repo(&loc()).unwrap();
        let store = DurablePrStore::rooted(&root);
        let cfg = BranchProtectionConfig {
            rulesets: vec![BranchProtectionRuleset {
                ref_pattern: "refs/heads/main".into(),
                required_contexts: vec!["ci/build".into()],
                required_approvals: 1,
                require_codeowner_review: false,
                require_conversation_resolution: false,
                allow_force_push: false,
            }],
        };
        store.put_protection(&loc(), &cfg).unwrap();
        let store2 = DurablePrStore::rooted(&root);
        assert_eq!(store2.get_protection(&loc()).unwrap(), Some(cfg));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn oversized_branch_protection_is_rejected_before_read_or_write() {
        let root = temp_root("protection-size-bound");
        let gitstore = DurableGitStore::rooted(&root);
        gitstore.create_repo(&loc()).unwrap();
        let store = DurablePrStore::rooted(&root);
        let config = BranchProtectionConfig {
            rulesets: vec![BranchProtectionRuleset {
                ref_pattern: "refs/heads/main".into(),
                required_contexts: vec!["x".repeat(BRANCH_PROTECTION_MAX_BYTES)],
                required_approvals: 1,
                require_codeowner_review: false,
                require_conversation_resolution: false,
                allow_force_push: false,
            }],
        };
        assert!(matches!(
            store.put_protection(&loc(), &config),
            Err(DurableError::Git(message))
                if message == "branch protection limit exceeded: serialized bytes"
        ));

        let path = store.protection_path(&loc()).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, vec![b'x'; BRANCH_PROTECTION_MAX_BYTES + 1]).unwrap();
        assert!(matches!(
            store.get_protection(&loc()),
            Err(DurableError::Git(message))
                if message == "branch protection limit exceeded: serialized bytes"
        ));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_corrupt_protection_file_is_an_error_not_a_silent_none() {
        let root = temp_root("prot-corrupt");
        let gitstore = DurableGitStore::rooted(&root);
        gitstore.create_repo(&loc()).unwrap();
        let store = DurablePrStore::rooted(&root);
        store
            .put_protection(&loc(), &BranchProtectionConfig::default())
            .unwrap();
        let mut path = None;
        for entry in walkdir(&root) {
            if entry.file_name().and_then(|s| s.to_str()) == Some("branch-protection.json") {
                path = Some(entry);
                break;
            }
        }
        let path = path.expect("branch-protection.json was written by put_protection");
        std::fs::write(&path, b"{ this is not valid json").unwrap();

        let result = store.get_protection(&loc());
        assert!(
            result.is_err(),
            "a corrupt branch-protection.json must be Err (fail-closed), got {result:?}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn title_and_body_round_trip_durably() {
        let root = temp_root("title");
        let gitstore = DurableGitStore::rooted(&root);
        gitstore.create_repo(&loc()).unwrap();
        let store = DurablePrStore::rooted(&root);
        let mut rec = open_record(1, "refs/heads/main", &"a".repeat(40), "psn:author@acme");
        rec.title = "R2.4 MCP HITL server-side verdicts".into();
        rec.body_md = Some("The gate withholds until a human approves.".into());
        rec.author_is_agent = true;
        rec.updated_at = Some(1_752_000_000);
        store.open_pr(&loc(), &rec).unwrap();

        let back = DurablePrStore::rooted(&root)
            .get(&loc(), 1)
            .unwrap()
            .unwrap();
        assert_eq!(back.title, "R2.4 MCP HITL server-side verdicts");
        assert_eq!(
            back.body_md.as_deref(),
            Some("The gate withholds until a human approves.")
        );
        assert!(back.author_is_agent);
        assert_eq!(back.updated_at, Some(1_752_000_000));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn concurrent_record_updates_preserve_every_mutation() {
        const WRITERS: usize = 32;
        let root = temp_root("concurrent-updates");
        let gitstore = DurableGitStore::rooted(&root);
        gitstore.create_repo(&loc()).unwrap();
        let store = Arc::new(DurablePrStore::rooted(&root));
        store
            .open_pr(
                &loc(),
                &open_record(1, "refs/heads/main", &"a".repeat(40), "psn:author@acme"),
            )
            .unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(WRITERS));
        let mut handles = Vec::with_capacity(WRITERS);
        for writer in 0..WRITERS {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                store.update(&loc(), 1, |record| {
                    record.endorsed_contexts.push(format!("ci/writer-{writer}"));
                    Ok(())
                })
            }));
        }
        for handle in handles {
            handle
                .join()
                .expect("writer must not panic")
                .expect("writer must persist");
        }

        let record = store.get(&loc(), 1).unwrap().unwrap();
        assert_eq!(
            record.endorsed_contexts.len(),
            WRITERS,
            "no successful concurrent update may be overwritten"
        );
        drop(store);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn record_filename_and_embedded_number_must_match() {
        let root = temp_root("record-identity");
        let gitstore = DurableGitStore::rooted(&root);
        gitstore.create_repo(&loc()).unwrap();
        let store = DurablePrStore::rooted(&root);
        let record = open_record(2, "refs/heads/main", &"a".repeat(40), "psn:author@acme");
        let path = store.pr_path(&loc(), 1).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();

        assert!(
            matches!(store.get(&loc(), 1), Err(DurableError::Git(message)) if message.contains("identity mismatch")),
            "a point read must not expose the mismatched record"
        );
        assert!(
            matches!(
                store.list_bounded(&loc(), 10, 10 * PR_RECORD_MAX_BYTES),
                Err(DurableError::Git(message)) if message.contains("identity mismatch")
            ),
            "a list must not expose the mismatched record"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn record_filenames_have_one_positive_decimal_identity() {
        let root = temp_root("record-filename-canon");
        let gitstore = DurableGitStore::rooted(&root);
        gitstore.create_repo(&loc()).unwrap();
        let store = DurablePrStore::rooted(&root);
        let canonical = store.pr_path(&loc(), 1).unwrap();
        std::fs::create_dir_all(canonical.parent().unwrap()).unwrap();
        let alias = canonical.with_file_name("01.json");
        std::fs::write(
            &alias,
            serde_json::to_vec(&open_record(
                1,
                "refs/heads/main",
                &"a".repeat(40),
                "psn:author@acme",
            ))
            .unwrap(),
        )
        .unwrap();

        assert!(matches!(
            store.list_bounded(&loc(), 10, 10 * PR_RECORD_MAX_BYTES),
            Err(DurableError::Io(message)) if message.contains("invalid PR record filename")
        ));
        assert!(matches!(
            store.max_pr_number(&loc()),
            Err(DurableError::Io(message)) if message.contains("invalid PR record filename")
        ));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn bounded_list_stops_before_reading_surplus_records() {
        let root = temp_root("bounded-list");
        let gitstore = DurableGitStore::rooted(&root);
        gitstore.create_repo(&loc()).unwrap();
        let store = DurablePrStore::rooted(&root);
        for number in [1, 2] {
            store
                .open_pr(
                    &loc(),
                    &open_record(
                        number,
                        "refs/heads/main",
                        &format!("{number:040}"),
                        "psn:author@acme",
                    ),
                )
                .unwrap();
        }

        assert!(matches!(
            store.list_bounded(&loc(), 1, usize::MAX),
            Err(DurableError::Git(message))
                if message == "pull request list limit exceeded: record count"
        ));
        assert_eq!(store.list_bounded(&loc(), 2, usize::MAX).unwrap().len(), 2);
        assert!(matches!(
            store.list_bounded(&loc(), 2, 1),
            Err(DurableError::Git(message))
                if message == "pull request list limit exceeded: serialized bytes"
        ));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn filesystem_list_page_preserves_exact_counts_filters_order_and_limit_plus_one() {
        let root = temp_root("bounded-page");
        let gitstore = DurableGitStore::rooted(&root);
        gitstore.create_repo(&loc()).unwrap();
        let store = DurablePrStore::rooted(&root);
        let viewer = "psn:viewer@acme";
        let fixtures = [
            (1, PrState::Open, viewer, Some(10)),
            (2, PrState::Draft, "psn:other@acme", Some(30)),
            (3, PrState::Merged, "psn:other@acme", Some(40)),
            (4, PrState::Closed, viewer, Some(50)),
            (5, PrState::Open, "psn:other@acme", None),
        ];
        for (number, state, author, updated_at) in fixtures {
            let mut record =
                open_record(number, "refs/heads/main", &format!("{number:040}"), author);
            record.state = state;
            record.updated_at = updated_at;
            if number == 2 {
                record.reviews.push(ReviewRecord {
                    reviewer_pseudonym: viewer.into(),
                    state: ReviewState::Requested,
                    is_agent: false,
                });
            }
            store.open_pr(&loc(), &record).unwrap();
        }

        let query = PrListQuery::new(PrListState::Open, PrListSort::Updated, 0, 2, viewer).unwrap();
        let first = store
            .list_page_bounded(&loc(), &query, 10, 10 * PR_RECORD_MAX_BYTES)
            .unwrap();
        assert_eq!(
            first.records.iter().map(|r| r.number).collect::<Vec<_>>(),
            [2, 1]
        );
        assert!(
            first.has_older,
            "the third open row is an Older continuation"
        );
        assert_eq!(first.total, 3);
        assert_eq!(
            first.counts,
            PrListCounts {
                open: 3,
                merged: 1,
                closed: 1,
                all: 5,
                yours: 2,
                needs_review: 1,
            }
        );

        let tail_query =
            PrListQuery::new(PrListState::Open, PrListSort::Updated, 2, 2, viewer).unwrap();
        let tail = store
            .list_page_bounded(&loc(), &tail_query, 10, 10 * PR_RECORD_MAX_BYTES)
            .unwrap();
        assert_eq!(
            tail.records.iter().map(|r| r.number).collect::<Vec<_>>(),
            [5]
        );
        assert!(!tail.has_older);
        assert_eq!(
            tail.counts, first.counts,
            "counts never narrow with the page"
        );

        let created =
            PrListQuery::new(PrListState::All, PrListSort::Created, 0, 2, viewer).unwrap();
        let created = store
            .list_page_bounded(&loc(), &created, 10, 10 * PR_RECORD_MAX_BYTES)
            .unwrap();
        assert_eq!(
            created.records.iter().map(|r| r.number).collect::<Vec<_>>(),
            [5, 4]
        );

        let beyond =
            PrListQuery::new(PrListState::Merged, PrListSort::Created, 9, 2, viewer).unwrap();
        let beyond = store
            .list_page_bounded(&loc(), &beyond, 10, 10 * PR_RECORD_MAX_BYTES)
            .unwrap();
        assert!(beyond.records.is_empty());
        assert_eq!(beyond.total, 1);
        assert_eq!(beyond.counts, first.counts);

        assert!(PrListQuery::new(PrListState::All, PrListSort::Created, 0, 0, viewer).is_err());
        assert!(PrListQuery::new(PrListState::All, PrListSort::Created, 0, 101, viewer).is_err());
        assert!(PrListQuery::new(
            PrListState::All,
            PrListSort::Created,
            PR_LIST_OFFSET_MAX,
            1,
            viewer,
        )
        .is_ok());
        assert!(PrListQuery::new(
            PrListState::All,
            PrListSort::Created,
            PR_LIST_OFFSET_MAX + 1,
            1,
            viewer,
        )
        .is_err());
        assert!(matches!(
            store.list_page_bounded(&loc(), &query, 4, usize::MAX),
            Err(DurableError::Git(message))
                if message == "pull request list limit exceeded: record count"
        ));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn oversized_record_is_rejected_before_read_or_write() {
        let root = temp_root("record-size-bound");
        let gitstore = DurableGitStore::rooted(&root);
        gitstore.create_repo(&loc()).unwrap();
        let store = DurablePrStore::rooted(&root);
        let mut record = open_record(1, "refs/heads/main", &"a".repeat(40), "psn:author@acme");
        record.body_md = Some("x".repeat(PR_RECORD_MAX_BYTES));
        assert!(matches!(
            store.open_pr(&loc(), &record),
            Err(DurableError::Git(message))
                if message == "pull request record limit exceeded: serialized bytes"
        ));

        let path = store.pr_path(&loc(), 1).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, vec![b'x'; PR_RECORD_MAX_BYTES + 1]).unwrap();
        assert!(matches!(
            store.get(&loc(), 1),
            Err(DurableError::Git(message))
                if message == "pull request record limit exceeded: serialized bytes"
        ));
        assert!(matches!(
            store.list_bounded(&loc(), 10, 10 * PR_RECORD_MAX_BYTES),
            Err(DurableError::Git(message))
                if message == "pull request record limit exceeded: serialized bytes"
        ));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_legacy_record_without_title_deserializes_with_defaults() {
        let legacy = serde_json::json!({
            "number": 7,
            "state": "Open",
            "base_ref": "refs/heads/main",
            "head_ref": "refs/heads/feature",
            "head_oid": "deadbeef",
            "author_pseudonym": "psn:old@acme",
            "reviews": [],
            "green_contexts": [],
            "fork_unendorsed_contexts": [],
            "endorsed_contexts": [],
            "codeowner_review_satisfied": false,
            "outstanding_conversations": 0
        });
        let rec: PrRecord = serde_json::from_value(legacy).expect("legacy record deserializes");
        assert_eq!(rec.number, 7);
        assert_eq!(
            rec.title, "",
            "no title → empty (the list renders #number, honest)"
        );
        assert_eq!(rec.body_md, None);
        assert!(!rec.author_is_agent);
        assert_eq!(rec.updated_at, None);
    }

    #[test]
    fn checks_summary_rolls_up_from_greens_and_required_set() {
        let ruleset = BranchProtectionRuleset {
            ref_pattern: "refs/heads/main".into(),
            required_contexts: vec!["ci/build".into(), "ci/test".into()],
            required_approvals: 0,
            require_codeowner_review: false,
            require_conversation_resolution: false,
            allow_force_push: false,
        };
        let mut rec = open_record(1, "refs/heads/main", "abc", "psn:a@acme");

        let s = rec.checks_summary(&ruleset);
        assert_eq!(s.verdict, ChecksVerdict::Running);
        assert_eq!((s.passing, s.failing, s.total), (0, 0, 2));

        rec.green_contexts = vec!["ci/build".into()];
        assert_eq!(rec.checks_summary(&ruleset).verdict, ChecksVerdict::Running);

        rec.green_contexts = vec!["ci/build".into(), "ci/test".into()];
        let s = rec.checks_summary(&ruleset);
        assert_eq!(s.verdict, ChecksVerdict::Pass);
        assert_eq!(s.passing, 2);

        let empty_rs = BranchProtectionRuleset {
            required_contexts: vec![],
            ..ruleset.clone()
        };
        let mut fresh = open_record(2, "refs/heads/main", "abc", "psn:a@acme");
        assert_eq!(fresh.checks_summary(&empty_rs).verdict, ChecksVerdict::None);
        fresh.green_contexts = vec!["ci/build".into()];
        assert_eq!(fresh.checks_summary(&empty_rs).verdict, ChecksVerdict::Pass);
    }

    fn walkdir(dir: &std::path::Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    out.extend(walkdir(&p));
                } else {
                    out.push(p);
                }
            }
        }
        out
    }

    #[test]
    fn protected_ref_defaults_closed_no_author_policy_can_open_it() {
        let root = temp_root("closed");
        let gitstore = DurableGitStore::rooted(&root);
        let repo = Arc::new(gitstore.create_repo(&loc()).unwrap());
        let (_c1, c2) = seed_main_then_descendant(&repo);
        let store = DurablePrStore::rooted(&root);
        store
            .open_pr(
                &loc(),
                &open_record(1, "refs/heads/main", &c2.0, "psn:author@acme"),
            )
            .unwrap();
        let rs = durable_ref_store(repo.clone());
        let attempt = merge_pr(
            &store,
            &loc(),
            1,
            &rs,
            &repo,
            "psn:author@acme",
            PushProvenance::NonAgent,
        )
        .unwrap();
        assert!(
            matches!(attempt, MergeAttempt::Blocked(_)),
            "default-closed blocks: {attempt:?}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn protected_merge_needs_human_provenance_without_losing_agent_authorship() {
        let root = temp_root("agent-merge-provenance");
        let gitstore = DurableGitStore::rooted(&root);
        let repo = Arc::new(gitstore.create_repo(&loc()).unwrap());
        let (base, head) = seed_main_then_descendant(&repo);
        let store = DurablePrStore::rooted(&root);
        store
            .put_protection(
                &loc(),
                &BranchProtectionConfig {
                    rulesets: vec![BranchProtectionRuleset {
                        ref_pattern: "refs/heads/main".into(),
                        required_contexts: Vec::new(),
                        required_approvals: 0,
                        require_codeowner_review: false,
                        require_conversation_resolution: false,
                        allow_force_push: false,
                    }],
                },
            )
            .unwrap();
        store
            .open_pr(
                &loc(),
                &open_record(1, "refs/heads/main", &head.0, "psn:author@acme"),
            )
            .unwrap();
        let refs = durable_ref_store(repo.clone());

        assert_eq!(
            merge_pr(
                &store,
                &loc(),
                1,
                &refs,
                &repo,
                "agent-7@acme.noreply",
                PushProvenance::Agent,
            )
            .unwrap(),
            MergeAttempt::RefRefused(crate::receive_pack::RejectReason::AgentNeedsHuman {
                ref_name: RefName::new("refs/heads/main"),
            })
        );
        assert_eq!(
            refs.tip(&RefName::new("refs/heads/main")),
            Some(PushOid::new(base.0))
        );
        assert_eq!(store.get(&loc(), 1).unwrap().unwrap().state, PrState::Open);

        assert!(matches!(
            merge_pr(
                &store,
                &loc(),
                1,
                &refs,
                &repo,
                "agent-7@acme.noreply",
                PushProvenance::HumanApprovedAgent,
            )
            .unwrap(),
            MergeAttempt::Merged { .. }
        ));
        assert_eq!(
            refs.tip(&RefName::new("refs/heads/main")),
            Some(PushOid::new(head.0))
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn merge_admits_only_with_genuine_repo_required_checks_and_nonauthor_approval() {
        let root = temp_root("genuine");
        let gitstore = DurableGitStore::rooted(&root);
        let repo = Arc::new(gitstore.create_repo(&loc()).unwrap());
        let (_c1, c2) = seed_main_then_descendant(&repo);
        let store = DurablePrStore::rooted(&root);
        store
            .put_protection(
                &loc(),
                &BranchProtectionConfig {
                    rulesets: vec![BranchProtectionRuleset {
                        ref_pattern: "refs/heads/main".into(),
                        required_contexts: vec!["ci/build".into()],
                        required_approvals: 1,
                        require_codeowner_review: false,
                        require_conversation_resolution: false,
                        allow_force_push: false,
                    }],
                },
            )
            .unwrap();
        store
            .open_pr(
                &loc(),
                &open_record(1, "refs/heads/main", &c2.0, "psn:author@acme"),
            )
            .unwrap();
        let rs = durable_ref_store(repo.clone());

        assert!(matches!(
            merge_pr(
                &store,
                &loc(),
                1,
                &rs,
                &repo,
                "psn:m@acme",
                PushProvenance::NonAgent,
            )
            .unwrap(),
            MergeAttempt::Blocked(_)
        ));

        let mut rec = store.get(&loc(), 1).unwrap().unwrap();
        rec.green_contexts = vec!["ci/build".into()];
        rec.reviews.push(ReviewRecord {
            reviewer_pseudonym: "psn:author@acme".into(),
            state: ReviewState::Submitted(ReviewVerdict::Approve),
            is_agent: false,
        });
        store.put(&loc(), &rec).unwrap();
        assert!(
            matches!(
                merge_pr(
                    &store,
                    &loc(),
                    1,
                    &rs,
                    &repo,
                    "psn:m@acme",
                    PushProvenance::NonAgent,
                )
                .unwrap(),
                MergeAttempt::Blocked(_)
            ),
            "a self-approval must NOT satisfy the approval threshold"
        );

        let mut rec = store.get(&loc(), 1).unwrap().unwrap();
        rec.reviews.push(ReviewRecord {
            reviewer_pseudonym: "psn:reviewer@acme".into(),
            state: ReviewState::Submitted(ReviewVerdict::Approve),
            is_agent: false,
        });
        store.put(&loc(), &rec).unwrap();
        match merge_pr(
            &store,
            &loc(),
            1,
            &rs,
            &repo,
            "psn:m@acme",
            PushProvenance::NonAgent,
        )
        .unwrap()
        {
            MergeAttempt::Merged { new_oid, .. } => assert_eq!(new_oid, c2.0),
            other => panic!("expected Merged, got {other:?}"),
        }
        assert_eq!(
            rs.tip(&RefName::new("refs/heads/main")),
            Some(PushOid::new(c2.0))
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn max_pr_number_counts_a_corrupt_record_so_its_number_is_never_reused() {
        let root = temp_root("prnum");
        let store = DurablePrStore::rooted(&root);
        store
            .open_pr(
                &loc(),
                &open_record(1, "refs/heads/main", &"a".repeat(40), "psn:a@acme"),
            )
            .unwrap();
        store
            .open_pr(
                &loc(),
                &open_record(2, "refs/heads/main", &"b".repeat(40), "psn:a@acme"),
            )
            .unwrap();
        let corrupt = store.pr_path(&loc(), 3).unwrap();
        std::fs::create_dir_all(corrupt.parent().unwrap()).unwrap();
        std::fs::write(&corrupt, b"{ this is not valid PrRecord json").unwrap();

        let list_error = store
            .list_bounded(&loc(), 10, 10 * PR_RECORD_MAX_BYTES)
            .expect_err("the authoritative PR list must surface a corrupt record");
        assert!(list_error.to_string().contains("parse"));
        assert_eq!(
            store.max_pr_number(&loc()).unwrap(),
            Some(3),
            "the corrupt highest-numbered record still sets the max (filename-authoritative)"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn counting_approvals_dedups_reviewers_and_excludes_agents() {
        let approve = |who: &str, agent: bool| ReviewRecord {
            reviewer_pseudonym: who.into(),
            state: ReviewState::Submitted(ReviewVerdict::Approve),
            is_agent: agent,
        };

        let mut rec = open_record(1, "refs/heads/main", &"a".repeat(40), "psn:author@acme");
        rec.reviews.push(approve("psn:rev1@acme", false));
        rec.reviews.push(approve("psn:rev1@acme", false));
        rec.reviews.push(approve("psn:rev1@acme", false));
        assert_eq!(
            rec.counting_approvals(),
            1,
            "N approvals by ONE reviewer count once"
        );

        rec.reviews.push(approve("psn:agent@acme", true));
        assert_eq!(
            rec.counting_approvals(),
            1,
            "an agent approval must not count toward the gate"
        );

        rec.reviews.push(approve("psn:author@acme", false));
        assert_eq!(rec.counting_approvals(), 1, "self-approval must not count");

        rec.reviews.push(approve("psn:rev2@acme", false));
        assert_eq!(
            rec.counting_approvals(),
            2,
            "two DISTINCT human reviewers count as two"
        );
    }

    #[test]
    fn the_latest_state_per_reviewer_drives_work_and_merge_readiness() {
        let reviewer = "psn:reviewer@acme";
        let mut rec = open_record(1, "refs/heads/main", &"a".repeat(40), "psn:author@acme");
        let review = |state| ReviewRecord {
            reviewer_pseudonym: reviewer.into(),
            state,
            is_agent: false,
        };

        rec.reviews.push(review(ReviewState::Requested));
        assert!(rec.is_review_requested_of(reviewer));
        assert!(rec.has_review_relationship_with(reviewer));
        assert_eq!(rec.review_state_label(), "requested");

        rec.reviews
            .push(review(ReviewState::Submitted(ReviewVerdict::Approve)));
        assert!(
            !rec.is_review_requested_of(reviewer),
            "a submitted decision completes the active request"
        );
        assert!(
            rec.has_review_relationship_with(reviewer),
            "completion does not erase the reviewer's access relationship"
        );
        assert_eq!(rec.counting_approvals(), 1);
        assert!(!rec.has_blocking_review());

        rec.reviews.push(review(ReviewState::Submitted(
            ReviewVerdict::RequestChanges,
        )));
        assert_eq!(
            rec.counting_approvals(),
            0,
            "an older approval cannot survive a newer decision by the same reviewer"
        );
        assert!(rec.has_blocking_review());
        assert_eq!(rec.review_state_label(), "changes");

        rec.reviews.push(review(ReviewState::Requested));
        assert!(rec.is_review_requested_of(reviewer));
        assert!(!rec.has_blocking_review());
        assert_eq!(rec.review_state_label(), "requested");
    }

    #[test]
    fn arbitrary_or_nondescendant_head_oid_is_refused() {
        let root = temp_root("head");
        let gitstore = DurableGitStore::rooted(&root);
        let repo = Arc::new(gitstore.create_repo(&loc()).unwrap());
        let (c1, c2) = seed_main_then_descendant(&repo);
        let store = DurablePrStore::rooted(&root);
        let rs = durable_ref_store(repo.clone());
        rs.receive(
            &PushSession {
                updates: vec![ProposedRefUpdate {
                    ref_name: RefName::new("refs/heads/feat"),
                    expected_old: PushOid::zero(),
                    new_oid: PushOid::new(c2.0.clone()),
                    forced: false,
                    commit_oids: vec![PushOid::new(c2.0.clone())],
                }],
                quarantine: vec![],
                pusher: Pusher::direct("psn:m@acme", false),
            },
            &InMemoryObjectDb::new(),
            CrashPoint::None,
        )
        .unwrap();

        let bogus = "0".repeat(40);
        store
            .open_pr(
                &loc(),
                &open_record(1, "refs/heads/feat", &bogus, "psn:author@acme"),
            )
            .unwrap();
        assert!(matches!(
            merge_pr(
                &store,
                &loc(),
                1,
                &rs,
                &repo,
                "psn:m@acme",
                PushProvenance::NonAgent,
            )
            .unwrap(),
            MergeAttempt::InvalidHead(_)
        ));

        store
            .open_pr(
                &loc(),
                &open_record(2, "refs/heads/feat", &c1.0, "psn:author@acme"),
            )
            .unwrap();
        assert!(
            matches!(
                merge_pr(
                    &store,
                    &loc(),
                    2,
                    &rs,
                    &repo,
                    "psn:m@acme",
                    PushProvenance::NonAgent,
                )
                .unwrap(),
                MergeAttempt::InvalidHead(_)
            ),
            "an ancestor (non-descendant) head is refused"
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
