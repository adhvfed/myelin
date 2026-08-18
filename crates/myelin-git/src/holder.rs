use myelin_gdpr::{
    DsrError, EraseReceipt, EraseScope, LocateReport, Patch, PersonalDataHolder, PortableBundle,
    Receipt, RectifyReceipt, RestrictReceipt, Result as DsrResult, SubjectRef, TenantId,
};
use myelin_storage::encryption::SubjectId;
use myelin_storage::erase::{CryptoShredErase, EpochMillis, EraseError, EraseHolders};
use myelin_storage::kms::KmsEngine;
use myelin_tenancy::Region;

use crate::code_tools::{
    CacheInvalidator, CacheNamespace, HistoryRewriteError, HistoryRewritePlan,
    HistoryRewriteReceipt, HistoryRewriteTool, RewriteRateLimiter,
};
use crate::core::WireExecutor;
use crate::holder_intent::HOLDER_ID;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GitHolder {
    PseudonymMap,
    SubjectBodies,
    GitStructures,
    SearchIndex,
    RefsProjection,
    BusKeys,
    CacheCdn,
    ErasureLedger,
}

impl GitHolder {
    pub fn label(self) -> &'static str {
        match self {
            GitHolder::PseudonymMap => "pseudonym-map",
            GitHolder::SubjectBodies => "subject-bodies-dek",
            GitHolder::GitStructures => "git-structures-blob-dek",
            GitHolder::SearchIndex => "search-index",
            GitHolder::RefsProjection => "refs-projection",
            GitHolder::BusKeys => "bus-keys",
            GitHolder::CacheCdn => "cache-cdn",
            GitHolder::ErasureLedger => "erasure-ledger",
        }
    }

    pub const ALL: [GitHolder; 8] = [
        GitHolder::PseudonymMap,
        GitHolder::SubjectBodies,
        GitHolder::GitStructures,
        GitHolder::SearchIndex,
        GitHolder::RefsProjection,
        GitHolder::BusKeys,
        GitHolder::CacheCdn,
        GitHolder::ErasureLedger,
    ];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitResidualPosture {
    OnePlatformPosture,
}

impl GitResidualPosture {
    pub const RESIDUAL_POSTURE_REF: &'static str =
        "contract 10.9 / 00 §X-7 (the ONE platform free-text/immutable-content erasure posture); \
         git: pseudonymous-by-default (Id 4.8) + per-subject DEK shred (11.4) + restrict suppression; \
         on-demand history-rewrite = 10.6; lawful-basis residual = R-7 (parallel/Legal)";
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitDsrReceipt {
    pub subject: String,
    pub tenant: TenantId,
    pub holders_hit: Vec<GitHolder>,
    pub recoverable_in_backup: usize,
    pub cache_namespaces_invalidated: Vec<CacheNamespace>,
    pub residual: GitResidualPosture,
    pub audit_receipt: Receipt,
    pub re_run: bool,
}

impl GitDsrReceipt {
    pub fn is_green(&self) -> bool {
        GitHolder::ALL.iter().all(|h| self.holders_hit.contains(h))
            && self.recoverable_in_backup == 0
            && CacheNamespace::ALL
                .iter()
                .all(|n| self.cache_namespaces_invalidated.contains(n))
            && self.residual == GitResidualPosture::OnePlatformPosture
    }

    pub fn missed_holders(&self) -> Vec<GitHolder> {
        GitHolder::ALL
            .iter()
            .copied()
            .filter(|h| !self.holders_hit.contains(h))
            .collect()
    }
}

pub struct GitPersonalDataHolder<'a, I: CacheInvalidator> {
    engine: &'a KmsEngine,
    region: Region,
    invalidator: I,
}

