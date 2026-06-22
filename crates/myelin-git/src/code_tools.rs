//! # `code_tools` — the code-executing git tools on the unified sandbox (GIT-P27 / P-283, M3-G6)
//!
//! The **history-rewrite erasure path** (an audited, tamper-evident, **rate-limited** tenant op,
//! contract 10.6, with **fork/mirror/clone-cache invalidation fan-out**) + the **SCIP indexing**
//! compute job — the two **code-executing** git tools that ride the **ONE unified sandbox** by
//! construction.
//!
//! **Owning architecture docs (read in full before editing):**
//! - `planning/04-subsystem-architectures/git-hosting/architecture/03-events-contracts-and-glue.md`
//!   §7 (the git `ToolDef`s + **the four uniform sandbox guarantees** apply by construction to any
//!   git tool that executes code — *the history-rewrite activity, SCIP indexing if run as a job*),
//!   §6.2 (the history-rewrite erasure path: an audited, tamper-evident, rate-limited tenant op
//!   with fork/mirror/clone-cache invalidation fan-out — the trust-scoped cache namespaces).
//! - `00-reconciliation-decisions.md` X-6 (the unified sandbox; `ToolHands::exec` **is** CI's
//!   `kind=agent` job; the four uniform guarantees), §9 (**history-rewrite as audited,
//!   tamper-evident, rate-limited tenant op with fork/mirror/clone-cache invalidation fan-out** —
//!   the Git erasure-admin tool: an audited op (contract 10.6 hash-chain) + the invalidation
//!   surface, ties to the trust-scoped cache namespaces).
//! - VISION §3 (consequential/irreversible actions human-confirmed — history-rewrite changes every
//!   downstream hash, so it is the `requires_approval = yes` consequential gate).
//!
//! **Contract-index rows:**
//! - **10.6** (OWNED here as the TOOL) — the audited history-rewrite tenant op: rate-limited,
//!   content-addressed receipt (the [`myelin_gdpr::Receipt`] hash-chain convention — the Merkle
//!   seal is the GDPR P-GA-20 follow-on), with the fork/mirror/clone-cache invalidation fan-out.
//!   The erasure SEMANTICS (the byte-expunge actually reaching every holder) COMPLETE at GIT-P29
//!   (→ P-289 follow-on band); here we build the audited, sandboxed, rate-limited TOOL.
//! - **8.4** (CONSUMED) — the code-executing tools ride the unified sandbox: history-rewrite is a
//!   sandboxed canonical-`git` invocation through the [`crate::core::WireExecutor`] port (the git
//!   no-host-exec seam, the same "all execution goes through the ONE sandbox seam" discipline as
//!   `ToolHands::exec`); SCIP indexing is a sandboxed compute job descriptor.
//!
//! ## Why this lives in `myelin-git` (not in the Fabric)
//! The history-rewrite op is **git's domain logic** — it rides git's own [`crate::core::WireExecutor`]
//! sandbox port (a `filter-repo`/`replace`-class canonical-`git` invocation) and fans out over git's
//! own fork/mirror/clone-cache namespaces. The Fabric `ToolDef` **registration** (the catalogue row
//! the agent loop sees) is the THIN projection that lives in `myelin_agent_service::git_tools` (it
//! cannot live here — `myelin-git` is a LEAF, it does not depend on `myelin-agent`; the §2.9 DAG).
//! This module owns the OP + the identity constants that registration keys on (so a rename here is a
//! compile/test break there, never a silent drift). EI-01 §7 (extend/reconcile, never duplicate):
//! the `merge`/`open_pr` producer tools already live in `git_tools.rs` (P-267); this prompt ADDS the
//! two code-executing tools to that SAME registration site, keyed on the constants here.
//!
//! ## The four uniform guarantees — BY CONSTRUCTION, never re-implemented (§7 / X-6)
//! A history-rewrite / SCIP job executes code, so it inherits all four **by construction**, exactly
//! because it rides the ONE sandbox seam (it does NOT re-implement any):
//! 1. **reserve/settle cost gate** (11.7) — heavy maintenance compute (history-rewrite, SCIP) runs
//!    as a CI job / durable activity through the universal reserve/settle gate (it does NOT meter
//!    here; it declares the spend-bearing activity to the shared gate, `03 §8`).
//! 2. **per-run attenuated token** (4.7) — the sandbox job runs under the per-run token, not the
//!    broad platform token (the Fabric/CI runner's `mint_run_token`).
//! 3. **HITL withhold** — history-rewrite is `requires_approval = yes` (consequential): the agent
//!    `ToolDef` routes through `EffectApi::apply` (plan-then-apply) and WITHHOLDS until approval; it
//!    never reaches the bare sandbox as an un-gated agent mutation.
//! 4. **isolation floor + the real-kernel escape drill (AG-D4)** — the production sandbox is the
//!    X-6-hardened CI runner the AG-D4 escape gate proved green; this module's [`crate::core::WireExecutor`]
//!    port carries NO host-exec fingerprint (the `no-host-exec` lint stays green over `src/`).
//!
//! ## Floors named (VISION §3 / EI-01 §1)
//! - **GF-9 — `exposed_over_mcp = false`** for both code-executing tools (the external MCP server +
//!   threat model is the post-M5 follow-on, GIT-P33/P6 + Legal). Set here, no external endpoint.
//! - **The erasure SEMANTICS complete at GIT-P29** — this module builds the audited, rate-limited,
//!   sandboxed history-rewrite TOOL + the invalidation fan-out; the byte-expunge actually reaching
//!   every fork/mirror/clone-cache holder (the full GIT-D2-completion erase) lands at GIT-P29.
//! - **Agents-as-first-class-authors/reviewers is GIT-P28** (→ P-289): the legible, bounded
//!   agent authoring this tool is invoked through — HITL on `git.merge`, AG-D1/D2/D3/D5.
//! - **The real X-6-hardened WireExecutor host** (the sandboxed `git` launch on the CI runner) is
//!   wired in GIT-P9/GIT-P13; this module is the OP that rides that port (the port is a swappable
//!   seam — the test executor and the production host both implement [`crate::core::WireExecutor`]).
//!
//! ## DB-free
//! This module builds in-memory plan/receipt values and invokes the [`crate::core::WireExecutor`]
//! trait seam (the production host's reserve/settle/token bodies it consumes are proven against the
//! live stack at AG-P14/AG-P13 / the receive-pack store). So `cargo build --workspace` stays DB-free.

