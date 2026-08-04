use myelin_git::code_tools::{
    CacheInvalidator, CacheNamespace, HistoryRewriteError, HistoryRewritePlan, HistoryRewriteTool,
    RewriteRateLimiter,
};
use myelin_git::core::{GitCoreError, RepoLoc, WireExecutor, WireInvocation, WireOutput};
use myelin_tenancy::TenantId;
use std::cell::RefCell;
use std::rc::Rc;

fn repo() -> RepoLoc {
    RepoLoc::new("acme", "fr-par", "team/secrets-leak")
}
fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn plan() -> HistoryRewritePlan {
    HistoryRewritePlan {
        tenant: tenant(),
        repo: repo(),
        target_refs: vec!["refs/heads/main".into()],
        reason_code: "leaked-secret".into(),
    }
}

struct Wire {
    status: i32,
    ran: RefCell<Vec<Vec<String>>>,
}
impl WireExecutor for Wire {
    fn run(&self, inv: &WireInvocation) -> Result<WireOutput, GitCoreError> {
        self.ran.borrow_mut().push(inv.argv.clone());
        Ok(WireOutput {
            stdout: vec![],
            status: self.status,
        })
    }
}

struct Inv {
    fail: Option<CacheNamespace>,
    seen: Rc<RefCell<Vec<CacheNamespace>>>,
}
impl Inv {
    fn new(fail: Option<CacheNamespace>) -> (Inv, Rc<RefCell<Vec<CacheNamespace>>>) {
        let seen = Rc::new(RefCell::new(vec![]));
        (
            Inv {
                fail,
                seen: seen.clone(),
            },
            seen,
        )
    }
}
impl CacheInvalidator for Inv {
    fn invalidate(
        &self,
        _t: &TenantId,
        _r: &RepoLoc,
        ns: CacheNamespace,
    ) -> Result<usize, GitCoreError> {
        if self.fail == Some(ns) {
            return Err(GitCoreError::Wire("unreachable".into()));
        }
        self.seen.borrow_mut().push(ns);
        Ok(1)
    }
}

#[test]
fn the_history_rewrite_seals_a_content_addressed_audit_receipt() {
    let tool = HistoryRewriteTool::new(
        Wire {
            status: 0,
            ran: RefCell::new(vec![]),
        },
        Inv::new(None).0,
    );
    let mut limiter = RewriteRateLimiter::new(5);
    let r = tool
        .rewrite(&plan(), &mut limiter, 1700000000000)
        .expect("green rewrite");

    assert_eq!(r.receipt.operation, "git.history_rewrite");
    assert!(
        r.receipt.content_hash.starts_with("blake3:"),
        "the audit hash-link is blake3:<hex>"
    );
    let tool2 = HistoryRewriteTool::new(
        Wire {
            status: 0,
            ran: RefCell::new(vec![]),
        },
        Inv::new(None).0,
    );
    let mut l2 = RewriteRateLimiter::new(5);
    let r2 = tool2.rewrite(&plan(), &mut l2, 1700000000000).unwrap();
    assert_eq!(
        r.receipt.content_hash, r2.receipt.content_hash,
        "the audit link is deterministic"
    );
}

#[test]
fn the_invalidation_fan_out_reaches_fork_mirror_and_clone_cache() {
    let inv = Inv::new(None).0;
    let tool = HistoryRewriteTool::new(
        Wire {
            status: 0,
            ran: RefCell::new(vec![]),
        },
        inv,
    );
    let mut limiter = RewriteRateLimiter::new(5);
    let r = tool.rewrite(&plan(), &mut limiter, 1).unwrap();

    assert!(
        r.is_complete(),
        "the fan-out reached every trust-scoped namespace"
    );
    for ns in [
        CacheNamespace::Fork,
        CacheNamespace::Mirror,
        CacheNamespace::CloneCache,
    ] {
        assert!(
            r.namespaces_invalidated.contains(&ns),
            "the {} cache was invalidated (recon §9 fan-out)",
            ns.label()
        );
    }
}

#[test]
fn an_incomplete_fan_out_is_a_loud_red() {
    let inv = Inv::new(Some(CacheNamespace::Mirror)).0;
    let tool = HistoryRewriteTool::new(
        Wire {
            status: 0,
            ran: RefCell::new(vec![]),
        },
        inv,
    );
    let mut limiter = RewriteRateLimiter::new(5);
    let err = tool.rewrite(&plan(), &mut limiter, 1).unwrap_err();
    assert_eq!(
        err,
        HistoryRewriteError::IncompleteFanOut {
            missing: vec![CacheNamespace::Mirror]
        }
    );
}

#[test]
fn the_op_is_rate_limited_and_runs_sandboxed_no_host_exec() {
    let tool = HistoryRewriteTool::new(
        Wire {
            status: 0,
            ran: RefCell::new(vec![]),
        },
        Inv::new(None).0,
    );
    let mut limiter = RewriteRateLimiter::new(1);
    assert!(tool.rewrite(&plan(), &mut limiter, 1).is_ok());
    assert!(matches!(
        tool.rewrite(&plan(), &mut limiter, 2).unwrap_err(),
        HistoryRewriteError::RateLimited { .. }
    ));
}

#[test]
fn a_failed_rewrite_runs_no_fan_out() {
    let (inv, seen) = Inv::new(None);
    let tool = HistoryRewriteTool::new(
        Wire {
            status: 1,
            ran: RefCell::new(vec![]),
        },
        inv,
    );
    let mut limiter = RewriteRateLimiter::new(5);
    assert!(matches!(
        tool.rewrite(&plan(), &mut limiter, 1).unwrap_err(),
        HistoryRewriteError::SandboxFailed(_)
    ));
    assert!(
        seen.borrow().is_empty(),
        "no cache was invalidated for a rewrite that failed (the fan-out never ran)"
    );
}