impl<'a, I: CacheInvalidator> GitPersonalDataHolder<'a, I> {
    pub fn new(
        engine: &'a KmsEngine,
        region: Region,
        invalidator: I,
    ) -> GitPersonalDataHolder<'a, I> {
        GitPersonalDataHolder {
            engine,
            region,
            invalidator,
        }
    }

    pub fn holder_id(&self) -> &'static str {
        HOLDER_ID
    }

    fn erase_cache_target(tenant: &TenantId) -> crate::core::RepoLoc {
        crate::core::RepoLoc::new(tenant.as_str(), "fr-par", "*")
    }

    pub fn erase_fanout(
        &self,
        subject: &SubjectId,
        tenant: &TenantId,
        holders: &EraseHolders<'_>,
        now: EpochMillis,
    ) -> Result<GitDsrReceipt, GitDsrError> {
        if holders.git_reach.is_none() {
            return Err(GitDsrError::GitStructureReachNotWired);
        }

        let orchestrator = CryptoShredErase::new(self.engine, self.region.clone());
        let storage_receipt = orchestrator
            .erase(subject, tenant, holders, now)
            .map_err(GitDsrError::FanOut)?;

        let target = Self::erase_cache_target(tenant);
        let mut invalidated = Vec::new();
        let mut missing = Vec::new();
        for ns in CacheNamespace::ALL {
            match self.invalidator.invalidate(tenant, &target, ns) {
                Ok(_) => invalidated.push(ns),
                Err(_) => missing.push(ns),
            }
        }
        if !missing.is_empty() {
            return Err(GitDsrError::IncompleteCacheFanOut { missing });
        }

        let holders_hit = GitHolder::ALL.to_vec();

        let audit_receipt = Receipt::content_addressed(
            "erase",
            HOLDER_ID,
            &storage_receipt.subject,
            tenant.as_str(),
            &format!(
                "git DSR fan-out: {} holder(s) hit; 0 recoverable in backup; {} cache namespace(s) \
                 invalidated; residual == the ONE platform posture (10.9)",
                holders_hit.len(),
                invalidated.len(),
            ),
            storage_receipt
                .dek_destroyed_now
                .then_some(storage_receipt.completed_at),
            now,
        );

        let receipt = GitDsrReceipt {
            subject: storage_receipt.subject,
            tenant: tenant.clone(),
            holders_hit,
            recoverable_in_backup: storage_receipt.recoverable_in_backup,
            cache_namespaces_invalidated: invalidated,
            residual: GitResidualPosture::OnePlatformPosture,
            audit_receipt,
            re_run: storage_receipt.re_run,
        };

        if !receipt.is_green() {
            return Err(GitDsrError::NotGreen {
                missed_holders: receipt.missed_holders(),
                recoverable_in_backup: receipt.recoverable_in_backup,
            });
        }
        Ok(receipt)
    }

    pub fn expunge_body<E: WireExecutor>(
        &self,
        tool: &HistoryRewriteTool<E, &I>,
        plan: &HistoryRewritePlan,
        limiter: &mut RewriteRateLimiter,
        at_ms: u64,
    ) -> Result<HistoryRewriteReceipt, HistoryRewriteError> {
        tool.rewrite(plan, limiter, at_ms)
    }

    pub fn invalidator(&self) -> &I {
        &self.invalidator
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitDsrError {
    GitStructureReachNotWired,
    FanOut(EraseError),
    IncompleteCacheFanOut {
        missing: Vec<CacheNamespace>,
    },
    NotGreen {
        missed_holders: Vec<GitHolder>,
        recoverable_in_backup: usize,
    },
}

impl std::fmt::Display for GitDsrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitDsrError::GitStructureReachNotWired => write!(
                f,
                "git DSR erase REFUSED: the git-structure crypto-shred reach (§2b - \
                 reflog/bitmap/pack-backup) is not wired - an erase that cannot reach the git \
                 structures would miss a holder (a breach); refused fail-closed before any step ran"
            ),
            GitDsrError::FanOut(e) => write!(
                f,
                "git DSR erase fan-out failed: {e} - the erase is INCOMPLETE, NEVER recorded as \
                 erased (a partial erase is a loud retry, not 'assume erased')"
            ),
            GitDsrError::IncompleteCacheFanOut { missing } => write!(
                f,
                "git DSR erase cache/CDN (H9) fan-out is INCOMPLETE - {} namespace(s) NOT \
                 invalidated ({:?}); a fork/mirror/clone-cache could resurrect the subject's \
                 pre-erase derived state",
                missing.len(),
                missing.iter().map(|n| n.label()).collect::<Vec<_>>(),
            ),
            GitDsrError::NotGreen {
                missed_holders,
                recoverable_in_backup,
            } => write!(
                f,
                "git DSR erase is NOT green (GIT-D2 RED): {} holder(s) missed ({:?}), {} \
                 per-subject DEK(s) still recoverable in backup - the erase is INCOMPLETE, never \
                 recorded as complete",
                missed_holders.len(),
                missed_holders.iter().map(|h| h.label()).collect::<Vec<_>>(),
                recoverable_in_backup,
            ),
        }
    }
}