use crate::core::{GitCoreError, RepoLoc, WireExecutor, WireInvocation, WireOutput};
use myelin_gdpr::Receipt;
use myelin_tenancy::TenantId;

// ───────────────────────── the code-executing git tool identity (the catalogue keys) ─────────────

/// **The Git subsystem token** — the `subsystem` half of the catalogue key `(subsystem, name,
/// version)` and the key the FROZEN §6.3 `requires_approval` defaults table is looked up under. The
/// SINGLE source of truth, shared with the `merge`/`open_pr` producer tools (P-267). A typo can't
/// drift the registration seed because the Fabric registration consumes THIS constant.
pub const GIT_SUBSYSTEM: &str = "git";

/// **The `git.history_rewrite` tool name** (10.6 / recon §9 — the audited erasure-admin tool). The
/// agent `ToolDef` keys the FROZEN §6.3 default on `("git", "history_rewrite")` — a NAMED gated row
/// (a changes-every-downstream-hash erasure op is consequential/irreversible, VISION §3).
pub const HISTORY_REWRITE_TOOL: &str = "history_rewrite";

/// **The `git.scip_index` tool name** (§7 — "SCIP indexing if run as a job"). A `compute` tool that
/// rides the unified sandbox via `ToolHands::exec` (it produces a SCIP code-intelligence index, a
/// read-only artifact — no mutation, so it is NOT gated).
pub const SCIP_INDEX_TOOL: &str = "scip_index";

/// **The code-tool `ToolDef` version** (forward-only; the catalogue key is `(subsystem, name,
/// version)`). v1 is the first frozen shape, aligned with the `merge`/`open_pr` producer tools.
pub const GIT_CODE_TOOL_VERSION: u32 = 1;

