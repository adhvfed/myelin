use crate::core::{GitCoreError, RepoLoc, WireExecutor, WireInvocation, WireOutput};
use myelin_gdpr::Receipt;
use myelin_tenancy::TenantId;

pub const GIT_SUBSYSTEM: &str = "git";

pub const HISTORY_REWRITE_TOOL: &str = "history_rewrite";

pub const SCIP_INDEX_TOOL: &str = "scip_index";

pub const GIT_CODE_TOOL_VERSION: u32 = 1;

pub fn history_rewrite_required_caps() -> Vec<String> {
    vec![format!(
        "{}.administer",
        crate::rebac_fragment::object_types::REPO
    )]
}

pub fn scip_index_required_caps() -> Vec<String> {
    vec![format!(
        "{}.pull",
        crate::rebac_fragment::object_types::REPO
    )]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CacheNamespace {
    Fork,
    Mirror,
    CloneCache,
    ReadProjection,
}

impl CacheNamespace {
    pub fn label(self) -> &'static str {
        match self {
            CacheNamespace::Fork => "fork",
            CacheNamespace::Mirror => "mirror",
            CacheNamespace::CloneCache => "clone-cache",
            CacheNamespace::ReadProjection => "read-projection",
        }
    }

    pub const ALL: [CacheNamespace; 4] = [
        CacheNamespace::Fork,
        CacheNamespace::Mirror,
        CacheNamespace::CloneCache,
        CacheNamespace::ReadProjection,
    ];
}

pub trait CacheInvalidator {
    fn invalidate(
        &self,
        tenant: &TenantId,
        repo: &RepoLoc,
        namespace: CacheNamespace,
    ) -> Result<usize, GitCoreError>;
}

impl<T: CacheInvalidator + ?Sized> CacheInvalidator for &T {
    fn invalidate(
        &self,
        tenant: &TenantId,
        repo: &RepoLoc,
        namespace: CacheNamespace,
    ) -> Result<usize, GitCoreError> {
        (**self).invalidate(tenant, repo, namespace)
    }
}

#[derive(Clone, Debug)]
pub struct RewriteRateLimiter {
    max_per_window: u32,
    consumed: std::collections::HashMap<String, u32>,
}

impl RewriteRateLimiter {
    pub fn new(max_per_window: u32) -> RewriteRateLimiter {
        RewriteRateLimiter {
            max_per_window,
            consumed: std::collections::HashMap::new(),
        }
    }

    pub fn try_consume(&mut self, tenant: &TenantId) -> Option<u32> {
        let used = self
            .consumed
            .entry(tenant.as_str().to_string())
            .or_insert(0);
        if *used >= self.max_per_window {
            return None;
        }
        *used += 1;
        Some(self.max_per_window - *used)
    }