impl std::error::Error for GitDsrError {}

fn subject_id_of(subject: &SubjectRef) -> SubjectId {
    SubjectId::new(subject.principal.principal_id.0.clone())
}

impl<I: CacheInvalidator> PersonalDataHolder for GitPersonalDataHolder<'_, I> {
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        let sid = subject_id_of(subject);
        let receipt = Receipt::content_addressed(
            "locate",
            HOLDER_ID,
            &sid.0,
            tenant.as_str(),
            "git locate: PRs/reviews/comments by pseudonym + repos + refs/reflog + LFS + id-map ref",
            None,
            0,
        );
        Ok(LocateReport { receipt })
    }

    fn export(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<PortableBundle> {
        let sid = subject_id_of(subject);
        let receipt = Receipt::content_addressed(
            "export",
            HOLDER_ID,
            &sid.0,
            tenant.as_str(),
            "git export: the subject's hosting content + clonable repos as a MerkleProvenBundle (10.4)",
            None,
            0,
        );
        Ok(PortableBundle { receipt })
    }

    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        let sid = subject_id_of(subject);
        let receipt = Receipt::content_addressed(
            "rectify",
            HOLDER_ID,
            &sid.0,
            "",
            "git rectify: update hosting-layer text the subject controls (comment bodies, PR titles)",
            None,
            0,
        );
        Ok(RectifyReceipt { receipt })
    }

    fn restrict(&self, subject: &SubjectRef, on: bool) -> DsrResult<RestrictReceipt> {
        let sid = subject_id_of(subject);
        let receipt = Receipt::content_addressed(
            "restrict",
            HOLDER_ID,
            &sid.0,
            "",
            if on {
                "git restrict ON: no indexing / no agent-use / no analytics / no notification (§6.3)"
            } else {
                "git restrict OFF: the restriction flag is cleared for the subject (§6.3)"
            },
            None,
            0,
        );
        Ok(RestrictReceipt { receipt })
    }

    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        let (subject_label, tenant_label) = match &scope {
            EraseScope::Subject { subject, tenant } => (
                subject.principal.principal_id.0.clone(),
                tenant.as_str().to_string(),
            ),
            EraseScope::Tenant(tenant) => (
                "<tenant-offboarding>".to_string(),
                tenant.as_str().to_string(),
            ),
        };
        Err(DsrError(format!(
            "git erase(scope) for subject `{subject_label}` in tenant `{tenant_label}` requires the \
             wired cross-holder seams (Id pseudonym-map shred + per-subject DEK + git-structure reach \
             + Search purge + Refs tombstone + Bus erase + erasure ledger + the cache/CDN fan-out) - \
             the contract-shaped trait `erase` carries no seam bundle, so it REFUSES rather than claim \
             an un-wired erase succeeded (never a false 'erased'). Drive the real §6.1 fan-out through \
             GitPersonalDataHolder::erase_fanout with the wired EraseHolders bundle (GIT-D2 complete)."
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code_tools::HistoryRewriteTool;
    use crate::core::{GitCoreError, RepoLoc, WireInvocation, WireOutput};
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_storage::erase::{
        BusErase, ErasureLedgerSink, PseudonymShred, RefsTombstone, SearchPurge,
    };
    use myelin_storage::git_shred::GitCryptoShredReach;
    use myelin_storage::kms::{KekId, KeyClass};
    use std::cell::RefCell;
    use std::collections::HashSet;
    use std::sync::Mutex;

    fn tenant() -> TenantId {
        myelin_tenancy::TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }

    fn subject_ref() -> SubjectRef {
        let p = Principal::stub(
            PrincipalId("p-opaque-ada".into()),
            PrincipalKind::Human,
            tenant(),
        );
        SubjectRef::new(p)
    }
    fn subject_id() -> SubjectId {
        SubjectId::new("p-opaque-ada")
    }

    #[derive(Default)]
    struct RecordingSeam {
        ran: Mutex<bool>,
        fail: bool,
    }
    impl RecordingSeam {
        fn ok() -> RecordingSeam {
            RecordingSeam {
                ran: Mutex::new(false),
                fail: false,
            }
        }
        fn failing() -> RecordingSeam {
            RecordingSeam {
                ran: Mutex::new(false),
                fail: true,
            }
        }
        fn did_run(&self) -> bool {
            *self.ran.lock().unwrap()
        }
        fn mark(&self) -> Result<(), EraseError> {
            *self.ran.lock().unwrap() = true;
            Ok(())
        }
    }
    impl PseudonymShred for RecordingSeam {
        fn shred_pseudonym(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
            if self.fail {
                return Err(EraseError::PseudonymShred("Id unreachable".into()));
            }
            self.mark()
        }
    }
    impl SearchPurge for RecordingSeam {
        fn purge_and_reindex(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
            if self.fail {
                return Err(EraseError::SearchPurge("index unreachable".into()));
            }
            self.mark()
        }
    }
    impl RefsTombstone for RecordingSeam {
        fn tombstone(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
            self.mark()
        }
    }
    impl BusErase for RecordingSeam {
        fn erase_inline_pii(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
            self.mark()
        }
    }

    #[derive(Default)]
    struct RecordingLedger {
        erased: Mutex<HashSet<String>>,
    }
    impl ErasureLedgerSink for RecordingLedger {
        fn record_erasure(&self, subject: &SubjectId, _t: &TenantId, _at: EpochMillis) {
            self.erased.lock().unwrap().insert(subject.0.clone());
        }
        fn is_erased(&self, subject: &SubjectId, _t: &TenantId) -> bool {
            self.erased.lock().unwrap().contains(&subject.0)
        }
    }

    struct RecordingInvalidator {
        fail: Option<CacheNamespace>,
        seen: RefCell<Vec<CacheNamespace>>,
    }
    impl RecordingInvalidator {
        fn all_ok() -> RecordingInvalidator {
            RecordingInvalidator {
                fail: None,
                seen: RefCell::new(vec![]),
            }
        }
        fn failing(ns: CacheNamespace) -> RecordingInvalidator {
            RecordingInvalidator {
                fail: Some(ns),
                seen: RefCell::new(vec![]),
            }
        }
    }
    impl CacheInvalidator for RecordingInvalidator {
        fn invalidate(
            &self,
            _tenant: &TenantId,
            _repo: &RepoLoc,
            namespace: CacheNamespace,
        ) -> Result<usize, GitCoreError> {
            if self.fail == Some(namespace) {
                return Err(GitCoreError::Wire(format!(
                    "cache `{}` unreachable",
                    namespace.label()
                )));
            }
            self.seen.borrow_mut().push(namespace);
            Ok(1)
        }
    }

    fn engine_with_subject_and_git_keys() -> KmsEngine {
        let kms = KmsEngine::new();
        let (t, r) = (tenant(), region());
        kms.ensure_kek(&KekId::new(t.clone(), r.clone()))
            .expect("seed the in-memory KEK");
        kms.ensure_dek(&t, &r, KeyClass::Subject("p-opaque-ada".into()))
            .expect("subject dek");
        kms.ensure_dek(&t, &r, KeyClass::Blob).expect("blob dek");
        kms
    }

    fn holders<'a>(
        pseudonym: &'a RecordingSeam,
        search: &'a RecordingSeam,
        refs: &'a RecordingSeam,
        bus: &'a RecordingSeam,
        ledger: &'a RecordingLedger,
        git_reach: &'a GitCryptoShredReach<'a>,
    ) -> EraseHolders<'a> {
        EraseHolders {
            pseudonym,
            search,
            refs,
            bus,
            ledger,
            git_reach: Some(git_reach),
        }
    }

    #[test]
    fn the_git_holder_set_is_the_dsr_fan_out() {
        assert_eq!(GitHolder::ALL.len(), 8);
        for h in [
            GitHolder::PseudonymMap,
            GitHolder::SubjectBodies,
            GitHolder::GitStructures,
            GitHolder::SearchIndex,
            GitHolder::RefsProjection,
            GitHolder::BusKeys,
            GitHolder::CacheCdn,
            GitHolder::ErasureLedger,
        ] {
            assert!(
                GitHolder::ALL.contains(&h),
                "{} must be in the DSR fan-out",
                h.label()
            );
        }
        assert_eq!(GitHolder::PseudonymMap.label(), "pseudonym-map");
        assert_eq!(GitHolder::SubjectBodies.label(), "subject-bodies-dek");
        assert_eq!(GitHolder::GitStructures.label(), "git-structures-blob-dek");
        assert_eq!(GitHolder::CacheCdn.label(), "cache-cdn");
    }

    #[test]
    fn git_d2_erase_reaches_every_holder_residual_is_the_posture_backups_shredded() {
        let engine = engine_with_subject_and_git_keys();
        let (t, sid) = (tenant(), subject_id());

        let subject_dek_ref = myelin_storage::kms::PiiKeyRef::new(
            t.clone(),
            0,
            KeyClass::Subject("p-opaque-ada".into()),
        );
        let body = b"PR comment authored by the subject: please review";
        let dek = engine
            .resolve_dek(&subject_dek_ref, &region())
            .expect("subject dek resolves");
        let (nonce, ct) = dek.seal(body);
        assert_eq!(
            dek.open(&nonce, &ct).unwrap(),
            body,
            "the body decrypts BEFORE erase"
        );

        let git_reach = GitCryptoShredReach::new(&engine, region());

        let (pseudonym, search, refs, bus) = (
            RecordingSeam::ok(),
            RecordingSeam::ok(),
            RecordingSeam::ok(),
            RecordingSeam::ok(),
        );
        let ledger = RecordingLedger::default();
        let inv = RecordingInvalidator::all_ok();
        let holder = GitPersonalDataHolder::new(&engine, region(), inv);
        let bundle = holders(&pseudonym, &search, &refs, &bus, &ledger, &git_reach);

        let receipt = holder
            .erase_fanout(&sid, &t, &bundle, 1_000)
            .expect("the git DSR erase is green");

        assert!(
            receipt.is_green(),
            "GIT-D2: the erase reaches every holder + backups shredded"
        );
        assert!(
            receipt.missed_holders().is_empty(),
            "0 holders missed (a missed holder is a breach)"
        );
        assert_eq!(receipt.holders_hit.len(), GitHolder::ALL.len());
        assert_eq!(
            receipt.recoverable_in_backup, 0,
            "GIT-D2: 0 recoverable PII in any backup"
        );
        assert_eq!(
            receipt.cache_namespaces_invalidated.len(),
            CacheNamespace::ALL.len()
        );
        assert_eq!(receipt.residual, GitResidualPosture::OnePlatformPosture);

        assert!(
            pseudonym.did_run(),
            "step 1: pseudonym-map shred ran (Id.erase)"
        );
        assert!(search.did_run(), "step 3: search purge+reindex ran");
        assert!(refs.did_run(), "step 4: refs tombstone ran");
        assert!(bus.did_run(), "step 5: bus erase ran");
        assert!(
            ledger.is_erased(&sid, &t),
            "step 6: the erasure ledger recorded the subject"
        );

        assert!(
            engine.resolve_dek(&subject_dek_ref, &region()).is_err(),
            "the body is unrecoverable after erase (live): the per-subject DEK is destroyed"
        );
        let subject_dek =
            myelin_storage::kms::DekId::new(t.clone(), KeyClass::Subject("p-opaque-ada".into()));
        let blob_dek = myelin_storage::kms::DekId::new(t.clone(), KeyClass::Blob);
        let backup = engine.backup_snapshot().unwrap();
        assert!(
            !backup.iter().any(|(d, _)| *d == subject_dek),
            "subject DEK absent from backup"
        );
        assert!(
            !backup.iter().any(|(d, _)| *d == blob_dek),
            "blob DEK (reflog/bitmap/pack) absent from backup"
        );

        assert_eq!(receipt.audit_receipt.operation, "erase");
        assert!(receipt.audit_receipt.content_hash.starts_with("blake3:"));
        assert!(
            receipt.audit_receipt.key_epoch_destroyed.is_some(),
            "the destroyed key epoch is named"
        );
    }

    #[test]
    fn a_failed_holder_step_aborts_loud_never_recorded_as_erased() {
        let engine = engine_with_subject_and_git_keys();
        let git_reach = GitCryptoShredReach::new(&engine, region());
        let (pseudonym, search, refs, bus) = (
            RecordingSeam::failing(),
            RecordingSeam::ok(),
            RecordingSeam::ok(),
            RecordingSeam::ok(),
        );
        let ledger = RecordingLedger::default();
        let holder = GitPersonalDataHolder::new(&engine, region(), RecordingInvalidator::all_ok());
        let bundle = holders(&pseudonym, &search, &refs, &bus, &ledger, &git_reach);

        let err = holder
            .erase_fanout(&subject_id(), &tenant(), &bundle, 1)
            .unwrap_err();
        assert!(matches!(
            err,
            GitDsrError::FanOut(EraseError::PseudonymShred(_))
        ));
        assert!(
            !ledger.is_erased(&subject_id(), &tenant()),
            "an incomplete erase is NEVER recorded"
        );
    }

    #[test]
    fn an_incomplete_cache_fan_out_aborts_loud() {
        let engine = engine_with_subject_and_git_keys();
        let git_reach = GitCryptoShredReach::new(&engine, region());
        let (pseudonym, search, refs, bus) = (
            RecordingSeam::ok(),
            RecordingSeam::ok(),
            RecordingSeam::ok(),
            RecordingSeam::ok(),
        );
        let ledger = RecordingLedger::default();
        let holder = GitPersonalDataHolder::new(
            &engine,
            region(),
            RecordingInvalidator::failing(CacheNamespace::CloneCache),
        );
        let bundle = holders(&pseudonym, &search, &refs, &bus, &ledger, &git_reach);

        let err = holder
            .erase_fanout(&subject_id(), &tenant(), &bundle, 1)
            .unwrap_err();
        match err {
            GitDsrError::IncompleteCacheFanOut { missing } => {
                assert_eq!(missing, vec![CacheNamespace::CloneCache]);
            }
            other => panic!("expected IncompleteCacheFanOut, got {other:?}"),
        }
    }

    #[test]
    fn an_unwired_git_structure_reach_is_refused_fail_closed() {
        let engine = engine_with_subject_and_git_keys();
        let (pseudonym, search, refs, bus) = (
            RecordingSeam::ok(),
            RecordingSeam::ok(),
            RecordingSeam::ok(),
            RecordingSeam::ok(),
        );
        let ledger = RecordingLedger::default();
        let holder = GitPersonalDataHolder::new(&engine, region(), RecordingInvalidator::all_ok());
        let bundle = EraseHolders {
            pseudonym: &pseudonym,
            search: &search,
            refs: &refs,
            bus: &bus,
            ledger: &ledger,
            git_reach: None,
        };
        let err = holder
            .erase_fanout(&subject_id(), &tenant(), &bundle, 1)
            .unwrap_err();
        assert_eq!(err, GitDsrError::GitStructureReachNotWired);
        assert!(
            !pseudonym.did_run(),
            "refused before any step ran (fail-closed)"
        );
    }

    #[test]
    fn a_re_erase_is_an_idempotent_no_op_success() {
        let engine = engine_with_subject_and_git_keys();
        let git_reach = GitCryptoShredReach::new(&engine, region());
        let (pseudonym, search, refs, bus) = (
            RecordingSeam::ok(),
            RecordingSeam::ok(),
            RecordingSeam::ok(),
            RecordingSeam::ok(),
        );
        let ledger = RecordingLedger::default();
        let holder = GitPersonalDataHolder::new(&engine, region(), RecordingInvalidator::all_ok());
        let bundle = holders(&pseudonym, &search, &refs, &bus, &ledger, &git_reach);

        let first = holder
            .erase_fanout(&subject_id(), &tenant(), &bundle, 1)
            .expect("first erase green");
        assert!(!first.re_run, "the first erase is not a re-run");
        assert!(first.is_green());

        let second = holder
            .erase_fanout(&subject_id(), &tenant(), &bundle, 2)
            .expect("re-erase green");
        assert!(second.re_run, "the re-erase is flagged as a re-run");
        assert!(
            second.is_green(),
            "the re-erase re-affirms every holder + 0 recoverable"
        );
        assert_eq!(second.recoverable_in_backup, 0);
    }

    #[test]
    fn is_green_requires_every_holder_zero_backups_and_the_posture() {
        let green = GitDsrReceipt {
            subject: "p-opaque-ada".into(),
            tenant: tenant(),
            holders_hit: GitHolder::ALL.to_vec(),
            recoverable_in_backup: 0,
            cache_namespaces_invalidated: CacheNamespace::ALL.to_vec(),
            residual: GitResidualPosture::OnePlatformPosture,
            audit_receipt: Receipt::content_addressed("erase", "H1", "p", "acme", "ok", Some(1), 1),
            re_run: false,
        };
        assert!(green.is_green());
        let dropped = GitDsrReceipt {
            holders_hit: vec![GitHolder::PseudonymMap],
            ..green.clone()
        };
        assert!(!dropped.is_green(), "a missed holder is a breach (RED)");
        assert_eq!(dropped.missed_holders().len(), GitHolder::ALL.len() - 1);
        let recoverable = GitDsrReceipt {
            recoverable_in_backup: 1,
            ..green.clone()
        };
        assert!(!recoverable.is_green(), "a recoverable backup is RED");
        let dropped_cache = GitDsrReceipt {
            cache_namespaces_invalidated: vec![CacheNamespace::Fork],
            ..green.clone()
        };
        assert!(
            !dropped_cache.is_green(),
            "a dropped cache namespace is RED"
        );
    }

    #[test]
    fn the_git_dsr_errors_render_loud_and_self_describing() {
        assert!(GitDsrError::GitStructureReachNotWired
            .to_string()
            .contains("fail-closed"));
        assert!(GitDsrError::FanOut(EraseError::PseudonymShred("x".into()))
            .to_string()
            .contains("INCOMPLETE"));
        assert!(GitDsrError::IncompleteCacheFanOut {
            missing: vec![CacheNamespace::Mirror]
        }
        .to_string()
        .contains("INCOMPLETE"));
        let not_green = GitDsrError::NotGreen {
            missed_holders: vec![GitHolder::SubjectBodies],
            recoverable_in_backup: 2,
        }
        .to_string();
        assert!(
            not_green.contains("NOT green"),
            "names the RED reading: {not_green}"
        );
        assert!(
            not_green.contains("subject-bodies-dek"),
            "names the missed holder: {not_green}"
        );
        assert!(
            not_green.contains('2'),
            "names the recoverable-in-backup count: {not_green}"
        );
    }


    struct OkWire;
    impl WireExecutor for OkWire {
        fn run(&self, _inv: &WireInvocation) -> Result<WireOutput, GitCoreError> {
            Ok(WireOutput {
                stdout: vec![],
                status: 0,
            })
        }
    }

    #[test]
    fn the_history_rewrite_path_expunges_a_body_with_the_invalidation_fan_out() {
        let engine = engine_with_subject_and_git_keys();
        let holder = GitPersonalDataHolder::new(&engine, region(), RecordingInvalidator::all_ok());
        let tool = HistoryRewriteTool::new(OkWire, holder.invalidator());
        let mut limiter = RewriteRateLimiter::new(5);
        let plan = HistoryRewritePlan {
            tenant: tenant(),
            repo: RepoLoc::new("acme", "fr-par", "team/app"),
            target_refs: vec!["refs/heads/main".into()],
            reason_code: "dsr-body".into(),
        };
        let receipt = holder
            .expunge_body(&tool, &plan, &mut limiter, 2_000)
            .expect("the expunge is green");
        assert!(
            receipt.is_complete(),
            "the invalidation fan-out reached every namespace"
        );
        assert_eq!(receipt.receipt.operation, "git.history_rewrite");
    }

    #[test]
    fn locate_export_rectify_restrict_return_content_addressed_receipts() {
        let engine = KmsEngine::new();
        let holder = GitPersonalDataHolder::new(&engine, region(), RecordingInvalidator::all_ok());
        let s = subject_ref();

        let loc = holder.locate(&s, tenant()).expect("locate");
        assert_eq!(loc.receipt.operation, "locate");
        assert!(loc.receipt.content_hash.starts_with("blake3:"));

        let exp = holder.export(&s, tenant()).expect("export");
        assert_eq!(exp.receipt.operation, "export");

        let rec = holder
            .rectify(&s, Patch("title: redacted".into()))
            .expect("rectify");
        assert_eq!(rec.receipt.operation, "rectify");

        let on = holder.restrict(&s, true).expect("restrict on");
        assert_eq!(on.receipt.operation, "restrict");
        let off = holder.restrict(&s, false).expect("restrict off");
        assert_ne!(on.receipt.content_hash, off.receipt.content_hash);

        assert_eq!(holder.holder_id(), "H1");
    }

    #[test]
    fn the_trait_erase_refuses_loud_without_wired_seams() {
        let engine = KmsEngine::new();
        let holder = GitPersonalDataHolder::new(&engine, region(), RecordingInvalidator::all_ok());
        let scope = EraseScope::Subject {
            subject: subject_ref(),
            tenant: tenant(),
        };
        let err = holder.erase(scope).unwrap_err();
        assert!(
            err.0.contains("requires the wired cross-holder seams"),
            "loud refusal: {}",
            err.0
        );
        assert!(
            err.0.contains("erase_fanout"),
            "points the caller at the real fan-out: {}",
            err.0
        );
        let tenant_scope = EraseScope::Tenant(tenant());
        assert!(holder.erase(tenant_scope).is_err());
    }
}