/// The `required_caps` for `git.history_rewrite` — the audited erasure op is an **admin-scoped
/// tenant op** (recon §9 "tenant op"), governed by the `repo.administer` permission the FROZEN Git
/// ReBAC fragment already declares ([`repo_fragment`](crate::rebac_fragment::repo_fragment):
/// `administer = admin + parent_project->admin`). The cap STRING is `"<object_type>.<permission>"`
/// (the EffectApi `check` step resolves it). Built from the canonical `myelin-git` constants so a
/// fragment rename is a compile/test break here, never a silent drift. (We do NOT invent a new
/// permission — the frozen four-permission `repo` set stays frozen; an erasure-admin op is `administer`.)
pub fn history_rewrite_required_caps() -> Vec<String> {
    vec![format!(
        "{}.administer",
        crate::rebac_fragment::object_types::REPO
    )]
}

/// The `required_caps` for `git.scip_index` — building a code-intelligence index reads the repo
/// objects, governed by `repo.pull` (the read permission; a compute artifact over readable bytes).
pub fn scip_index_required_caps() -> Vec<String> {
    vec![format!(
        "{}.pull",
        crate::rebac_fragment::object_types::REPO
    )]
}

// ───────────────────────── the trust-scoped cache namespaces (the fan-out surface, 11.2 C4) ───────

/// **A trust-scoped cache namespace a history-rewrite must INVALIDATE** (recon §9 — the
/// fork/mirror/clone-cache invalidation fan-out; Storage 11.2 C4 the trust-scoped cache namespaces).
/// When the immutable bytes change (history-rewrite changes every downstream hash), every derived /
/// cached / mirrored copy of the OLD bytes must be invalidated, or a fork/mirror/CDN-clone could
/// resurrect the expunged content. PII-free — an opaque namespace tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CacheNamespace {
    /// **Fork caches** — a fork shares the upstream's object DB through a fork-scoped namespace
    /// (`fork:<pr_id>` confinement, the trust-scoped fork cache). A rewrite must invalidate every
    /// fork's view of the rewritten objects.
    Fork,
    /// **Push-mirror caches** — an outbound push-mirror holds a (content-addressed, encrypted) copy
    /// of the pack bytes (Storage C6). A rewrite must invalidate the mirror's cached advertisement /
    /// pack so a re-clone does not serve the old bytes.
    Mirror,
    /// **CDN clone-cache / bundle-URI** — the within-EU CDN clone class caches bundle URIs +
    /// advertised refs (11.2 C3). A rewrite must invalidate the cached bundle so an accelerated clone
    /// re-fetches the rewritten history.
    CloneCache,
    /// **The read/projection caches** — the in-process read backend's blob/diff/projection caches
    /// (per-tenant namespaced). A rewrite invalidates them so Search / `project(ref)` re-read the
    /// rewritten bytes (never the stale OID).
    ReadProjection,
}

impl CacheNamespace {
    /// A stable, PII-free label (telemetry / the receipt — never personal data).
    pub fn label(self) -> &'static str {
        match self {
            CacheNamespace::Fork => "fork",
            CacheNamespace::Mirror => "mirror",
            CacheNamespace::CloneCache => "clone-cache",
            CacheNamespace::ReadProjection => "read-projection",
        }
    }

    /// **The FULL fan-out set a history-rewrite must reach** (recon §9: fork + mirror + clone-cache,
    /// plus the read/projection caches the rewritten OID would otherwise be stale in). The op asserts
    /// EVERY member is invalidated — a missed namespace is a leak (a fork/mirror/CDN could resurrect
    /// the expunged bytes). The set is closed so a new cache surface can NOT be added without a
    /// fan-out decision (the routing is total — proven by the unit test over `ALL`).
    pub const ALL: [CacheNamespace; 4] = [
        CacheNamespace::Fork,
        CacheNamespace::Mirror,
        CacheNamespace::CloneCache,
        CacheNamespace::ReadProjection,
    ];
}

/// **The cache-invalidation fan-out port (the trust-scoped namespaces, recon §9 / 11.2 C4).** A
/// history-rewrite invalidates a `(tenant, repo, namespace)` cache entry through this seam — the
/// production impl drops the trust-scoped cache namespace (the [`myelin_storage`] cache / CDN
/// classes); the test impl records the fan-out. The op REQUIRES every [`CacheNamespace::ALL`] member
/// be invalidated, so the seam is the structural fan-out, not an optional best-effort.
pub trait CacheInvalidator {
    /// Invalidate the cached copies of `repo`'s rewritten objects in `tenant`'s `namespace`. Returns
    /// the count of cache entries dropped (`0` is a clean already-empty namespace — still success).
    fn invalidate(
        &self,
        tenant: &TenantId,
        repo: &RepoLoc,
        namespace: CacheNamespace,
    ) -> Result<usize, GitCoreError>;
}

