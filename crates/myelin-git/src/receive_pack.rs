use crate::events::GIT_REF_UPDATED;
use myelin_events::{
    AggregateKey, ArtifactRef, DataRole, EmitContextBase, EventDraft, EventType, IdMinter,
    OutboxError, OutboxStore, OutboxTx, Visibility,
};
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::check_status::{
    CheckContext, CheckState, CheckStatus, CheckStatusProjection, GitOid, HumanisedRef,
    Timestamp as CheckTimestamp, TrustTier,
};
use crate::lifecycle::{
    evaluate_ruleset, BlockReason, BranchProtectionRuleset, MergeContext, RulesetOutcome,
};
use crate::merge_gate::{
    evaluate_merge_gate, parse_required_context, MergeGateOutcome, MergeGatePolicy, UnmetContext,
};

pub const GIT_REF_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS git_ref (
    tenant      TEXT    NOT NULL,
    region      TEXT    NOT NULL,
    repo        TEXT    NOT NULL,
    ref_name    TEXT    NOT NULL,
    target_oid  TEXT    NOT NULL,
    update_seq  BIGINT  NOT NULL DEFAULT 0,
    PRIMARY KEY (tenant, repo, ref_name)
);
CREATE TABLE IF NOT EXISTS git_reflog (
    tenant      TEXT    NOT NULL,
    repo        TEXT    NOT NULL,
    ref_name    TEXT    NOT NULL,
    old_oid     TEXT,
    new_oid     TEXT    NOT NULL,
    update_seq  BIGINT  NOT NULL,
    pusher_pseudonym TEXT NOT NULL,
    at          TIMESTAMPTZ NOT NULL DEFAULT now()
);";

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RefName(pub String);

impl RefName {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }
    pub fn validate(&self) -> Result<(), RefNameError> {
        let name = self.0.as_str();
        let invalid = !name.starts_with("refs/")
            || name.ends_with('/')
            || name.ends_with('.')
            || name.contains("//")
            || name.contains("..")
            || name.contains("@{")
            || name.contains([':', '\\'])
            || name
                .split('/')
                .any(|part| part.is_empty() || part.starts_with('.') || part.ends_with(".lock"))
            || name.chars().any(|c| {
                c.is_ascii_control()
                    || c.is_ascii_whitespace()
                    || matches!(c, '~' | '^' | '?' | '*' | '[')
            });
        if invalid {
            Err(RefNameError)
        } else {
            Ok(())
        }
    }
    pub fn is_protected(&self) -> bool {
        self.0 == "refs/heads/main" || self.0.starts_with("refs/heads/release/")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RefNameError;

impl std::fmt::Display for RefNameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid canonical Git ref name")
    }
}

impl std::error::Error for RefNameError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitRefEventKey {
    encoded_repo: myelin_events::SubjectComponent,
    encoded_ref: myelin_events::SubjectComponent,
}

impl GitRefEventKey {
    pub fn new(repo: &str, ref_name: &RefName) -> Result<Self, GitRefEventKeyError> {
        ref_name.validate().map_err(|_| GitRefEventKeyError)?;
        Ok(Self {
            encoded_repo: myelin_events::SubjectComponent::encode(repo)
                .map_err(|_| GitRefEventKeyError)?,
            encoded_ref: myelin_events::SubjectComponent::encode(&ref_name.0)
                .map_err(|_| GitRefEventKeyError)?,
        })
    }

    pub fn parse_id(id: &str) -> Result<(String, RefName), GitRefEventKeyError> {
        let (repo, ref_name) = id.split_once(':').ok_or(GitRefEventKeyError)?;
        let repo = myelin_events::SubjectComponent::parse(repo)
            .map_err(|_| GitRefEventKeyError)?
            .decode();
        let ref_name = RefName::new(
            myelin_events::SubjectComponent::parse(ref_name)
                .map_err(|_| GitRefEventKeyError)?
                .decode(),
        );
        ref_name.validate().map_err(|_| GitRefEventKeyError)?;
        Ok((repo, ref_name))
    }

    pub fn aggregate(&self) -> AggregateKey {
        AggregateKey(format!("ref:{}", self.id()))
    }

    pub fn subject(&self, tenant: &str) -> Result<ArtifactRef, GitRefEventKeyError> {
        myelin_refs::parse(&format!("myelin://{tenant}/git/ref/{}", self.id()))
            .map_err(|_| GitRefEventKeyError)
    }