    pub fn consumed_by(&self, tenant: &TenantId) -> u32 {
        self.consumed.get(tenant.as_str()).copied().unwrap_or(0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryRewritePlan {
    pub tenant: TenantId,
    pub repo: RepoLoc,
    pub target_refs: Vec<String>,
    pub reason_code: String,
}

impl HistoryRewritePlan {
    fn rewrite_argv(&self) -> Vec<String> {
        let mut argv = vec!["filter-repo".to_string(), "--force".to_string()];
        for r in &self.target_refs {
            argv.push("--refs".to_string());
            argv.push(r.clone());
        }
        argv
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HistoryRewriteError {
    EmptyPlan,
    RateLimited {
        tenant: String,
    },
    SandboxFailed(GitCoreError),
    IncompleteFanOut {
        missing: Vec<CacheNamespace>,
    },
}

impl std::fmt::Display for HistoryRewriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HistoryRewriteError::EmptyPlan => write!(
                f,
                "history-rewrite plan targets no refs - a rewrite that touches nothing is rejected \
                 (the audited op must not run an empty rewrite)"
            ),
            HistoryRewriteError::RateLimited { tenant } => write!(
                f,
                "history-rewrite REFUSED for tenant `{tenant}`: per-tenant rate limit exhausted \
                 (recon §9 - the rewrite is a rate-limited tenant op; it did NOT run, retry after \
                 the window)"
            ),
            HistoryRewriteError::SandboxFailed(e) => write!(
                f,
                "history-rewrite sandbox invocation failed: {e} - the op is ABORTED, no \
                 cache-invalidation fan-out ran (the caches still point at valid pre-rewrite bytes)"
            ),
            HistoryRewriteError::IncompleteFanOut { missing } => write!(
                f,
                "history-rewrite cache-invalidation fan-out is INCOMPLETE - {} trust-scoped \
                 namespace(s) NOT invalidated ({:?}); a fork/mirror/clone-cache could resurrect the \
                 expunged bytes (recon §9 fan-out)",
                missing.len(),
                missing.iter().map(|n| n.label()).collect::<Vec<_>>(),
            ),
        }
    }
}

impl std::error::Error for HistoryRewriteError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryRewriteReceipt {
    pub receipt: Receipt,
    pub namespaces_invalidated: Vec<CacheNamespace>,
    pub entries_invalidated: usize,
}

impl HistoryRewriteReceipt {
    pub fn is_complete(&self) -> bool {
        CacheNamespace::ALL
            .iter()
            .all(|n| self.namespaces_invalidated.contains(n))
    }
}

pub struct HistoryRewriteTool<E: WireExecutor, I: CacheInvalidator> {
    wire: E,
    invalidator: I,
}

impl<E: WireExecutor, I: CacheInvalidator> HistoryRewriteTool<E, I> {
    pub fn new(wire: E, invalidator: I) -> HistoryRewriteTool<E, I> {
        HistoryRewriteTool { wire, invalidator }
    }

    pub fn rewrite(
        &self,
        plan: &HistoryRewritePlan,
        limiter: &mut RewriteRateLimiter,
        at_ms: u64,
    ) -> Result<HistoryRewriteReceipt, HistoryRewriteError> {
        if plan.target_refs.is_empty() {
            return Err(HistoryRewriteError::EmptyPlan);
        }

        if limiter.try_consume(&plan.tenant).is_none() {
            return Err(HistoryRewriteError::RateLimited {
                tenant: plan.tenant.as_str().to_string(),
            });
        }

        let inv = WireInvocation {
            repo: plan.repo.clone(),
            argv: plan.rewrite_argv(),
            stdin: Vec::new(),
        };
        let out: WireOutput = self
            .wire
            .run(&inv)
            .map_err(HistoryRewriteError::SandboxFailed)?;
        if out.status != 0 {
            return Err(HistoryRewriteError::SandboxFailed(GitCoreError::Wire(
                format!("history-rewrite exited non-zero ({})", out.status),
            )));
        }

        let mut invalidated = Vec::new();
        let mut entries = 0usize;
        let mut missing = Vec::new();
        for ns in CacheNamespace::ALL {
            match self.invalidator.invalidate(&plan.tenant, &plan.repo, ns) {
                Ok(n) => {
                    entries += n;
                    invalidated.push(ns);
                }
                Err(_) => missing.push(ns),
            }
        }
        if !missing.is_empty() {
            return Err(HistoryRewriteError::IncompleteFanOut { missing });
        }

        let receipt = Receipt::content_addressed(
            "git.history_rewrite",
            "git",
            &plan.reason_code,
            plan.tenant.as_str(),
            &format!(
                "rewrote {} ref(s); invalidated {} cache namespace(s) ({} entries)",
                plan.target_refs.len(),
                invalidated.len(),
                entries
            ),
            None,
            at_ms,
        );

        Ok(HistoryRewriteReceipt {
            receipt,
            namespaces_invalidated: invalidated,
            entries_invalidated: entries,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScipIndexJob {
    pub repo: RepoLoc,
    pub commit_oid: String,
}

impl ScipIndexJob {
    pub fn index_argv(&self) -> Vec<String> {
        vec![
            "scip-index".to_string(),
            "--repo".to_string(),
            self.repo.repo.clone(),
            "--commit".to_string(),
            self.commit_oid.clone(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn repo() -> RepoLoc {
        RepoLoc::new("acme", "fr-par", "team/app")
    }
    fn tenant() -> TenantId {
        TenantId("acme".into())
    }

    struct RecordingWire {
        status: i32,
        ran: RefCell<Vec<Vec<String>>>,
    }
    impl RecordingWire {
        fn ok() -> RecordingWire {
            RecordingWire {
                status: 0,
                ran: RefCell::new(vec![]),
            }
        }
        fn failing() -> RecordingWire {
            RecordingWire {
                status: 1,
                ran: RefCell::new(vec![]),
            }
        }
    }
    impl WireExecutor for RecordingWire {
        fn run(&self, inv: &WireInvocation) -> Result<WireOutput, GitCoreError> {
            self.ran.borrow_mut().push(inv.argv.clone());
            Ok(WireOutput {
                stdout: vec![],
                status: self.status,
            })
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
            Ok(2)
        }
    }

    fn plan() -> HistoryRewritePlan {
        HistoryRewritePlan {
            tenant: tenant(),
            repo: repo(),
            target_refs: vec!["refs/heads/main".into()],
            reason_code: "leaked-secret".into(),
        }
    }

    #[test]
    fn the_cache_fan_out_set_is_fork_mirror_clone_cache_and_read_projection() {
        assert_eq!(CacheNamespace::ALL.len(), 4);
        assert!(CacheNamespace::ALL.contains(&CacheNamespace::Fork));
        assert!(CacheNamespace::ALL.contains(&CacheNamespace::Mirror));
        assert!(CacheNamespace::ALL.contains(&CacheNamespace::CloneCache));
        assert!(CacheNamespace::ALL.contains(&CacheNamespace::ReadProjection));
        assert_eq!(CacheNamespace::Fork.label(), "fork");
        assert_eq!(CacheNamespace::Mirror.label(), "mirror");
        assert_eq!(CacheNamespace::CloneCache.label(), "clone-cache");
    }

    #[test]
    fn a_history_rewrite_runs_sandboxed_then_fans_out_and_seals_an_audited_receipt() {
        let wire = RecordingWire::ok();
        let inv = RecordingInvalidator::all_ok();
        let tool = HistoryRewriteTool::new(wire, inv);
        let mut limiter = RewriteRateLimiter::new(5);

        let receipt = tool
            .rewrite(&plan(), &mut limiter, 1000)
            .expect("the rewrite is green");

        assert!(receipt.is_complete(), "the fan-out reached every namespace");
        assert_eq!(
            receipt.namespaces_invalidated.len(),
            CacheNamespace::ALL.len()
        );
        assert_eq!(receipt.entries_invalidated, 8, "2 entries × 4 namespaces");
        assert_eq!(receipt.receipt.operation, "git.history_rewrite");
        assert!(receipt.receipt.content_hash.starts_with("blake3:"));
        assert_eq!(limiter.consumed_by(&tenant()), 1);
    }

    #[test]
    fn the_rewrite_runs_sandboxed_through_the_wire_executor_no_host_exec() {
        let wire = RecordingWire::ok();
        let tool = HistoryRewriteTool::new(wire, RecordingInvalidator::all_ok());
        let mut limiter = RewriteRateLimiter::new(5);
        tool.rewrite(&plan(), &mut limiter, 1).unwrap();
        let ran = tool.wire.ran.borrow();
        assert_eq!(ran.len(), 1, "exactly one sandboxed invocation");
        assert_eq!(ran[0][0], "filter-repo", "a filter-repo-class rewrite");
        assert!(
            ran[0].iter().any(|a| a == "refs/heads/main"),
            "targets the planned ref"
        );
    }

    #[test]
    fn the_rewrite_is_rate_limited_per_tenant_a_refusal_does_not_run() {
        let wire = RecordingWire::ok();
        let tool = HistoryRewriteTool::new(wire, RecordingInvalidator::all_ok());
        let mut limiter = RewriteRateLimiter::new(1);

        assert!(
            tool.rewrite(&plan(), &mut limiter, 1).is_ok(),
            "first rewrite admitted"
        );
        let err = tool.rewrite(&plan(), &mut limiter, 2).unwrap_err();
        assert_eq!(
            err,
            HistoryRewriteError::RateLimited {
                tenant: "acme".into()
            }
        );
        assert_eq!(
            tool.wire.ran.borrow().len(),
            1,
            "the refused rewrite never reached the sandbox"
        );
        assert_eq!(limiter.consumed_by(&tenant()), 1);
    }

    #[test]
    fn a_zero_budget_tenant_is_always_refused() {
        let wire = RecordingWire::ok();
        let tool = HistoryRewriteTool::new(wire, RecordingInvalidator::all_ok());
        let mut limiter = RewriteRateLimiter::new(0);
        let err = tool.rewrite(&plan(), &mut limiter, 1).unwrap_err();
        assert!(matches!(err, HistoryRewriteError::RateLimited { .. }));
        assert_eq!(
            tool.wire.ran.borrow().len(),
            0,
            "rewrites disabled - nothing ran"
        );
    }

    #[test]
    fn an_incomplete_fan_out_aborts_loud_so_no_cache_can_resurrect_the_bytes() {
        let wire = RecordingWire::ok();
        let inv = RecordingInvalidator::failing(CacheNamespace::CloneCache);
        let tool = HistoryRewriteTool::new(wire, inv);
        let mut limiter = RewriteRateLimiter::new(5);
        let err = tool.rewrite(&plan(), &mut limiter, 1).unwrap_err();
        match err {
            HistoryRewriteError::IncompleteFanOut { missing } => {
                assert_eq!(missing, vec![CacheNamespace::CloneCache]);
            }
            other => panic!("expected IncompleteFanOut, got {other:?}"),
        }
    }

    #[test]
    fn a_sandbox_failure_aborts_the_op_before_any_fan_out_runs() {
        let wire = RecordingWire::failing();
        let inv = RecordingInvalidator::all_ok();
        let tool = HistoryRewriteTool::new(wire, inv);
        let mut limiter = RewriteRateLimiter::new(5);
        let err = tool.rewrite(&plan(), &mut limiter, 1).unwrap_err();
        assert!(matches!(err, HistoryRewriteError::SandboxFailed(_)));
        assert!(
            tool.invalidator.seen.borrow().is_empty(),
            "no fan-out on a failed rewrite"
        );
    }

    #[test]
    fn an_empty_plan_is_rejected_and_consumes_no_budget() {
        let wire = RecordingWire::ok();
        let tool = HistoryRewriteTool::new(wire, RecordingInvalidator::all_ok());
        let mut limiter = RewriteRateLimiter::new(5);
        let mut p = plan();
        p.target_refs.clear();
        assert_eq!(
            tool.rewrite(&p, &mut limiter, 1).unwrap_err(),
            HistoryRewriteError::EmptyPlan
        );
        assert_eq!(limiter.consumed_by(&tenant()), 0);
        assert_eq!(tool.wire.ran.borrow().len(), 0);
    }

    #[test]
    fn the_receipt_is_complete_only_when_every_namespace_is_invalidated() {
        let full = HistoryRewriteReceipt {
            receipt: Receipt::content_addressed(
                "git.history_rewrite",
                "git",
                "r",
                "acme",
                "ok",
                None,
                1,
            ),
            namespaces_invalidated: CacheNamespace::ALL.to_vec(),
            entries_invalidated: 8,
        };
        assert!(full.is_complete());
        let partial = HistoryRewriteReceipt {
            namespaces_invalidated: vec![CacheNamespace::Fork, CacheNamespace::Mirror],
            ..full.clone()
        };
        assert!(
            !partial.is_complete(),
            "a missed namespace is RED (a fork/mirror could resurrect)"
        );
    }

    #[test]
    fn the_code_tool_identity_constants_are_the_frozen_keys() {
        assert_eq!(GIT_SUBSYSTEM, "git");
        assert_eq!(HISTORY_REWRITE_TOOL, "history_rewrite");
        assert_eq!(SCIP_INDEX_TOOL, "scip_index");
        assert_eq!(
            history_rewrite_required_caps(),
            vec!["repo.administer".to_string()]
        );
        assert_eq!(scip_index_required_caps(), vec!["repo.pull".to_string()]);
    }

    #[test]
    fn the_rate_limiter_remaining_countdown_is_exact() {
        let mut limiter = RewriteRateLimiter::new(3);
        let t = tenant();
        assert_eq!(
            limiter.try_consume(&t),
            Some(2),
            "after the 1st of 3, 2 remain"
        );
        assert_eq!(limiter.try_consume(&t), Some(1), "after the 2nd, 1 remains");
        assert_eq!(limiter.try_consume(&t), Some(0), "after the 3rd, 0 remain");
        assert_eq!(
            limiter.try_consume(&t),
            None,
            "the 4th is refused (budget exhausted)"
        );
        assert_eq!(
            limiter.consumed_by(&t),
            3,
            "exactly 3 consumed (the refusal did not consume)"
        );
    }

    #[test]
    fn the_history_rewrite_errors_render_loud_and_self_describing() {
        assert!(HistoryRewriteError::EmptyPlan
            .to_string()
            .contains("no refs"));
        assert!(HistoryRewriteError::RateLimited {
            tenant: "acme".into()
        }
        .to_string()
        .contains("rate limit"));
        assert!(
            HistoryRewriteError::SandboxFailed(GitCoreError::Wire("boom".into()))
                .to_string()
                .contains("ABORTED")
        );
        assert!(HistoryRewriteError::IncompleteFanOut {
            missing: vec![CacheNamespace::Mirror]
        }
        .to_string()
        .contains("INCOMPLETE"));
    }

    #[test]
    fn a_scip_index_job_builds_a_sandboxed_indexer_argv_no_host_exec() {
        let job = ScipIndexJob {
            repo: repo(),
            commit_oid: "deadbeef".into(),
        };
        let argv = job.index_argv();
        assert_eq!(argv[0], "scip-index");
        assert!(
            argv.iter().any(|a| a == "deadbeef"),
            "indexes at the planned commit"
        );
    }
}