/// A reference to a [`CacheInvalidator`] is itself a [`CacheInvalidator`] (the blanket forwarding
/// impl). This lets a caller wire the SAME invalidator into both the [`HistoryRewriteTool`] and a
/// holder's erase fan-out (the GIT-P29 [`crate::holder::GitPersonalDataHolder`]) by reference — one
/// invalidator, never two parallel cache seams (EI-01 §7).
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

// ───────────────────────── the rate limiter (the "rate-limited tenant op", recon §9) ─────────────

/// **A per-tenant rate limiter for the history-rewrite op (recon §9 — "rate-limited tenant op").**
/// History-rewrite is a heavy, consequential, hash-changing op; a tenant may not run an unbounded
/// stream of them (a runaway erasure-admin would thrash every fork/mirror/clone-cache). The limiter
/// is a simple per-tenant token budget over a window; a refused op is a LOUD [`HistoryRewriteError::RateLimited`]
/// (never a silent drop — the op did not run, the caller retries after the window).
///
/// The PRODUCTION limiter is the shared substrate rate limiter (the same lane the front-door shed +
/// the agent runaway self-limiter use); this is the typed per-tenant budget the op consults. It is
/// deliberately tiny (a max count per window) — the real distributed counter is the substrate's.
#[derive(Clone, Debug)]
pub struct RewriteRateLimiter {
    /// The maximum number of history-rewrites a tenant may run per window (the budget). `0` refuses
    /// every rewrite (a tenant with rewrites disabled).
    max_per_window: u32,
    /// The per-tenant consumed count this window (`(tenant, consumed)`). A real impl keys on a
    /// windowed distributed counter; the in-memory budget is the typed seam.
    consumed: std::collections::HashMap<String, u32>,
}

impl RewriteRateLimiter {
    /// Build a limiter admitting at most `max_per_window` history-rewrites per tenant per window.
    pub fn new(max_per_window: u32) -> RewriteRateLimiter {
        RewriteRateLimiter {
            max_per_window,
            consumed: std::collections::HashMap::new(),
        }
    }

    /// **Try to consume one rewrite budget for `tenant`.** Returns `Some(remaining)` (the budget LEFT
    /// after this consume) and DECREMENTS the budget on success; returns `None` (budget exhausted)
    /// WITHOUT mutating on refusal — the op did not run, so it did not consume. Fail-closed: a
    /// `max_per_window == 0` tenant is always refused (rewrites disabled). Public so the per-tenant
    /// budget arithmetic (the `remaining` countdown) is directly testable.
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

    /// How many rewrites `tenant` has consumed this window (observability — never personal data).
    pub fn consumed_by(&self, tenant: &TenantId) -> u32 {
        self.consumed.get(tenant.as_str()).copied().unwrap_or(0)
    }
}

// ───────────────────────── the history-rewrite plan + its audited, sandboxed run ──────────────────

/// **A history-rewrite plan — the typed erasure-admin op the tool executes** (10.6 / recon §9). The
/// rare case where a body must be EXPUNGED from the immutable bytes (a leaked secret, a court order)
/// — with the understood consequence of changed hashes (every downstream OID changes). PII-free: the
/// plan names the repo + the target refs + an opaque reason code, never the leaked content itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryRewritePlan {
    /// The tenant the audited op runs FOR (the rate-limit + audit + fan-out are per-tenant).
    pub tenant: TenantId,
    /// The repo whose history is rewritten.
    pub repo: RepoLoc,
    /// The refs the rewrite targets (the branches whose history changes). At least one — an empty
    /// target is a rejected no-op plan (the op must not run a rewrite that touches nothing).
    pub target_refs: Vec<String>,
    /// An OPAQUE reason code for the audit trail (`"leaked-secret"`, `"court-order"`, `"dsr-body"`)
    /// — never the leaked content, never personal data. Folded into the content-addressed receipt.
    pub reason_code: String,
}