    fn id(&self) -> String {
        format!(
            "{}:{}",
            self.encoded_repo.as_str(),
            self.encoded_ref.as_str()
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GitRefEventKeyError;

impl std::fmt::Display for GitRefEventKeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid canonical Git ref event key")
    }
}

impl std::error::Error for GitRefEventKeyError {}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Oid(pub String);

impl Oid {
    pub fn new(hex: impl Into<String>) -> Self {
        Self(hex.into())
    }
    pub fn zero() -> Self {
        Self("0".repeat(40))
    }
    pub fn is_zero(&self) -> bool {
        self.0.chars().all(|c| c == '0')
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuarantineObject {
    pub oid: Oid,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProposedRefUpdate {
    pub ref_name: RefName,
    pub expected_old: Oid,
    pub new_oid: Oid,
    pub forced: bool,
    pub commit_oids: Vec<Oid>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pusher {
    pub pseudonym: String,
    pub is_agent: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PushSession {
    pub updates: Vec<ProposedRefUpdate>,
    pub quarantine: Vec<QuarantineObject>,
    pub pusher: Pusher,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RejectReason {
    InvalidRefName,
    DuplicateRefUpdate {
        ref_name: RefName,
    },
    ForcePushOnProtected {
        ref_name: RefName,
    },
    DeleteProtected {
        ref_name: RefName,
    },
    SecretDetected {
        oid: Oid,
        pattern: String,
    },
    ObjectTooLarge {
        oid: Oid,
        size: usize,
        limit: usize,
    },
    AgentNeedsHuman {
        ref_name: RefName,
    },
    PseudonymRequired,
    NonPseudonymousCommit {
        oid: Oid,
        identity: crate::commit::NonPseudonymousIdentity,
    },
    NonFastForward {
        ref_name: RefName,
        expected: Oid,
        actual: Oid,
    },
    ProtectedCheckNotGreen {
        ref_name: RefName,
        unmet: Vec<UnmetContext>,
    },
    ProtectedGateInput {
        ref_name: RefName,
        detail: String,
    },
    ProtectedRulesetNotSatisfied {
        ref_name: RefName,
        reasons: Vec<BlockReason>,
    },
}

#[derive(Clone, Debug)]
pub struct PushPolicy {
    pub max_object_bytes: usize,
    pub secret_patterns: Vec<String>,
    pub protected_needs_human: bool,
    pub tenant: String,
}

impl Default for PushPolicy {
    fn default() -> Self {
        Self {
            max_object_bytes: 50 * 1024 * 1024,
            secret_patterns: vec![
                ["AK", "IA"].concat(),
                ["-----BEGIN ", "PRIVATE KEY"].concat(),
                ["-----BEGIN RSA ", "PRIVATE KEY"].concat(),
            ],
            protected_needs_human: true,
            tenant: String::new(),
        }
    }
}

impl PushPolicy {
    pub fn evaluate(&self, push: &PushSession) -> Result<(), RejectReason> {
        if push.pusher.pseudonym.trim().is_empty() {
            return Err(RejectReason::PseudonymRequired);
        }
        for u in &push.updates {
            if u.ref_name.is_protected() {
                if u.new_oid.is_zero() {
                    return Err(RejectReason::DeleteProtected {
                        ref_name: u.ref_name.clone(),
                    });
                }
                if u.forced {
                    return Err(RejectReason::ForcePushOnProtected {
                        ref_name: u.ref_name.clone(),
                    });
                }
                if self.protected_needs_human && push.pusher.is_agent {
                    return Err(RejectReason::AgentNeedsHuman {
                        ref_name: u.ref_name.clone(),
                    });
                }
            }
        }
        for obj in &push.quarantine {
            if obj.bytes.len() > self.max_object_bytes {
                return Err(RejectReason::ObjectTooLarge {
                    oid: obj.oid.clone(),
                    size: obj.bytes.len(),
                    limit: self.max_object_bytes,
                });
            }
            let haystack = String::from_utf8_lossy(&obj.bytes);
            for pat in &self.secret_patterns {
                if haystack.contains(pat.as_str()) {
                    return Err(RejectReason::SecretDetected {
                        oid: obj.oid.clone(),
                        pattern: pat.clone(),
                    });
                }
            }
            if crate::commit::is_commit_object(&obj.bytes) {
                if let Err(identity) =
                    crate::commit::enforce_pseudonymous_commit(&obj.bytes, &self.tenant)
                {
                    return Err(RejectReason::NonPseudonymousCommit {
                        oid: obj.oid.clone(),
                        identity,
                    });
                }
            }
        }
        Ok(())
    }
}

fn synthetic_check_fact(head: &GitOid, ctx: CheckContext, trust: TrustTier) -> CheckStatus {
    CheckStatus {
        tenant: myelin_events::TenantId("_wirepush".into()),
        repo: ArtifactRef("myelin://_wirepush/git/repo/_".into()),
        commit_oid: head.clone(),
        context: ctx,
        state: CheckState::Success,
        required: true,
        run: ArtifactRef("myelin://_wirepush/ci/run/_".into()),
        run_attempt: 1,
        trust_tier: trust,
        details_ref: ArtifactRef("myelin://_wirepush/ci/run/_#s".into()),
        summary: HumanisedRef {
            template_key: "ci.check.updated".into(),
            args: Default::default(),
        },
        started_at: CheckTimestamp("2026-06-29T00:00:00Z".into()),
        completed_at: Some(CheckTimestamp("2026-06-29T00:01:00Z".into())),
        cost_settled: true,
    }
}

#[allow(clippy::too_many_arguments)]
pub fn evaluate_protected_ref_push(
    ref_name: &RefName,
    is_delete: bool,
    is_forced: bool,
    pusher_has_protected_push: bool,
    ruleset: &BranchProtectionRuleset,
    head_oid: &GitOid,
    green_contexts: &[String],
    fork_unendorsed_contexts: &[String],
    endorsed_contexts: &[String],
) -> Result<(), RejectReason> {
    if is_delete {
        return Err(RejectReason::DeleteProtected {
            ref_name: ref_name.clone(),
        });
    }
    if is_forced && !ruleset.allow_force_push {
        return Err(RejectReason::ForcePushOnProtected {
            ref_name: ref_name.clone(),
        });
    }
    if pusher_has_protected_push {
        return Ok(());
    }

    let gate_input = |detail: String| RejectReason::ProtectedGateInput {
        ref_name: ref_name.clone(),
        detail,
    };
    let policy = MergeGatePolicy::from_required_contexts(&ruleset.required_contexts)
        .map_err(|e| gate_input(e.to_string()))?;

    let mut proj = CheckStatusProjection::new();
    for c in green_contexts {
        let ctx = parse_required_context(c).map_err(|e| gate_input(e.to_string()))?;
        proj.apply(&synthetic_check_fact(head_oid, ctx, TrustTier::Trusted));
    }
    for c in fork_unendorsed_contexts {
        let ctx = parse_required_context(c).map_err(|e| gate_input(e.to_string()))?;
        proj.apply(&synthetic_check_fact(
            head_oid,
            ctx,
            TrustTier::UntrustedFork,
        ));
    }
    let endorsed: Vec<CheckContext> = endorsed_contexts
        .iter()
        .map(|c| parse_required_context(c).map_err(|e| gate_input(e.to_string())))
        .collect::<Result<_, _>>()?;

    if let MergeGateOutcome::Blocked { unmet } =
        evaluate_merge_gate(&policy, &proj, head_oid, &endorsed)
    {
        return Err(RejectReason::ProtectedCheckNotGreen {
            ref_name: ref_name.clone(),
            unmet,
        });
    }

    let direct_push_ctx = MergeContext {
        green_contexts: Vec::new(),
        current_approvals: 0,
        codeowner_review_satisfied: false,
        has_blocking_review: false,
        outstanding_conversations: 0,
    };
    let ruleset_no_contexts = BranchProtectionRuleset {
        required_contexts: Vec::new(),
        ..ruleset.clone()
    };
    match evaluate_ruleset(&ruleset_no_contexts, &direct_push_ctx) {
        RulesetOutcome::Satisfied => Ok(()),
        RulesetOutcome::Blocked { reasons } => Err(RejectReason::ProtectedRulesetNotSatisfied {
            ref_name: ref_name.clone(),
            reasons,
        }),
    }
}

pub trait QuarantineMigration {
    fn migrate(&self, objects: &[QuarantineObject]) -> Result<(), String>;
}

#[derive(Clone, Default)]
pub struct InMemoryObjectDb {
    migrated: Arc<std::sync::Mutex<std::collections::BTreeSet<Oid>>>,
}

impl InMemoryObjectDb {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn contains(&self, oid: &Oid) -> bool {
        self.migrated
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(oid)
    }
    pub fn len(&self) -> usize {
        self.migrated
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl QuarantineMigration for InMemoryObjectDb {
    fn migrate(&self, objects: &[QuarantineObject]) -> Result<(), String> {
        let mut g = self.migrated.lock().unwrap_or_else(|e| e.into_inner());
        for o in objects {
            g.insert(o.oid.clone());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrashPoint {
    None,
    AfterPolicy,
    BeforeCommit,
    AfterCommit,
    AfterCommitBeforeApply,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InjectedCrash {
    pub at: CrashPoint,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PushOutcome {
    Accepted {
        moved: Vec<(RefName, Oid, u64)>,
        emitted: Vec<myelin_events::EventId>,
    },
    Rejected(RejectReason),
    Crashed(InjectedCrash),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RefRow {
    target_oid: Oid,
    update_seq: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReflogEntry {
    pub ref_name: RefName,
    pub old_oid: Option<Oid>,
    pub new_oid: Oid,
    pub update_seq: u64,
    pub pusher_pseudonym: String,
}

type RefLock = std::sync::Mutex<()>;

enum RefBacking {
    Memory {
        rows: std::sync::Mutex<BTreeMap<RefName, RefRow>>,
        reflog: std::sync::Mutex<Vec<ReflogEntry>>,
    },
    Disk {
        repo: Arc<crate::durable::DurableGitRepo>,
    },
}

pub struct RefStore {
    repo: String,
    ctx_base: EmitContextBase,
    outbox: OutboxStore,
    minter: Arc<dyn IdMinter>,
    backing: RefBacking,
    locks: std::sync::Mutex<BTreeMap<RefName, Arc<RefLock>>>,
    holder: crate::holder_intent::HolderRegistration,
}

impl RefStore {
    pub fn open(
        repo: impl Into<String>,
        ctx_base: EmitContextBase,
        outbox: OutboxStore,
        minter: Arc<dyn IdMinter>,
    ) -> Self {
        Self {
            repo: repo.into(),
            ctx_base,
            outbox,
            minter,
            backing: RefBacking::Memory {
                rows: std::sync::Mutex::new(BTreeMap::new()),
                reflog: std::sync::Mutex::new(Vec::new()),
            },
            locks: std::sync::Mutex::new(BTreeMap::new()),
            holder: crate::holder_intent::HolderRegistration::auto_register(),
        }
    }

    pub fn open_durable(
        durable_repo: Arc<crate::durable::DurableGitRepo>,
        repo: impl Into<String>,
        ctx_base: EmitContextBase,
        outbox: OutboxStore,
        minter: Arc<dyn IdMinter>,
    ) -> Self {
        Self {
            repo: repo.into(),
            ctx_base,
            outbox,
            minter,
            backing: RefBacking::Disk { repo: durable_repo },
            locks: std::sync::Mutex::new(BTreeMap::new()),
            holder: crate::holder_intent::HolderRegistration::auto_register(),
        }
    }

    pub fn holder(&self) -> &crate::holder_intent::HolderRegistration {
        &self.holder
    }

    pub fn outbox(&self) -> &OutboxStore {
        &self.outbox
    }

    pub fn tip(&self, ref_name: &RefName) -> Option<Oid> {
        self.try_tip(ref_name).ok().flatten()
    }

    pub fn try_tip(&self, ref_name: &RefName) -> Result<Option<Oid>, crate::durable::DurableError> {
        let lock = self.ref_lock(ref_name);
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        self.tip_of(ref_name)
    }

    pub fn reflog(&self) -> Result<Vec<ReflogEntry>, crate::durable::DurableError> {
        self.reflog_bounded(
            crate::durable::REFLOG_MAX_TOTAL_ENTRIES,
            crate::durable::REFLOG_MAX_TOTAL_BYTES,
        )
    }

    fn reflog_bounded(
        &self,
        maximum_total_entries: usize,
        maximum_total_bytes: usize,
    ) -> Result<Vec<ReflogEntry>, crate::durable::DurableError> {
        match &self.backing {
            RefBacking::Memory { reflog, .. } => {
                let reflog = reflog.lock().unwrap_or_else(|e| e.into_inner());
                if reflog.len() > maximum_total_entries {
                    return Err(crate::durable::DurableError::Git(
                        "audit reflog limit exceeded: total entry count".into(),
                    ));
                }
                let materialized_bytes = reflog.iter().try_fold(0usize, |total, entry| {
                    total.checked_add(Self::reflog_entry_string_bytes(entry))
                });
                if materialized_bytes.is_none_or(|bytes| bytes > maximum_total_bytes) {
                    return Err(crate::durable::DurableError::Git(
                        "audit reflog limit exceeded: total bytes".into(),
                    ));
                }
                Ok(reflog.clone())
            }
            RefBacking::Disk { repo } => {
                let mut out = Vec::new();
                let mut input_bytes = 0usize;
                let mut output_bytes = 0usize;
                for (name, _tip) in repo.list_refs_bounded(crate::durable::WIRE_MAX_REFS)? {
                    let remaining_entries = maximum_total_entries
                        .checked_sub(out.len())
                        .ok_or_else(|| {
                            crate::durable::DurableError::Git(
                                "audit reflog limit exceeded: total entry count".into(),
                            )
                        })?;
                    let remaining_input_bytes = maximum_total_bytes
                        .checked_sub(input_bytes)
                        .ok_or_else(|| {
                            crate::durable::DurableError::Git(
                                "audit reflog limit exceeded: total bytes".into(),
                            )
                        })?;
                    let (entries, on_disk_bytes, generation) = repo.reflog_entries_bounded(
                        &name,
                        crate::durable::REFLOG_MAX_ENTRIES_PER_REF.min(remaining_entries),
                        crate::durable::REFLOG_MAX_BYTES_PER_REF.min(remaining_input_bytes),
                    )?;
                    input_bytes = input_bytes.checked_add(on_disk_bytes).ok_or_else(|| {
                        crate::durable::DurableError::Git(
                            "audit reflog limit exceeded: total bytes".into(),
                        )
                    })?;
                    let entry_count = u64::try_from(entries.len()).map_err(|_| {
                        crate::durable::DurableError::Git(
                            "audit reflog entry count does not fit durable generation".into(),
                        )
                    })?;
                    let sequence_base = generation.checked_sub(entry_count).ok_or_else(|| {
                        crate::durable::DurableError::Git(format!(
                            "reflog for {name} is ahead of its durable generation"
                        ))
                    })?;
                    for (i, e) in entries.into_iter().enumerate() {
                        let offset = u64::try_from(i)
                            .ok()
                            .and_then(|index| index.checked_add(1))
                            .ok_or_else(|| {
                                crate::durable::DurableError::Git(
                                    "audit reflog sequence overflow".into(),
                                )
                            })?;
                        let entry = ReflogEntry {
                            ref_name: RefName::new(name.clone()),
                            old_oid: e.old_oid.map(|o| Oid::new(o.0)),
                            new_oid: Oid::new(e.new_oid.0),
                            update_seq: sequence_base.checked_add(offset).ok_or_else(|| {
                                crate::durable::DurableError::Git(
                                    "audit reflog sequence overflow".into(),
                                )
                            })?,
                            pusher_pseudonym: e.committer,
                        };
                        output_bytes = output_bytes
                            .checked_add(Self::reflog_entry_string_bytes(&entry))
                            .ok_or_else(|| {
                                crate::durable::DurableError::Git(
                                    "audit reflog limit exceeded: total bytes".into(),
                                )
                            })?;
                        if output_bytes > maximum_total_bytes {
                            return Err(crate::durable::DurableError::Git(
                                "audit reflog limit exceeded: total bytes".into(),
                            ));
                        }
                        out.push(entry);
                    }
                }
                Ok(out)
            }
        }
    }

    fn reflog_entry_string_bytes(entry: &ReflogEntry) -> usize {
        entry.ref_name.0.len()
            + entry.old_oid.as_ref().map_or(0, |oid| oid.0.len())
            + entry.new_oid.0.len()
            + entry.pusher_pseudonym.len()
    }

    fn tip_of(&self, ref_name: &RefName) -> Result<Option<Oid>, crate::durable::DurableError> {
        match &self.backing {
            RefBacking::Memory { rows, .. } => Ok(rows
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(ref_name)
                .map(|r| r.target_oid.clone())),
            RefBacking::Disk { repo } => repo
                .read_ref(&ref_name.0)
                .map(|tip| tip.map(|o| Oid::new(o.0))),
        }
    }

    fn seq_of(&self, ref_name: &RefName) -> Result<u64, OutboxError> {
        match &self.backing {
            RefBacking::Memory { rows, .. } => Ok(rows
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(ref_name)
                .map(|r| r.update_seq)
                .unwrap_or(0)),
            RefBacking::Disk { repo } => repo.ref_generation(&ref_name.0).map_err(|e| {
                OutboxError(format!(
                    "read durable ref generation for {}: {e}",
                    ref_name.0
                ))
            }),
        }
    }

    fn apply_one(
        &self,
        ref_name: &RefName,
        new_oid: &Oid,
        new_seq: u64,
        old: Option<Oid>,
        pseudonym: &str,
    ) -> Result<(), OutboxError> {
        match &self.backing {
            RefBacking::Memory { rows, reflog } => {
                let mut rows = rows.lock().unwrap_or_else(|e| e.into_inner());
                if new_oid.is_zero() {
                    rows.remove(ref_name);
                } else {
                    rows.insert(
                        ref_name.clone(),
                        RefRow {
                            target_oid: new_oid.clone(),
                            update_seq: new_seq,
                        },
                    );
                }
                drop(rows);
                reflog
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(ReflogEntry {
                        ref_name: ref_name.clone(),
                        old_oid: old,
                        new_oid: new_oid.clone(),
                        update_seq: new_seq,
                        pusher_pseudonym: pseudonym.to_string(),
                    });
                Ok(())
            }
            RefBacking::Disk { repo } => {
                let old_core = old.as_ref().map(|o| crate::core::Oid::new(o.0.clone()));
                let new_core = if new_oid.is_zero() {
                    None
                } else {
                    Some(crate::core::Oid::new(new_oid.0.clone()))
                };
                let msg = format!("receive-pack: {} -> {}", self.repo, ref_name.0);
                repo.update_ref_cas(
                    &ref_name.0,
                    old_core.as_ref(),
                    new_core.as_ref(),
                    &msg,
                    pseudonym,
                )
                .map_err(|e| OutboxError(format!("durable ref apply failed (post-commit): {e}")))
            }
        }
    }

    fn aggregate_for(&self, ref_name: &RefName) -> AggregateKey {
        GitRefEventKey::new(&self.repo, ref_name)
            .expect("receive validates canonical Git ref event keys")
            .aggregate()
    }

    fn subject_for(&self, ref_name: &RefName) -> ArtifactRef {
        GitRefEventKey::new(&self.repo, ref_name)
            .expect("receive validates canonical Git ref event keys")
            .subject(&self.ctx_base.tenant.0)
            .expect("validated Git ref key forms a canonical ArtifactRef")
    }

    pub fn receive<M: QuarantineMigration>(
        &self,
        push: &PushSession,
        migration: &M,
        crash: CrashPoint,
    ) -> Result<PushOutcome, OutboxError> {
        let mut unique_refs = std::collections::BTreeSet::new();
        for update in &push.updates {
            if GitRefEventKey::new(&self.repo, &update.ref_name).is_err() {
                return Ok(PushOutcome::Rejected(RejectReason::InvalidRefName));
            }
            if !unique_refs.insert(update.ref_name.clone()) {
                return Ok(PushOutcome::Rejected(RejectReason::DuplicateRefUpdate {
                    ref_name: update.ref_name.clone(),
                }));
            }
        }

        let policy = PushPolicy {
            tenant: self.ctx_base.tenant.0.clone(),
            ..PushPolicy::default()
        };
        if let Err(reason) = policy.evaluate(push) {
            return Ok(PushOutcome::Rejected(reason));
        }

        if crash == CrashPoint::AfterPolicy {
            return Ok(PushOutcome::Crashed(InjectedCrash { at: crash }));
        }

        if let Err(e) = migration.migrate(&push.quarantine) {
            return Ok(PushOutcome::Rejected(RejectReason::SecretDetected {
                oid: Oid::zero(),
                pattern: format!("object-migration-not-durable: {e}"),
            }));
        }

        if crash == CrashPoint::BeforeCommit {
            return Ok(PushOutcome::Crashed(InjectedCrash { at: crash }));
        }

        let mut targets: Vec<RefName> = push.updates.iter().map(|u| u.ref_name.clone()).collect();
        targets.sort();
        targets.dedup();
        let locks: Vec<Arc<RefLock>> = targets.iter().map(|r| self.ref_lock(r)).collect();
        let _guards: Vec<std::sync::MutexGuard<'_, ()>> = locks
            .iter()
            .map(|l| l.lock().unwrap_or_else(|e| e.into_inner()))
            .collect();
        let _durable_guards: Vec<std::fs::File> = match &self.backing {
            RefBacking::Disk { repo } => targets
                .iter()
                .map(|target| repo.lock_ref_exclusive(&target.0))
                .collect::<Result<_, _>>()
                .map_err(|error| {
                    OutboxError(format!("acquire durable ref linearisation lock: {error}"))
                })?,
            RefBacking::Memory { .. } => Vec::new(),
        };

        for u in &push.updates {
            let actual = self
                .tip_of(&u.ref_name)
                .map_err(|e| {
                    OutboxError(format!("read durable ref tip for {}: {e}", u.ref_name.0))
                })?
                .unwrap_or_else(Oid::zero);
            if actual != u.expected_old {
                return Ok(PushOutcome::Rejected(RejectReason::NonFastForward {
                    ref_name: u.ref_name.clone(),
                    expected: u.expected_old.clone(),
                    actual,
                }));
            }
        }

        let mut tx = self
            .outbox
            .begin(Arc::clone(&self.minter), self.ctx_base.clone());

        let mut planned: Vec<(RefName, Oid, u64, Option<Oid>, myelin_events::EventId)> = Vec::new();
        for u in &push.updates {
            let old = self.tip_of(&u.ref_name).map_err(|e| {
                OutboxError(format!("read durable ref tip for {}: {e}", u.ref_name.0))
            })?;
            let prev_seq = self.seq_of(&u.ref_name)?;
            let new_seq = crate::durable::next_ref_generation(prev_seq).ok_or_else(|| {
                OutboxError(format!("ref generation exhausted for {}", u.ref_name.0))
            })?;

            tx.stage_state_change(format!(
                "git_ref CAS {}:{} {} -> {} (seq {new_seq})",
                self.repo,
                u.ref_name.0,
                old.clone().unwrap_or_else(Oid::zero).0,
                u.new_oid.0
            ));

            let draft = EventDraft {
                type_: EventType(GIT_REF_UPDATED.into()),
                subject: self.subject_for(&u.ref_name),
                aggregate: self.aggregate_for(&u.ref_name),
                payload: serde_json::json!({
                    "repo": self.repo,
                    "ref": u.ref_name.0,
                    "old_oid": old.clone().unwrap_or_else(Oid::zero).0,
                    "new_oid": u.new_oid.0,
                    "forced": u.forced,
                    "commit_oids": u.commit_oids.iter().map(|o| o.0.clone()).collect::<Vec<_>>(),
                    "pusher_pseudonym": push.pusher.pseudonym,
                    "update_seq": new_seq,
                }),
                data_role: DataRole::Processor,
                visibility: Visibility::Internal,
                contains_personal_data: false,
                pii_key_ref: None,
            };
            let id = tx.emit(draft, None)?;
            planned.push((u.ref_name.clone(), u.new_oid.clone(), new_seq, old, id));
        }

        tx.commit()?;

        if crash == CrashPoint::AfterCommitBeforeApply {
            return Ok(PushOutcome::Crashed(InjectedCrash { at: crash }));
        }

        let mut moved = Vec::new();
        let mut emitted = Vec::new();
        for (ref_name, new_oid, new_seq, old, id) in planned {
            self.apply_one(&ref_name, &new_oid, new_seq, old, &push.pusher.pseudonym)?;
            moved.push((ref_name, new_oid, new_seq));
            emitted.push(id);
        }
        drop(_guards);
        drop(_durable_guards);

        if crash == CrashPoint::AfterCommit {
            return Ok(PushOutcome::Crashed(InjectedCrash { at: crash }));
        }

        Ok(PushOutcome::Accepted { moved, emitted })
    }

    fn ref_lock(&self, ref_name: &RefName) -> Arc<RefLock> {
        let mut g = self.locks.lock().unwrap_or_else(|e| e.into_inner());
        Arc::clone(g.entry(ref_name.clone()).or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{Actor, CausedBy, MonotonicMinter, Region, TenantId, Timestamp};
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

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
            occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
            caused_by: Some(CausedBy("session:push-1".into())),
        }
    }

    fn store() -> (RefStore, OutboxStore) {
        let outbox = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let store = RefStore::open("core", ctx_base(), outbox.clone(), minter);
        (store, outbox)
    }

    fn human_push(ref_name: &str, old: Oid, new: Oid) -> PushSession {
        PushSession {
            updates: vec![ProposedRefUpdate {
                ref_name: RefName::new(ref_name),
                expected_old: old,
                new_oid: new.clone(),
                forced: false,
                commit_oids: vec![new],
            }],
            quarantine: vec![QuarantineObject {
                oid: Oid::new("cafe"),
                bytes: b"a normal commit blob".to_vec(),
            }],
            pusher: Pusher {
                pseudonym: "anon-7@acme.noreply".into(),
                is_agent: false,
            },
        }
    }

    const WRITER: bool = false;
    const ADMIN: bool = true;

    fn protected_ruleset(required: &[&str], allow_force_push: bool) -> BranchProtectionRuleset {
        BranchProtectionRuleset {
            ref_pattern: "refs/heads/main".into(),
            required_contexts: required.iter().map(|s| s.to_string()).collect(),
            required_approvals: 0,
            require_codeowner_review: false,
            require_conversation_resolution: false,
            allow_force_push,
        }
    }

    #[test]
    fn protected_direct_push_rejects_delete() {
        let rs = protected_ruleset(&[], false);
        let head = GitOid("0".repeat(40));
        for bypass in [WRITER, ADMIN] {
            assert_eq!(
                evaluate_protected_ref_push(
                    &RefName::new("refs/heads/main"),
                     true,
                     false,
                    bypass,
                    &rs,
                    &head,
                    &[],
                    &[],
                    &[],
                ),
                Err(RejectReason::DeleteProtected {
                    ref_name: RefName::new("refs/heads/main")
                })
            );
        }
    }

    #[test]
    fn protected_direct_push_rejects_force_unless_ruleset_allows() {
        let head = GitOid("abc".into());
        for bypass in [WRITER, ADMIN] {
            assert_eq!(
                evaluate_protected_ref_push(
                    &RefName::new("refs/heads/main"),
                    false,
                     true,
                    bypass,
                    &protected_ruleset(&[], false),
                    &head,
                    &[],
                    &[],
                    &[],
                ),
                Err(RejectReason::ForcePushOnProtected {
                    ref_name: RefName::new("refs/heads/main")
                })
            );
        }
        assert_eq!(
            evaluate_protected_ref_push(
                &RefName::new("refs/heads/main"),
                false,
                true,
                WRITER,
                &protected_ruleset(&[],  true),
                &head,
                &[],
                &[],
                &[],
            ),
            Ok(())
        );
    }

    #[test]
    fn protected_direct_push_requires_the_required_contexts_green_for_the_head() {
        let head = GitOid("deadbeef".into());
        let rs = protected_ruleset(&["ci/build", "ci/test"], false);
        match evaluate_protected_ref_push(
            &RefName::new("refs/heads/main"),
            false,
            false,
            WRITER,
            &rs,
            &head,
            &[],
            &[],
            &[],
        ) {
            Err(RejectReason::ProtectedCheckNotGreen { unmet, .. }) => {
                assert_eq!(unmet.len(), 2, "both required contexts are unmet");
            }
            other => panic!("expected ProtectedCheckNotGreen, got {other:?}"),
        }
        assert!(matches!(
            evaluate_protected_ref_push(
                &RefName::new("refs/heads/main"),
                false,
                false,
                WRITER,
                &rs,
                &head,
                &["ci/build".into()],
                &[],
                &[],
            ),
            Err(RejectReason::ProtectedCheckNotGreen { .. })
        ));
        assert_eq!(
            evaluate_protected_ref_push(
                &RefName::new("refs/heads/main"),
                false,
                false,
                WRITER,
                &rs,
                &head,
                &["ci/build".into(), "ci/test".into()],
                &[],
                &[],
            ),
            Ok(())
        );
    }

    #[test]
    fn protected_direct_push_fork_success_is_neutral_until_endorsed() {
        let head = GitOid("f00".into());
        let rs = protected_ruleset(&["ci/build"], false);
        assert!(matches!(
            evaluate_protected_ref_push(
                &RefName::new("refs/heads/main"),
                false,
                false,
                WRITER,
                &rs,
                &head,
                &[],
                &["ci/build".into()],
                &[],
            ),
            Err(RejectReason::ProtectedCheckNotGreen { .. })
        ));
        assert_eq!(
            evaluate_protected_ref_push(
                &RefName::new("refs/heads/main"),
                false,
                false,
                WRITER,
                &rs,
                &head,
                &[],
                &["ci/build".into()],
                &["ci/build".into()],
            ),
            Ok(())
        );
    }

    #[test]
    fn protected_direct_push_empty_required_set_admits_a_plain_fast_forward() {
        let head = GitOid("cafe".into());
        assert_eq!(
            evaluate_protected_ref_push(
                &RefName::new("refs/heads/main"),
                false,
                false,
                WRITER,
                &protected_ruleset(&[], false),
                &head,
                &[],
                &[],
                &[],
            ),
            Ok(())
        );
    }

    #[test]
    fn protected_direct_push_unparseable_required_context_is_fail_closed() {
        let head = GitOid("beef".into());
        let rs = protected_ruleset(&["ci/build"], false);
        assert!(matches!(
            evaluate_protected_ref_push(
                &RefName::new("refs/heads/main"),
                false,
                false,
                WRITER,
                &rs,
                &head,
                &["ci/".into()],
                &[],
                &[],
            ),
            Err(RejectReason::ProtectedGateInput { .. })
        ));
    }

    fn strict_protected_ruleset() -> BranchProtectionRuleset {
        BranchProtectionRuleset {
            ref_pattern: "refs/heads/main".into(),
            required_contexts: vec!["ci/build".into()],
            required_approvals: 1,
            require_codeowner_review: true,
            require_conversation_resolution: false,
            allow_force_push: false,
        }
    }

    #[test]
    fn writer_direct_push_to_strict_protected_ref_is_denied_by_the_full_ruleset() {
        let head = GitOid("c0ffee".into());
        let rs = strict_protected_ruleset();
        match evaluate_protected_ref_push(
            &RefName::new("refs/heads/main"),
             false,
             false,
            WRITER,
            &rs,
            &head,
            &["ci/build".into()],
            &[],
            &[],
        ) {
            Err(RejectReason::ProtectedRulesetNotSatisfied { reasons, .. }) => {
                assert!(
                    reasons
                        .iter()
                        .any(|r| matches!(r, BlockReason::InsufficientApprovals { need: 1, .. })),
                    "a direct push carries 0 approvals - the 1 required is unmet: {reasons:?}"
                );
                assert!(
                    reasons
                        .iter()
                        .any(|r| matches!(r, BlockReason::CodeownerReviewMissing)),
                    "a direct push has no CODEOWNERS approval: {reasons:?}"
                );
            }
            other => panic!("expected ProtectedRulesetNotSatisfied, got {other:?}"),
        }
    }

    #[test]
    fn writer_direct_push_without_greens_is_denied_at_the_contexts_half() {
        let head = GitOid("d00d".into());
        assert!(matches!(
            evaluate_protected_ref_push(
                &RefName::new("refs/heads/main"),
                false,
                false,
                WRITER,
                &strict_protected_ruleset(),
                &head,
                &[],
                &[],
                &[],
            ),
            Err(RejectReason::ProtectedCheckNotGreen { .. })
        ));
    }

    #[test]
    fn admin_bypass_may_direct_push_a_strict_protected_ref() {
        let head = GitOid("ba5e".into());
        assert_eq!(
            evaluate_protected_ref_push(
                &RefName::new("refs/heads/main"),
                 false,
                 false,
                ADMIN,
                &strict_protected_ruleset(),
                &head,
                &[],
                &[],
                &[],
            ),
            Ok(())
        );
    }

    #[test]
    fn writer_direct_push_blocked_by_required_approvals_alone() {
        let head = GitOid("feed".into());
        let rs = BranchProtectionRuleset {
            required_approvals: 2,
            require_codeowner_review: false,
            ..protected_ruleset(&[], false)
        };
        assert!(matches!(
            evaluate_protected_ref_push(
                &RefName::new("refs/heads/main"),
                false,
                false,
                WRITER,
                &rs,
                &head,
                &[],
                &[],
                &[],
            ),
            Err(RejectReason::ProtectedRulesetNotSatisfied { .. })
        ));
    }

    #[test]
    fn duplicate_ref_updates_reject_the_whole_push_before_side_effects() {
        fn assert_rejected(
            store: &RefStore,
            outbox: &OutboxStore,
            mut push: PushSession,
            expected_tip: Option<Oid>,
            expected_commits: usize,
        ) {
            let ref_name = push.updates[0].ref_name.clone();
            push.updates.push(push.updates[0].clone());
            let migration = InMemoryObjectDb::new();

            assert_eq!(
                store.receive(&push, &migration, CrashPoint::None).unwrap(),
                PushOutcome::Rejected(RejectReason::DuplicateRefUpdate {
                    ref_name: ref_name.clone()
                })
            );
            assert_eq!(store.tip(&ref_name), expected_tip, "the ref is unchanged");
            assert_eq!(
                outbox.committed_count(),
                expected_commits,
                "the rejection commits no witness"
            );
            assert_eq!(
                outbox.outbox_depth(),
                expected_commits,
                "the rejection stages no outbox row"
            );
            assert!(
                migration.is_empty(),
                "structural validation precedes object migration"
            );
        }

        let ref_name = "refs/heads/topic";

        let (create_store, create_outbox) = store();
        assert_rejected(
            &create_store,
            &create_outbox,
            human_push(ref_name, Oid::zero(), Oid::new("create")),
            None,
            0,
        );

        let (update_store, update_outbox) = store();
        let old = Oid::new("old-update");
        update_store
            .receive(
                &human_push(ref_name, Oid::zero(), old.clone()),
                &InMemoryObjectDb::new(),
                CrashPoint::None,
            )
            .unwrap();
        assert_rejected(
            &update_store,
            &update_outbox,
            human_push(ref_name, old.clone(), Oid::new("new-update")),
            Some(old),
            1,
        );

        let (delete_store, delete_outbox) = store();
        let old = Oid::new("old-delete");
        delete_store
            .receive(
                &human_push(ref_name, Oid::zero(), old.clone()),
                &InMemoryObjectDb::new(),
                CrashPoint::None,
            )
            .unwrap();
        assert_rejected(
            &delete_store,
            &delete_outbox,
            human_push(ref_name, old.clone(), Oid::zero()),
            Some(old),
            1,
        );
    }

    #[test]
    fn accepted_push_moves_ref_and_emits_one_event_in_one_tx() {
        let (store, outbox) = store();
        let db = InMemoryObjectDb::new();
        let push = human_push("refs/heads/feature", Oid::zero(), Oid::new("aaaa"));

        let outcome = store.receive(&push, &db, CrashPoint::None).unwrap();
        match outcome {
            PushOutcome::Accepted { moved, emitted } => {
                assert_eq!(moved.len(), 1);
                assert_eq!(moved[0].0, RefName::new("refs/heads/feature"));
                assert_eq!(moved[0].1, Oid::new("aaaa"));
                assert_eq!(moved[0].2, 1, "first move is update_seq 1");
                assert_eq!(emitted.len(), 1, "exactly one git.ref.updated emitted");
            }
            other => panic!("expected Accepted, got {other:?}"),
        }
        assert_eq!(
            store.tip(&RefName::new("refs/heads/feature")),
            Some(Oid::new("aaaa"))
        );
        assert_eq!(
            outbox.outbox_depth(),
            1,
            "one git.ref.updated row is durable + unsent"
        );
        assert_eq!(outbox.committed_count(), 1);
        assert!(db.contains(&Oid::new("cafe")));
        let id = match store
            .receive(
                &human_push("refs/heads/x", Oid::zero(), Oid::new("bb")),
                &db,
                CrashPoint::None,
            )
            .unwrap()
        {
            PushOutcome::Accepted { emitted, .. } => emitted[0].clone(),
            o => panic!("{o:?}"),
        };
        let row = outbox.row(&id).unwrap();
        assert_eq!(row.envelope.type_.0, GIT_REF_UPDATED);
        assert_eq!(
            row.aggregate,
            AggregateKey("ref:core:refs%2Fheads%2Fx".into())
        );
    }

    #[test]
    fn git_ref_event_key_round_trips_delimiter_bearing_components() {
        let key = GitRefEventKey::new(
            "repo.with:delimiter%value",
            &RefName::new("refs/heads/main"),
        )
        .unwrap();
        assert_eq!(
            key.aggregate(),
            AggregateKey("ref:repo%2Ewith%3Adelimiter%25value:refs%2Fheads%2Fmain".into())
        );
        assert_eq!(
            key.subject("acme").unwrap(),
            ArtifactRef(
                "myelin://acme/git/ref/repo%2Ewith%3Adelimiter%25value:refs%2Fheads%2Fmain".into()
            )
        );
        assert_eq!(
            GitRefEventKey::parse_id("repo%2Ewith%3Adelimiter%25value:refs%2Fheads%2Fmain")
                .unwrap(),
            (
                "repo.with:delimiter%value".into(),
                RefName::new("refs/heads/main")
            )
        );
    }

    #[test]
    fn invalid_ref_names_are_rejected_before_any_receive_effect() {
        let (store, outbox) = store();
        let db = InMemoryObjectDb::new();
        let push = human_push("HEAD", Oid::zero(), Oid::new("aaaa"));

        assert_eq!(
            store.receive(&push, &db, CrashPoint::None).unwrap(),
            PushOutcome::Rejected(RejectReason::InvalidRefName)
        );
        assert_eq!(outbox.outbox_depth(), 0);
        assert_eq!(store.tip(&RefName::new("HEAD")), None);
    }

    #[test]
    fn crash_after_policy_is_zero_ghost() {
        let (store, outbox) = store();
        let db = InMemoryObjectDb::new();
        let push = human_push("refs/heads/feature", Oid::zero(), Oid::new("aaaa"));

        let outcome = store.receive(&push, &db, CrashPoint::AfterPolicy).unwrap();
        assert_eq!(
            outcome,
            PushOutcome::Crashed(InjectedCrash {
                at: CrashPoint::AfterPolicy
            })
        );
        assert_eq!(
            outbox.outbox_depth(),
            0,
            "a crash before commit emits no event"
        );
        assert_eq!(outbox.committed_count(), 0);
        assert_eq!(
            store.tip(&RefName::new("refs/heads/feature")),
            None,
            "the ref never moved"
        );
        assert!(db.is_empty(), "the quarantine was NOT promoted");
    }

    #[test]
    fn crash_before_commit_is_zero_ghost_even_with_durable_bytes() {
        let (store, outbox) = store();
        let db = InMemoryObjectDb::new();
        let push = human_push("refs/heads/feature", Oid::zero(), Oid::new("aaaa"));

        let outcome = store.receive(&push, &db, CrashPoint::BeforeCommit).unwrap();
        assert_eq!(
            outcome,
            PushOutcome::Crashed(InjectedCrash {
                at: CrashPoint::BeforeCommit
            })
        );
        assert_eq!(outbox.outbox_depth(), 0);
        assert_eq!(outbox.committed_count(), 0);
        assert_eq!(
            store.tip(&RefName::new("refs/heads/feature")),
            None,
            "the ref never moved"
        );
        assert!(
            db.contains(&Oid::new("cafe")),
            "objects migrated before the kill (orphan, GC'd)"
        );
    }

    #[test]
    fn crash_after_commit_keeps_ref_and_event_zero_lost() {
        let (store, outbox) = store();
        let db = InMemoryObjectDb::new();
        let push = human_push("refs/heads/feature", Oid::zero(), Oid::new("aaaa"));

        let outcome = store.receive(&push, &db, CrashPoint::AfterCommit).unwrap();
        assert_eq!(
            outcome,
            PushOutcome::Crashed(InjectedCrash {
                at: CrashPoint::AfterCommit
            })
        );
        assert_eq!(
            store.tip(&RefName::new("refs/heads/feature")),
            Some(Oid::new("aaaa"))
        );
        assert_eq!(
            outbox.outbox_depth(),
            1,
            "the committed event is durable + awaiting the relay"
        );
        assert_eq!(outbox.committed_count(), 1);
    }

    #[test]
    fn force_push_on_protected_is_rejected_before_ref_moves() {
        let (store, outbox) = store();
        let db = InMemoryObjectDb::new();
        let p0 = human_push("refs/heads/main", Oid::zero(), Oid::new("old1"));
        store.receive(&p0, &db, CrashPoint::None).unwrap();
        let depth_before = outbox.outbox_depth();

        let mut p = human_push("refs/heads/main", Oid::new("old1"), Oid::new("new2"));
        p.updates[0].forced = true;
        let outcome = store.receive(&p, &db, CrashPoint::None).unwrap();
        assert_eq!(
            outcome,
            PushOutcome::Rejected(RejectReason::ForcePushOnProtected {
                ref_name: RefName::new("refs/heads/main")
            })
        );
        assert_eq!(
            store.tip(&RefName::new("refs/heads/main")),
            Some(Oid::new("old1"))
        );
        assert_eq!(
            outbox.outbox_depth(),
            depth_before,
            "a rejected push emits nothing"
        );
    }

    #[test]
    fn delete_protected_is_rejected() {
        let (store, _outbox) = store();
        let db = InMemoryObjectDb::new();
        store
            .receive(
                &human_push("refs/heads/main", Oid::zero(), Oid::new("t1")),
                &db,
                CrashPoint::None,
            )
            .unwrap();
        let del = PushSession {
            updates: vec![ProposedRefUpdate {
                ref_name: RefName::new("refs/heads/main"),
                expected_old: Oid::new("t1"),
                new_oid: Oid::zero(),
                forced: false,
                commit_oids: vec![],
            }],
            quarantine: vec![],
            pusher: Pusher {
                pseudonym: "anon-1@acme.noreply".into(),
                is_agent: false,
            },
        };
        assert_eq!(
            store.receive(&del, &db, CrashPoint::None).unwrap(),
            PushOutcome::Rejected(RejectReason::DeleteProtected {
                ref_name: RefName::new("refs/heads/main")
            })
        );
        assert_eq!(
            store.tip(&RefName::new("refs/heads/main")),
            Some(Oid::new("t1")),
            "ref not deleted"
        );
    }

    #[test]
    fn secret_in_quarantine_is_rejected_and_not_promoted() {
        let (store, outbox) = store();
        let db = InMemoryObjectDb::new();
        let push = PushSession {
            updates: vec![ProposedRefUpdate {
                ref_name: RefName::new("refs/heads/feature"),
                expected_old: Oid::zero(),
                new_oid: Oid::new("aaaa"),
                forced: false,
                commit_oids: vec![Oid::new("aaaa")],
            }],
            quarantine: vec![QuarantineObject {
                oid: Oid::new("bad"),
                bytes: [b"export AWS_KEY=AK".as_slice(), b"IAIOSFODNN7EXAMPLE"].concat(),
            }],
            pusher: Pusher {
                pseudonym: "anon-1@acme.noreply".into(),
                is_agent: false,
            },
        };
        match store.receive(&push, &db, CrashPoint::None).unwrap() {
            PushOutcome::Rejected(RejectReason::SecretDetected { oid, pattern }) => {
                assert_eq!(oid, Oid::new("bad"));
                assert_eq!(pattern, ["AK", "IA"].concat());
            }
            o => panic!("expected SecretDetected, got {o:?}"),
        }
        assert_eq!(
            store.tip(&RefName::new("refs/heads/feature")),
            None,
            "ref never moved"
        );
        assert!(
            db.is_empty(),
            "the secret object was NOT promoted out of quarantine"
        );
        assert_eq!(outbox.outbox_depth(), 0);
    }

    #[test]
    fn self_hosting_tree_contains_no_complete_default_secret_sentinel() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("myelin-git is a crate in the workspace");
        let repository = git2::Repository::discover(root).expect("workspace is a Git repository");
        let workdir = repository.workdir().expect("workspace is non-bare");
        let index = repository.index().expect("workspace index is readable");

        let patterns = PushPolicy::default().secret_patterns;
        for entry in index.iter() {
            let path = std::str::from_utf8(&entry.path).expect("tracked paths are UTF-8");
            let bytes = std::fs::read(workdir.join(path)).expect("tracked file remains readable");
            let contents = String::from_utf8_lossy(&bytes);
            for pattern in &patterns {
                assert!(
                    !contents.contains(pattern),
                    "tracked source blob `{path}` contains the default secret sentinel `{pattern}` \
                     and cannot pass Myelin's reject-before-promote wire gate"
                );
            }
        }
    }

    #[test]
    fn agent_push_to_protected_is_rejected() {
        let (store, _outbox) = store();
        let db = InMemoryObjectDb::new();
        let mut push = human_push("refs/heads/main", Oid::zero(), Oid::new("aaaa"));
        push.pusher.is_agent = true;
        assert_eq!(
            store.receive(&push, &db, CrashPoint::None).unwrap(),
            PushOutcome::Rejected(RejectReason::AgentNeedsHuman {
                ref_name: RefName::new("refs/heads/main")
            })
        );
    }

    fn push_with_commit_identity(ref_name: &str, identity_line: &str) -> PushSession {
        let commit_bytes = format!(
            "tree blake3:t\nauthor {identity_line}\ncommitter {identity_line}\n\nfeat: x\n"
        )
        .into_bytes();
        PushSession {
            updates: vec![ProposedRefUpdate {
                ref_name: RefName::new(ref_name),
                expected_old: Oid::zero(),
                new_oid: Oid::new("aaaa"),
                forced: false,
                commit_oids: vec![Oid::new("c0")],
            }],
            quarantine: vec![QuarantineObject {
                oid: Oid::new("c0"),
                bytes: commit_bytes,
            }],
            pusher: Pusher {
                pseudonym: "psn-7@acme.noreply".into(),
                is_agent: false,
            },
        }
    }

    #[test]
    fn non_pseudonymous_commit_is_rejected_before_ref_moves() {
        let (store, outbox) = store();
        let db = InMemoryObjectDb::new();
        let push = push_with_commit_identity(
            "refs/heads/feature",
            "Ada Lovelace <ada.lovelace@example.com> 1700000000 +0000",
        );
        match store.receive(&push, &db, CrashPoint::None).unwrap() {
            PushOutcome::Rejected(RejectReason::NonPseudonymousCommit { oid, identity }) => {
                assert_eq!(oid, Oid::new("c0"));
                assert_eq!(
                    identity,
                    crate::commit::NonPseudonymousIdentity::NotAPseudonym {
                        role: "author".into(),
                        offending_email: "ada.lovelace@example.com".into(),
                    }
                );
            }
            o => panic!("expected NonPseudonymousCommit, got {o:?}"),
        }
        assert_eq!(
            store.tip(&RefName::new("refs/heads/feature")),
            None,
            "the ref never moved"
        );
        assert_eq!(outbox.outbox_depth(), 0, "a rejected push emits nothing");
        assert!(
            db.is_empty(),
            "the cleartext-PII commit was NOT promoted out of quarantine"
        );
    }

    #[test]
    fn pseudonymous_commit_for_the_tenant_is_accepted() {
        let (store, outbox) = store();
        let db = InMemoryObjectDb::new();
        let push = push_with_commit_identity(
            "refs/heads/feature",
            "psn-7f3a9c@acme.noreply <psn-7f3a9c@acme.noreply> 1700000000 +0000",
        );
        assert!(matches!(
            store.receive(&push, &db, CrashPoint::None).unwrap(),
            PushOutcome::Accepted { .. }
        ));
        assert_eq!(
            store.tip(&RefName::new("refs/heads/feature")),
            Some(Oid::new("aaaa"))
        );
        assert_eq!(
            outbox.outbox_depth(),
            1,
            "the accepted push committed one git.ref.updated"
        );
        assert!(
            db.contains(&Oid::new("c0")),
            "the pseudonymous commit was promoted"
        );
    }

    #[test]
    fn wrong_tenant_pseudonym_commit_is_rejected() {
        let (store, _outbox) = store();
        let db = InMemoryObjectDb::new();
        let push = push_with_commit_identity(
            "refs/heads/feature",
            "psn-x@globex.noreply <psn-x@globex.noreply> 1700000000 +0000",
        );
        match store.receive(&push, &db, CrashPoint::None).unwrap() {
            PushOutcome::Rejected(RejectReason::NonPseudonymousCommit { identity, .. }) => {
                assert_eq!(
                    identity,
                    crate::commit::NonPseudonymousIdentity::WrongTenant {
                        role: "author".into(),
                        expected_tenant: "acme".into(),
                        found_tenant: "globex".into(),
                    }
                )
            }
            o => panic!("expected WrongTenant NonPseudonymousCommit, got {o:?}"),
        }
        assert_eq!(
            store.tip(&RefName::new("refs/heads/feature")),
            None,
            "the ref never moved"
        );
    }

    #[test]
    fn blob_object_is_not_gated_by_the_pseudonymity_rule() {
        let (store, _outbox) = store();
        let db = InMemoryObjectDb::new();
        let push = PushSession {
            updates: vec![ProposedRefUpdate {
                ref_name: RefName::new("refs/heads/feature"),
                expected_old: Oid::zero(),
                new_oid: Oid::new("aaaa"),
                forced: false,
                commit_oids: vec![],
            }],
            quarantine: vec![QuarantineObject {
                oid: Oid::new("blob0"),
                bytes: b"contact: ada@example.com for support\n".to_vec(),
            }],
            pusher: Pusher {
                pseudonym: "psn-7@acme.noreply".into(),
                is_agent: false,
            },
        };
        assert!(
            matches!(
                store.receive(&push, &db, CrashPoint::None).unwrap(),
                PushOutcome::Accepted { .. }
            ),
            "a blob is not gated by the commit pseudonymity rule"
        );
    }

    #[test]
    fn blank_pseudonym_is_rejected() {
        let (store, _outbox) = store();
        let db = InMemoryObjectDb::new();
        let mut push = human_push("refs/heads/feature", Oid::zero(), Oid::new("aaaa"));
        push.pusher.pseudonym = "   ".into();
        assert_eq!(
            store.receive(&push, &db, CrashPoint::None).unwrap(),
            PushOutcome::Rejected(RejectReason::PseudonymRequired)
        );
    }

    #[test]
    fn stale_cas_is_non_fast_forward_reject_zero_ghost() {
        let (store, outbox) = store();
        let db = InMemoryObjectDb::new();
        store
            .receive(
                &human_push("refs/heads/feature", Oid::zero(), Oid::new("v1")),
                &db,
                CrashPoint::None,
            )
            .unwrap();
        assert_eq!(outbox.committed_count(), 1);

        let stale = human_push("refs/heads/feature", Oid::zero(), Oid::new("v2"));
        match store.receive(&stale, &db, CrashPoint::None).unwrap() {
            PushOutcome::Rejected(RejectReason::NonFastForward {
                ref_name,
                expected,
                actual,
            }) => {
                assert_eq!(ref_name, RefName::new("refs/heads/feature"));
                assert_eq!(expected, Oid::zero());
                assert_eq!(actual, Oid::new("v1"));
            }
            o => panic!("expected NonFastForward, got {o:?}"),
        }
        assert_eq!(
            store.tip(&RefName::new("refs/heads/feature")),
            Some(Oid::new("v1"))
        );
        assert_eq!(
            outbox.committed_count(),
            1,
            "the rejected stale push emitted nothing"
        );
    }

    #[test]
    fn atomic_push_with_one_stale_ref_moves_neither() {
        let (store, outbox) = store();
        let db = InMemoryObjectDb::new();
        store
            .receive(
                &human_push("refs/heads/a", Oid::zero(), Oid::new("v1")),
                &db,
                CrashPoint::None,
            )
            .unwrap();
        let committed_before = outbox.committed_count();

        let atomic = PushSession {
            updates: vec![
                ProposedRefUpdate {
                    ref_name: RefName::new("refs/heads/b"),
                    expected_old: Oid::zero(),
                    new_oid: Oid::new("bbb"),
                    forced: false,
                    commit_oids: vec![Oid::new("bbb")],
                },
                ProposedRefUpdate {
                    ref_name: RefName::new("refs/heads/a"),
                    expected_old: Oid::zero(),
                    new_oid: Oid::new("aaa"),
                    forced: false,
                    commit_oids: vec![Oid::new("aaa")],
                },
            ],
            quarantine: vec![],
            pusher: Pusher {
                pseudonym: "anon-1@acme.noreply".into(),
                is_agent: false,
            },
        };
        assert!(matches!(
            store.receive(&atomic, &db, CrashPoint::None).unwrap(),
            PushOutcome::Rejected(RejectReason::NonFastForward { .. })
        ));
        assert_eq!(
            store.tip(&RefName::new("refs/heads/b")),
            None,
            "the fresh ref was NOT created"
        );
        assert_eq!(
            store.tip(&RefName::new("refs/heads/a")),
            Some(Oid::new("v1"))
        );
        assert_eq!(
            outbox.committed_count(),
            committed_before,
            "no partial emit"
        );
    }

    #[test]
    fn successive_pushes_to_one_ref_are_monotonic() {
        let (store, outbox) = store();
        let db = InMemoryObjectDb::new();
        let mut ids = Vec::new();
        for (old, new) in [
            (Oid::zero(), Oid::new("v1")),
            (Oid::new("v1"), Oid::new("v2")),
            (Oid::new("v2"), Oid::new("v3")),
        ] {
            match store
                .receive(
                    &human_push("refs/heads/feature", old, new),
                    &db,
                    CrashPoint::None,
                )
                .unwrap()
            {
                PushOutcome::Accepted { emitted, .. } => ids.push(emitted[0].clone()),
                o => panic!("{o:?}"),
            }
        }

        assert_eq!(
            store.tip(&RefName::new("refs/heads/feature")),
            Some(Oid::new("v3"))
        );
        let log = store.reflog().expect("reflog");
        assert!(matches!(
            store.reflog_bounded(2, crate::durable::REFLOG_MAX_TOTAL_BYTES),
            Err(crate::durable::DurableError::Git(message))
                if message == "audit reflog limit exceeded: total entry count"
        ));
        assert!(matches!(
            store.reflog_bounded(3, 1),
            Err(crate::durable::DurableError::Git(message))
                if message == "audit reflog limit exceeded: total bytes"
        ));
        let seqs: Vec<u64> = log
            .iter()
            .filter(|e| e.ref_name == RefName::new("refs/heads/feature"))
            .map(|e| e.update_seq)
            .collect();
        assert_eq!(seqs, vec![1, 2, 3], "update_seq is monotonic per ref");
        let agg = AggregateKey("ref:core:refs%2Fheads%2Ffeature".into());
        let mut agg_seqs: Vec<u64> = ids
            .iter()
            .map(|id| {
                let row = outbox.row(id).unwrap();
                assert_eq!(
                    row.aggregate, agg,
                    "all three rows share the per-ref aggregate"
                );
                row.seq
            })
            .collect();
        agg_seqs.sort_unstable();
        assert_eq!(
            agg_seqs,
            vec![0, 1, 2],
            "per-ref outbox ordering is gap-free"
        );
    }

    #[test]
    fn hot_ref_burst_serialises_exactly_one_winner_per_generation() {
        use std::sync::Barrier;
        let (store, outbox) = store();
        let store = Arc::new(store);
        let n = 32usize;
        let barrier = Arc::new(Barrier::new(n));

        let mut handles = Vec::new();
        for i in 0..n {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                let db = InMemoryObjectDb::new();
                let push = human_push("refs/heads/hot", Oid::zero(), Oid::new(format!("w{i:02}")));
                barrier.wait();
                store.receive(&push, &db, CrashPoint::None).unwrap()
            }));
        }
        let outcomes: Vec<PushOutcome> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        let accepted = outcomes
            .iter()
            .filter(|o| matches!(o, PushOutcome::Accepted { .. }))
            .count();
        let rejected = outcomes
            .iter()
            .filter(|o| {
                matches!(
                    o,
                    PushOutcome::Rejected(RejectReason::NonFastForward { .. })
                )
            })
            .count();
        assert_eq!(
            accepted, 1,
            "exactly one racer wins the create (per-ref linearisation)"
        );
        assert_eq!(
            rejected,
            n - 1,
            "every loser is a non-fast-forward reject (0 lost-update)"
        );
        assert_eq!(
            outbox.committed_count(),
            1,
            "only the winner's git.ref.updated committed (0 ghost)"
        );
        assert_eq!(
            store
                .reflog()
                .expect("reflog")
                .iter()
                .filter(|e| e.ref_name == RefName::new("refs/heads/hot"))
                .count(),
            1,
            "the ref advanced by exactly one generation"
        );
        let tip = store.tip(&RefName::new("refs/heads/hot")).unwrap();
        assert!(tip.0.starts_with('w'), "the tip is a racer's oid: {tip:?}");
    }

    #[test]
    fn chained_hot_ref_burst_preserves_push_order_per_ref() {
        let (store, outbox) = store();
        let db = InMemoryObjectDb::new();
        let k = 50u64;
        let mut prev = Oid::zero();
        let mut ids = Vec::new();
        for i in 1..=k {
            let new = Oid::new(format!("gen{i:03}"));
            match store
                .receive(
                    &human_push("refs/heads/hot", prev.clone(), new.clone()),
                    &db,
                    CrashPoint::None,
                )
                .unwrap()
            {
                PushOutcome::Accepted { emitted, moved } => {
                    assert_eq!(moved[0].2, i, "update_seq is the contiguous generation");
                    ids.push(emitted[0].clone());
                }
                o => panic!("a fast-forward chain push must be accepted, got {o:?}"),
            }
            prev = new;
        }
        let agg = AggregateKey("ref:core:refs%2Fheads%2Fhot".into());
        let outbox_seqs: Vec<u64> = ids
            .iter()
            .map(|id| {
                let row = outbox.row(id).unwrap();
                assert_eq!(
                    row.aggregate, agg,
                    "every burst event is on the one per-ref aggregate"
                );
                row.seq
            })
            .collect();
        assert_eq!(
            outbox_seqs,
            (0..k).collect::<Vec<_>>(),
            "outbox order == ref-update order per ref (gap-free, in push order)"
        );
    }

    #[test]
    fn distinct_refs_fan_out_parallel_all_succeed() {
        use std::sync::Barrier;
        let (store, outbox) = store();
        let store = Arc::new(store);
        let n = 24usize;
        let barrier = Arc::new(Barrier::new(n));

        let mut handles = Vec::new();
        for i in 0..n {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                let db = InMemoryObjectDb::new();
                let ref_name = format!("refs/heads/r{i:02}");
                let push = human_push(&ref_name, Oid::zero(), Oid::new(format!("t{i:02}")));
                barrier.wait();
                (
                    ref_name,
                    store.receive(&push, &db, CrashPoint::None).unwrap(),
                )
            }));
        }
        let results: Vec<(String, PushOutcome)> =
            handles.into_iter().map(|h| h.join().unwrap()).collect();

        for (ref_name, outcome) in &results {
            assert!(
                matches!(outcome, PushOutcome::Accepted { .. }),
                "distinct ref {ref_name} must commit in parallel, got {outcome:?}"
            );
        }
        assert_eq!(
            outbox.committed_count(),
            n,
            "all N distinct-ref events committed"
        );
        for i in 0..n {
            assert_eq!(
                store.tip(&RefName::new(format!("refs/heads/r{i:02}"))),
                Some(Oid::new(format!("t{i:02}"))),
                "ref r{i:02} advanced independently"
            );
        }
        for row in (0..n).filter_map(|i| {
            outbox.row(&match &results[i].1 {
                PushOutcome::Accepted { emitted, .. } => emitted[0].clone(),
                _ => unreachable!(),
            })
        }) {
            assert_eq!(
                row.seq, 0,
                "each distinct ref's first event is its own aggregate's seq 0"
            );
        }
    }

    #[test]
    fn non_protected_ref_delete_then_recreate() {
        let (store, outbox) = store();
        let db = InMemoryObjectDb::new();
        store
            .receive(
                &human_push("refs/heads/feature", Oid::zero(), Oid::new("v1")),
                &db,
                CrashPoint::None,
            )
            .unwrap();
        let del = PushSession {
            updates: vec![ProposedRefUpdate {
                ref_name: RefName::new("refs/heads/feature"),
                expected_old: Oid::new("v1"),
                new_oid: Oid::zero(),
                forced: false,
                commit_oids: vec![],
            }],
            quarantine: vec![],
            pusher: Pusher {
                pseudonym: "anon-1@acme.noreply".into(),
                is_agent: false,
            },
        };
        assert!(matches!(
            store.receive(&del, &db, CrashPoint::None).unwrap(),
            PushOutcome::Accepted { .. }
        ));
        assert_eq!(
            store.tip(&RefName::new("refs/heads/feature")),
            None,
            "the ref was deleted"
        );
        match store
            .receive(
                &human_push("refs/heads/feature", Oid::zero(), Oid::new("v2")),
                &db,
                CrashPoint::None,
            )
            .unwrap()
        {
            PushOutcome::Accepted { moved, .. } => assert_eq!(
                moved[0].2, 1,
                "the re-created row starts a fresh generation"
            ),
            o => panic!("re-create must be accepted, got {o:?}"),
        }
        assert_eq!(
            store.tip(&RefName::new("refs/heads/feature")),
            Some(Oid::new("v2"))
        );
        assert_eq!(
            outbox.committed_count(),
            3,
            "create + delete + re-create each emitted"
        );
    }

    #[test]
    fn opening_the_store_registers_holder_h1() {
        let (store, _outbox) = store();
        assert_eq!(store.holder().holder_id, crate::holder_intent::HOLDER_ID);
        assert!(
            store.holder().registered,
            "the store auto-registered as H1 on open"
        );
    }

    #[test]
    fn durable_tip_read_fault_aborts_before_outbox_commit() {
        let root = std::env::temp_dir().join(format!(
            "myelin-ref-tip-fault-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos()
        ));
        let loc = crate::core::RepoLoc::new("acme", "fr-par", "core");
        let durable = Arc::new(
            crate::durable::DurableGitStore::rooted(&root)
                .create_repo(&loc)
                .expect("create durable repo"),
        );
        let outbox = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let store = RefStore::open_durable(durable, "core", ctx_base(), outbox.clone(), minter);
        std::fs::remove_dir_all(&root).expect("inject repository disappearance");

        assert!(store.try_tip(&RefName::new("refs/heads/feature")).is_err());
        assert!(
            store.reflog().is_err(),
            "audit history faults must not become an empty log"
        );
        let result = store.receive(
            &human_push("refs/heads/feature", Oid::zero(), Oid::new("new")),
            &InMemoryObjectDb::new(),
            CrashPoint::None,
        );
        assert!(
            result.is_err(),
            "a missing durable repo is not an absent ref"
        );
        assert_eq!(
            outbox.committed_count(),
            0,
            "no event commits on an invented empty tip"
        );
    }

    #[test]
    fn durable_reflog_sequence_survives_delete_and_recreate() {
        let root = std::env::temp_dir().join(format!(
            "myelin-reflog-sequence-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos()
        ));
        let loc = crate::core::RepoLoc::new("acme", "fr-par", "core");
        let durable = Arc::new(
            crate::durable::DurableGitStore::rooted(&root)
                .create_repo(&loc)
                .expect("create durable repo"),
        );
        let blob = durable.write_blob(b"audit\n").expect("write blob");
        let tree = durable
            .write_tree(&[("README.md", &blob)])
            .expect("write tree");
        let first = durable
            .write_commit(
                &tree,
                &[],
                "first",
                "anon-1@acme.noreply",
                "anon-1@acme.noreply",
            )
            .expect("write first commit");
        let second = durable
            .write_commit(
                &tree,
                &[&first],
                "second",
                "anon-1@acme.noreply",
                "anon-1@acme.noreply",
            )
            .expect("write second commit");
        durable
            .update_ref_cas(
                "refs/heads/main",
                None,
                Some(&first),
                "create",
                "anon-1@acme.noreply",
            )
            .expect("create ref");
        durable
            .update_ref_cas(
                "refs/heads/main",
                Some(&first),
                Some(&second),
                "update",
                "anon-1@acme.noreply",
            )
            .expect("update ref");
        durable
            .update_ref_cas(
                "refs/heads/main",
                Some(&second),
                None,
                "delete",
                "anon-1@acme.noreply",
            )
            .expect("delete ref");
        durable
            .update_ref_cas(
                "refs/heads/main",
                None,
                Some(&second),
                "recreate",
                "anon-1@acme.noreply",
            )
            .expect("recreate ref");

        let store = RefStore::open_durable(
            Arc::clone(&durable),
            "core",
            ctx_base(),
            OutboxStore::new(),
            Arc::new(MonotonicMinter::new()),
        );
        let log = store.reflog().expect("read durable reflog");
        assert_eq!(
            log.len(),
            1,
            "only the recreated ref's physical history survives"
        );
        assert_eq!(
            log[0].update_seq, 4,
            "durable generation must not reset to one"
        );

        drop(store);
        drop(durable);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn independent_durable_refstores_linearize_one_ref_and_commit_one_witness() {
        let root = std::env::temp_dir().join(format!(
            "myelin-ref-cross-store-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos()
        ));
        let loc = crate::core::RepoLoc::new("acme", "fr-par", "core");
        let durable = Arc::new(
            crate::durable::DurableGitStore::rooted(&root)
                .create_repo(&loc)
                .expect("create durable repo"),
        );
        let make_commit = |content: &[u8]| {
            let blob = durable.write_blob(content).unwrap();
            let tree = durable.write_tree(&[("file.txt", &blob)]).unwrap();
            durable
                .write_commit(
                    &tree,
                    &[],
                    "create",
                    "anon-7@acme.noreply",
                    "anon-7@acme.noreply",
                )
                .unwrap()
        };
        let heads = [make_commit(b"first\n"), make_commit(b"second\n")];
        let outbox = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let stores = [
            RefStore::open_durable(
                Arc::clone(&durable),
                "core",
                ctx_base(),
                outbox.clone(),
                Arc::clone(&minter),
            ),
            RefStore::open_durable(
                Arc::clone(&durable),
                "core",
                ctx_base(),
                outbox.clone(),
                minter,
            ),
        ];
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let threads: Vec<_> = stores
            .into_iter()
            .zip(heads)
            .map(|(store, head)| {
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    store
                        .receive(
                            &human_push("refs/heads/topic", Oid::zero(), Oid::new(head.0)),
                            &InMemoryObjectDb::new(),
                            CrashPoint::None,
                        )
                        .unwrap()
                })
            })
            .collect();
        let outcomes: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();

        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(outcome, PushOutcome::Accepted { .. }))
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(
                    outcome,
                    PushOutcome::Rejected(RejectReason::NonFastForward { .. })
                ))
                .count(),
            1
        );
        assert_eq!(
            outbox.committed_count(),
            1,
            "only the winning witness commits"
        );
        assert_eq!(durable.ref_generation("refs/heads/topic"), Ok(1));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn protected_set_is_exactly_main_and_release() {
        assert!(RefName::new("refs/heads/main").is_protected());
        assert!(RefName::new("refs/heads/release/1.0").is_protected());
        assert!(
            !RefName::new("refs/heads/feature").is_protected(),
            "a feature ref is NOT protected"
        );
        assert!(
            !RefName::new("refs/heads/mainline").is_protected(),
            "only exact `main` is protected"
        );

        let (store, outbox) = store();
        let db = InMemoryObjectDb::new();
        store
            .receive(
                &human_push("refs/heads/feature", Oid::zero(), Oid::new("a1")),
                &db,
                CrashPoint::None,
            )
            .unwrap();
        let mut forced = human_push("refs/heads/feature", Oid::new("a1"), Oid::new("a2"));
        forced.updates[0].forced = true;
        assert!(
            matches!(
                store.receive(&forced, &db, CrashPoint::None).unwrap(),
                PushOutcome::Accepted { .. }
            ),
            "a force-push to a NON-protected ref is accepted"
        );
        assert_eq!(
            store.tip(&RefName::new("refs/heads/feature")),
            Some(Oid::new("a2"))
        );
        assert_eq!(outbox.committed_count(), 2);
    }

    #[test]
    fn object_size_limit_is_strict_greater_than() {
        let policy = PushPolicy {
            max_object_bytes: 8,
            secret_patterns: vec![],
            protected_needs_human: true,
            tenant: "acme".into(),
        };
        let at_limit = PushSession {
            updates: vec![ProposedRefUpdate {
                ref_name: RefName::new("refs/heads/f"),
                expected_old: Oid::zero(),
                new_oid: Oid::new("a"),
                forced: false,
                commit_oids: vec![],
            }],
            quarantine: vec![QuarantineObject {
                oid: Oid::new("x"),
                bytes: vec![0u8; 8],
            }],
            pusher: Pusher {
                pseudonym: "p@acme.noreply".into(),
                is_agent: false,
            },
        };
        assert!(
            policy.evaluate(&at_limit).is_ok(),
            "an object exactly at the limit is accepted"
        );

        let mut over = at_limit.clone();
        over.quarantine[0].bytes = vec![0u8; 9];
        match policy.evaluate(&over) {
            Err(RejectReason::ObjectTooLarge { size, limit, .. }) => {
                assert_eq!(size, 9);
                assert_eq!(limit, 8);
            }
            other => panic!("expected ObjectTooLarge, got {other:?}"),
        }
    }

    #[test]
    fn object_db_accessors_track_migrations() {
        let db = InMemoryObjectDb::new();
        assert!(db.is_empty());
        assert_eq!(db.len(), 0);
        assert!(!db.contains(&Oid::new("z")), "a fresh DB contains nothing");

        db.migrate(&[QuarantineObject {
            oid: Oid::new("z"),
            bytes: vec![],
        }])
        .unwrap();
        assert!(!db.is_empty(), "a migrated DB is not empty");
        assert_eq!(db.len(), 1);
        assert!(db.contains(&Oid::new("z")));
        assert!(
            !db.contains(&Oid::new("other")),
            "it contains only what was migrated"
        );
    }

    #[test]
    fn outbox_accessor_returns_the_shared_store() {
        let (store, _outbox) = store();
        let db = InMemoryObjectDb::new();
        assert_eq!(
            store.outbox().outbox_depth(),
            0,
            "the shared outbox starts empty"
        );
        store
            .receive(
                &human_push("refs/heads/f", Oid::zero(), Oid::new("a")),
                &db,
                CrashPoint::None,
            )
            .unwrap();
        assert_eq!(
            store.outbox().outbox_depth(),
            1,
            "the accessor sees the committed event"
        );
    }

    #[test]
    fn git_ref_migration_is_the_frozen_shape() {
        assert!(GIT_REF_MIGRATION.contains("CREATE TABLE IF NOT EXISTS git_ref"));
        assert!(GIT_REF_MIGRATION.contains("PRIMARY KEY (tenant, repo, ref_name)"));
        assert!(GIT_REF_MIGRATION.contains("update_seq"));
        assert!(GIT_REF_MIGRATION.contains("git_reflog"));
        assert!(GIT_REF_MIGRATION.contains("pusher_pseudonym"));
        assert!(!GIT_REF_MIGRATION.contains("DROP TABLE"));
    }

    #[test]
    fn receive_pack_migrates_into_the_real_pack_tier_and_clone_round_trips() {
        use crate::pack_tier::{PackObjectDb, PackTierMigration};
        use myelin_storage::{
            FsBlobStore, GitPackTier, RepoGitPlacement, RepoId, RepoPlacementStatus, StorageGroup,
        };
        use myelin_tenancy::{Region, TenantId};

        let tier = GitPackTier::new(TenantId("acme".into()), FsBlobStore::new());
        let repo = RepoId::from_token("core");
        tier.place_repo(
            repo.clone(),
            RepoGitPlacement {
                group: StorageGroup::from_token("pack-0"),
                region: Region::new("fr-par"),
                status: RepoPlacementStatus::Active,
            },
        );
        let object_db = PackObjectDb::new(tier, repo);
        let migration = PackTierMigration::new(&object_db);

        let (store, outbox) = store();
        let pushed_oid = Oid::new("cafe");
        let pushed_bytes = b"a normal commit blob".to_vec();
        let push = human_push("refs/heads/feature", Oid::zero(), Oid::new("aaaa"));

        let outcome = store.receive(&push, &migration, CrashPoint::None).unwrap();
        assert!(
            matches!(outcome, PushOutcome::Accepted { .. }),
            "the push is accepted"
        );
        assert_eq!(
            outbox.outbox_depth(),
            1,
            "one git.ref.updated committed (emit-iff-committed)"
        );

        let served = object_db
            .serve_clone(std::slice::from_ref(&pushed_oid))
            .expect("clone served");
        assert_eq!(served.len(), 1);
        assert_eq!(served[0].0, pushed_oid);
        assert_eq!(
            served[0].1, pushed_bytes,
            "the clone round-trips byte-identical to the receive-pack input (0 corruption)"
        );
    }
}
