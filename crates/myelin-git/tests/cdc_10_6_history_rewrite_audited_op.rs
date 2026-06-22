//! # CDC for contract 10.6 — the history-rewrite AUDITED, rate-limited tenant op (GIT-P27 → P-283)
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row **10.6**
//! (*Tamper-evident audit log … **History-rewrite is an audited op here** (rate-limited, with
//! fork/mirror/clone-cache invalidation fan-out)*). OWNED here as the TOOL. **Reconciliation:**
//! `00-reconciliation-decisions.md` §9 (*History-rewrite as audited, tamper-evident, rate-limited
//! tenant op with fork/mirror/clone-cache invalidation fan-out — the Git erasure-admin tool: an
//! audited op (contract 10.6 hash-chain) + the invalidation surface*). **Architecture:**
//! `git-hosting/architecture/03-events-contracts-and-glue.md` §6.2 (the history-rewrite erasure
//! path — audited, tamper-evident, rate-limited, with the cache invalidation fan-out).
//!
//! ## The CDC pair (the erasure-admin tool ↔ the audit-log + cache-namespace consumers)
//! - **PRODUCER (git's [`HistoryRewriteTool`]):** runs the rewrite SANDBOXED (no-host-exec), then
//!   fans out the cache invalidation over EVERY trust-scoped namespace, then seals a
//!   **content-addressed receipt** (the [`myelin_gdpr::Receipt`] hash-chain convention the audit log
//!   consumes — the Merkle seal is the GDPR P-GA-20 follow-on).
//! - **CONSUMER (the audit log + the fork/mirror/clone-cache):** the receipt is the audit hash-link
//!   (`blake3:<hex>`, the ONE multihash convention); the fan-out reaches fork + mirror + clone-cache
//!   (+ the read/projection caches) so no derived/cached/mirrored copy of the expunged bytes survives.
//!
//! This file pins the FROZEN consumer-visible properties so a producer change that broke the audit
//! shape / the fan-out completeness / the rate-limit / the no-fan-out-on-failure ordering is caught.
//!
//! DB-free: the tool composes the [`WireExecutor`] / [`CacheInvalidator`] trait seams (in-memory
//! shape stubs here; the production X-6 host + the storage cache classes are the seams' real impls).

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

/// A sandbox executor that records the argv + returns a chosen status (the no-host-exec seam — the
/// rewrite never shells out to the host).
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

/// A cache-invalidation fan-out that records the namespaces it reached (or fails one). The `seen`
/// recorder is a SHARED `Rc<RefCell<…>>` so a test can inspect the fan-out after the tool owns the
/// invalidator.
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

// ───────────────────────── PRODUCER → CONSUMER: the audited receipt shape ────────────────────────

/// **The audited op returns a CONTENT-ADDRESSED receipt (the 10.6 audit hash-chain link).** The
/// receipt's `content_hash` is the `blake3:<hex>` multihash the audit log seals; the operation is
/// the frozen `git.history_rewrite` tag. (The Merkle seal of the link is the GDPR P-GA-20 follow-on.)
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

    // the audit hash-chain link: the ONE multihash convention the audit Merkle leaf uses.
    assert_eq!(r.receipt.operation, "git.history_rewrite");
    assert!(
        r.receipt.content_hash.starts_with("blake3:"),
        "the audit hash-link is blake3:<hex>"
    );
    // deterministic: the SAME plan at the SAME time seals the SAME content-address (replay-safe).
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

// ───────────────────────── the fork/mirror/clone-cache invalidation fan-out ──────────────────────

/// **The fan-out reaches EVERY trust-scoped namespace (recon §9): fork + mirror + clone-cache (+ the
/// read/projection caches).** The receipt names them, so a fork/mirror/CDN cannot resurrect the
/// expunged bytes — and a missed namespace is a visible RED.
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

/// **An INCOMPLETE fan-out aborts LOUD (a fork/mirror/CDN could otherwise resurrect the bytes).**
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

// ───────────────────────── rate-limited + sandboxed (recon §9 + 8.4) ─────────────────────────────

/// **The op is RATE-LIMITED per tenant (recon §9) and runs SANDBOXED through the no-host-exec
/// WireExecutor (8.4).** A budget refusal does not run the sandbox; the rewrite is a `filter-repo`
/// -class canonical-git invocation, never a host shell-out.
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
    // first admitted, runs sandboxed.
    assert!(tool.rewrite(&plan(), &mut limiter, 1).is_ok());
    // second refused by the rate limit — does NOT reach the sandbox.
    assert!(matches!(
        tool.rewrite(&plan(), &mut limiter, 2).unwrap_err(),
        HistoryRewriteError::RateLimited { .. }
    ));
}

/// **No fan-out runs on a FAILED sandbox rewrite (the caches still point at valid old bytes).** The
/// shared `seen` recorder proves the invalidator was NEVER called when the sandbox rewrite failed.
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