impl HistoryRewritePlan {
    /// The canonical sandboxed `git` argv for the rewrite — a `filter-repo`-class invocation that
    /// rewrites the target refs. The seam BUILDS it; the [`WireExecutor`] runs it sandboxed (the
    /// no-host-exec discipline). The real filter expression (the path/blob to expunge) is resolved
    /// inside the boundary; the argv carries only the op + the target refs (PII-free).
    fn rewrite_argv(&self) -> Vec<String> {
        let mut argv = vec!["filter-repo".to_string(), "--force".to_string()];
        for r in &self.target_refs {
            argv.push("--refs".to_string());
            argv.push(r.clone());
        }
        argv
    }
}

/// **Why a history-rewrite was REFUSED (every refusal is LOUD + self-describing — never a swallowed
/// pass).** A rate-limit / empty-plan / sandbox-failure / incomplete-fan-out is a typed error the
/// caller surfaces (the audited op records the attempt; the rewrite did NOT happen).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HistoryRewriteError {
    /// The plan targets no refs — a rewrite that touches nothing is a rejected no-op (the op must
    /// not run an empty rewrite).
    EmptyPlan,
    /// The per-tenant rate limit (recon §9) refused the op — the tenant exceeded its window budget.
    /// LOUD: the rewrite did NOT run; the caller retries after the window.
    RateLimited {
        /// The tenant whose budget was exhausted (opaque — never personal data).
        tenant: String,
    },
    /// The sandboxed `git` rewrite invocation failed (a non-zero exit / a sandbox veto). The op is
    /// ABORTED — no fan-out runs on a failed rewrite (the cache still points at valid old bytes).
    SandboxFailed(GitCoreError),
    /// The cache-invalidation fan-out did NOT reach every trust-scoped namespace — a fork / mirror /
    /// clone-cache could resurrect the expunged bytes. The op is recorded as INCOMPLETE (LOUD); the
    /// missing namespaces are named.
    IncompleteFanOut {
        /// The namespaces that were NOT invalidated (the fan-out gap).
        missing: Vec<CacheNamespace>,
    },
}

impl std::fmt::Display for HistoryRewriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HistoryRewriteError::EmptyPlan => write!(
                f,
                "history-rewrite plan targets no refs — a rewrite that touches nothing is rejected \
                 (the audited op must not run an empty rewrite)"
            ),
            HistoryRewriteError::RateLimited { tenant } => write!(
                f,
                "history-rewrite REFUSED for tenant `{tenant}`: per-tenant rate limit exhausted \
                 (recon §9 — the rewrite is a rate-limited tenant op; it did NOT run, retry after \
                 the window)"
            ),
            HistoryRewriteError::SandboxFailed(e) => write!(
                f,
                "history-rewrite sandbox invocation failed: {e} — the op is ABORTED, no \
                 cache-invalidation fan-out ran (the caches still point at valid pre-rewrite bytes)"
            ),
            HistoryRewriteError::IncompleteFanOut { missing } => write!(
                f,
                "history-rewrite cache-invalidation fan-out is INCOMPLETE — {} trust-scoped \
                 namespace(s) NOT invalidated ({:?}); a fork/mirror/clone-cache could resurrect the \
                 expunged bytes (recon §9 fan-out)",
                missing.len(),
                missing.iter().map(|n| n.label()).collect::<Vec<_>>(),
            ),
        }
    }
}

impl std::error::Error for HistoryRewriteError {}

/// **The dated, content-addressed receipt the audited history-rewrite returns** (10.6 — the audit
/// hash-chain; the Merkle seal is the GDPR P-GA-20 follow-on). PROOF the rewrite ran sandboxed AND
/// the cache-invalidation fan-out reached every trust-scoped namespace. PII-free: opaque tenant +
/// repo + reason code + namespace labels, sealed into the content-addressed [`Receipt`] body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryRewriteReceipt {
    /// The content-addressed audit receipt (the hash-chain link, [`myelin_gdpr::Receipt`] — the ONE
    /// multihash convention the audit Merkle leaf uses; the seal is P-GA-20).
    pub receipt: Receipt,
    /// The trust-scoped namespaces the fan-out invalidated (MUST be the full [`CacheNamespace::ALL`]
    /// set — the receipt names them so a dropped namespace is visible).
    pub namespaces_invalidated: Vec<CacheNamespace>,
    /// The total cache entries dropped across the fan-out (observability — never personal data).
    pub entries_invalidated: usize,
}

impl HistoryRewriteReceipt {
    /// Whether the audited history-rewrite is GREEN: the fan-out reached EVERY trust-scoped namespace
    /// (so no fork/mirror/clone-cache can resurrect the expunged bytes). A missed namespace is RED.
    pub fn is_complete(&self) -> bool {
        CacheNamespace::ALL
            .iter()
            .all(|n| self.namespaces_invalidated.contains(n))
    }
}

/// **The audited, rate-limited history-rewrite tool (contract 10.6 / recon §9) — OWNED here.**
///
/// Composes git's sandbox [`WireExecutor`] port (the rewrite runs sandboxed — no-host-exec) + the
/// [`CacheInvalidator`] fan-out (the trust-scoped namespaces) + the [`RewriteRateLimiter`]
/// (per-tenant budget). It does NOT re-implement the sandbox, the cache, or the audit Merkle — it
/// ties git's existing seams into the one audited erasure-admin op (EI-01 §7).
pub struct HistoryRewriteTool<E: WireExecutor, I: CacheInvalidator> {
    wire: E,
    invalidator: I,
}

impl<E: WireExecutor, I: CacheInvalidator> HistoryRewriteTool<E, I> {
    /// Build the tool over git's sandbox executor + the cache-invalidation fan-out.
    pub fn new(wire: E, invalidator: I) -> HistoryRewriteTool<E, I> {
        HistoryRewriteTool { wire, invalidator }
    }

    /// **Run the audited history-rewrite for a plan** (10.6 / recon §9). In strict order:
    ///
    /// 1. **reject an empty plan** ([`HistoryRewriteError::EmptyPlan`]) — the op must not run a
    ///    rewrite that touches nothing.
    /// 2. **rate-limit** (recon §9) — consume one per-tenant budget; a refusal is LOUD
    ///    ([`HistoryRewriteError::RateLimited`]) and the rewrite does NOT run (the budget is NOT
    ///    consumed on refusal).
    /// 3. **run the rewrite SANDBOXED** — the `filter-repo`-class invocation through the
    ///    [`WireExecutor`] port (no-host-exec; the X-6-hardened sandbox the AG-D4 drill gates). A
    ///    non-zero exit ABORTS the op ([`HistoryRewriteError::SandboxFailed`]) — no fan-out runs on a
    ///    failed rewrite (the caches still point at valid old bytes).
    /// 4. **fan out the cache invalidation** over EVERY trust-scoped namespace (fork / mirror /
    ///    clone-cache / read-projection). A namespace that fails to invalidate aborts the op
    ///    ([`HistoryRewriteError::IncompleteFanOut`]) — a fork/mirror/CDN could otherwise resurrect
    ///    the expunged bytes.
    /// 5. **seal the audited, content-addressed receipt** (the 10.6 hash-chain link; the Merkle seal
    ///    is P-GA-20). Returns the dated [`HistoryRewriteReceipt`].
    ///
    /// `at_ms` is the op timestamp (folded into the content-addressed receipt body — deterministic,
    /// so a replay returns the identical receipt). The limiter is `&mut` (the budget mutates).
    pub fn rewrite(
        &self,
        plan: &HistoryRewritePlan,
        limiter: &mut RewriteRateLimiter,
        at_ms: u64,
    ) -> Result<HistoryRewriteReceipt, HistoryRewriteError> {
        // (1) reject an empty plan — a rewrite that touches nothing is a no-op the op must refuse.
        if plan.target_refs.is_empty() {
            return Err(HistoryRewriteError::EmptyPlan);
        }

        // (2) RATE-LIMIT (recon §9 — the rewrite is a rate-limited tenant op). A refusal does NOT
        //     consume budget and the rewrite does NOT run (LOUD, never a silent drop).
        if limiter.try_consume(&plan.tenant).is_none() {
            return Err(HistoryRewriteError::RateLimited {
                tenant: plan.tenant.as_str().to_string(),
            });
        }

        // (3) run the rewrite SANDBOXED through git's no-host-exec WireExecutor port (the X-6
        //     hardened sandbox the AG-D4 escape drill gates). A non-zero exit ABORTS — no fan-out.
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

        // (4) FAN OUT the cache invalidation over EVERY trust-scoped namespace (fork / mirror /
        //     clone-cache / read-projection). A namespace that fails to invalidate is a fan-out gap
        //     (a fork/mirror/CDN could resurrect the expunged bytes) — abort INCOMPLETE.
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

        // (5) seal the audited, content-addressed receipt (10.6 hash-chain; Merkle seal = P-GA-20).
        //     The body carries only opaque ids + the reason code — PII-free, safe to seal.
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

// ───────────────────────── SCIP indexing — the compute job descriptor (rides the sandbox) ─────────

/// **A SCIP indexing compute job descriptor** (§7 — "SCIP indexing if run as a job"). A `compute`
/// tool: it produces a SCIP code-intelligence index (the `find usages` / `goto def` artifact the
/// Search code projection consumes) by reading the repo objects — a read-only artifact, no mutation.
/// It rides the unified sandbox via `ToolHands::exec` (a `compute` effect → the kernel sandbox). The
/// descriptor names the repo + the commit OID to index; the indexer argv runs sandboxed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScipIndexJob {
    /// The repo to index.
    pub repo: RepoLoc,
    /// The commit OID the SCIP index is built at (the index is content-addressed to the commit).
    pub commit_oid: String,
}

impl ScipIndexJob {
    /// The sandboxed indexer argv — a SCIP indexer over the repo at `commit_oid`. The seam BUILDS it;
    /// the unified sandbox (`ToolHands::exec` / a CI `kind=agent` job) runs it. NO host-exec here.
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

    /// A `WireExecutor` that records the sandboxed argv it ran and returns a chosen exit status — a
    /// SHAPE stub for the X-6-hardened production host (GIT-P9/P13). NO `std::process::Command` (the
    /// `no-host-exec` lint stays green over `src/`); all execution goes through this `run` seam.
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

    /// A `CacheInvalidator` that records the fan-out, or fails a chosen namespace (the gap case).
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
            Ok(2) // two cached entries dropped per namespace (the shape stub).
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

    // ───────────────────────── the fan-out surface is the full trust-scoped set ─────────────────

    #[test]
    fn the_cache_fan_out_set_is_fork_mirror_clone_cache_and_read_projection() {
        // recon §9: the fan-out reaches fork + mirror + clone-cache (+ the read/projection caches the
        // rewritten OID would be stale in). The closed set is the structural fan-out surface.
        assert_eq!(CacheNamespace::ALL.len(), 4);
        assert!(CacheNamespace::ALL.contains(&CacheNamespace::Fork));
        assert!(CacheNamespace::ALL.contains(&CacheNamespace::Mirror));
        assert!(CacheNamespace::ALL.contains(&CacheNamespace::CloneCache));
        assert!(CacheNamespace::ALL.contains(&CacheNamespace::ReadProjection));
        // labels are stable + PII-free.
        assert_eq!(CacheNamespace::Fork.label(), "fork");
        assert_eq!(CacheNamespace::Mirror.label(), "mirror");
        assert_eq!(CacheNamespace::CloneCache.label(), "clone-cache");
    }

    // ───────────────────────── the audited, sandboxed, fanned-out rewrite ───────────────────────

    #[test]
    fn a_history_rewrite_runs_sandboxed_then_fans_out_and_seals_an_audited_receipt() {
        let wire = RecordingWire::ok();
        let inv = RecordingInvalidator::all_ok();
        let tool = HistoryRewriteTool::new(wire, inv);
        let mut limiter = RewriteRateLimiter::new(5);

        let receipt = tool
            .rewrite(&plan(), &mut limiter, 1000)
            .expect("the rewrite is green");

        // The fan-out reached EVERY trust-scoped namespace (complete — no resurrection path).
        assert!(receipt.is_complete(), "the fan-out reached every namespace");
        assert_eq!(
            receipt.namespaces_invalidated.len(),
            CacheNamespace::ALL.len()
        );
        assert_eq!(receipt.entries_invalidated, 8, "2 entries × 4 namespaces");
        // The audited receipt is content-addressed (the 10.6 hash-chain link; Merkle seal = P-GA-20).
        assert_eq!(receipt.receipt.operation, "git.history_rewrite");
        assert!(receipt.receipt.content_hash.starts_with("blake3:"));
        // The op consumed exactly one rate-limit budget.
        assert_eq!(limiter.consumed_by(&tenant()), 1);
    }

    #[test]
    fn the_rewrite_runs_sandboxed_through_the_wire_executor_no_host_exec() {
        // The rewrite is a `filter-repo`-class canonical-git invocation through the sandbox seam —
        // it never shells out to the host (no-host-exec). The recorded argv proves the op ran
        // through `WireExecutor::run`, not a host `Command`.
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

    // ───────────────────────── rate-limited tenant op (recon §9) ────────────────────────────────

    #[test]
    fn the_rewrite_is_rate_limited_per_tenant_a_refusal_does_not_run() {
        // recon §9: the rewrite is a rate-limited tenant op. A budget of 1 admits one rewrite; the
        // second is REFUSED LOUD and does NOT run (no extra sandbox invocation, no fan-out).
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
        // The refused op did NOT run a second sandbox invocation.
        assert_eq!(
            tool.wire.ran.borrow().len(),
            1,
            "the refused rewrite never reached the sandbox"
        );
        // The budget was consumed exactly once (the refusal did not consume).
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
            "rewrites disabled — nothing ran"
        );
    }

    // ───────────────────────── fail-closed: an incomplete fan-out is RED ────────────────────────

    #[test]
    fn an_incomplete_fan_out_aborts_loud_so_no_cache_can_resurrect_the_bytes() {
        // If the clone-cache invalidation fails, the op is INCOMPLETE (a CDN clone could resurrect
        // the expunged bytes) — aborted LOUD, naming the missed namespace.
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
        // A non-zero rewrite exit ABORTS — no fan-out runs (the caches still point at valid old
        // bytes; we never invalidate caches for a rewrite that did not happen).
        let wire = RecordingWire::failing();
        let inv = RecordingInvalidator::all_ok();
        let tool = HistoryRewriteTool::new(wire, inv);
        let mut limiter = RewriteRateLimiter::new(5);
        let err = tool.rewrite(&plan(), &mut limiter, 1).unwrap_err();
        assert!(matches!(err, HistoryRewriteError::SandboxFailed(_)));
        // The fan-out never ran (the invalidator saw nothing).
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
        // An empty plan is rejected BEFORE the rate-limit — it consumes no budget and runs nothing.
        assert_eq!(limiter.consumed_by(&tenant()), 0);
        assert_eq!(tool.wire.ran.borrow().len(), 0);
    }

    #[test]
    fn the_receipt_is_complete_only_when_every_namespace_is_invalidated() {
        // Kills the `is_complete -> true` mutant: completeness requires every trust-scoped namespace.
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

    // ───────────────────────── the code-tool identity (the registration keys) ───────────────────

    #[test]
    fn the_code_tool_identity_constants_are_the_frozen_keys() {
        assert_eq!(GIT_SUBSYSTEM, "git");
        assert_eq!(HISTORY_REWRITE_TOOL, "history_rewrite");
        assert_eq!(SCIP_INDEX_TOOL, "scip_index");
        // the required_caps are built from the canonical Git ReBAC object types (4.9) — a rename
        // there is a compile/test break here, never a silent drift.
        assert_eq!(
            history_rewrite_required_caps(),
            vec!["repo.administer".to_string()]
        );
        assert_eq!(scip_index_required_caps(), vec!["repo.pull".to_string()]);
    }

    #[test]
    fn the_rate_limiter_remaining_countdown_is_exact() {
        // Kills the `max_per_window - *used` arithmetic mutants: each consume returns the EXACT
        // remaining budget (3 → 2 → 1 → 0), then refuses.
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
        // Kills the `Display::fmt -> Ok(default)` mutant: each error renders a non-empty, descriptive
        // message (never a swallowed empty string).
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
        // It rides the sandbox (a compute job); the argv is built here, run by ToolHands::exec.
    }
}
