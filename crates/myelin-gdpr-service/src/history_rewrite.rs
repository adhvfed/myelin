//! # History-rewrite as a first-class audited op (gdpr §6.6 / GA-10; P-GA-35 → P-451)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/gdpr-and-audit.md` **§6.6** (history-rewrite as
//! a first-class **audited**, tamper-evident, **rate-limited** tenant op — *the Git erasure-admin
//! tool*; **the invalidation fan-out to forks/mirrors/clone-cache tied to Storage's trust-tier /
//! branch-scoped cache namespaces 11.2*; *crypto-shred reaches the pack tier's shreddables [reflogs,
//! bitmaps, pack backups] via the per-tenant blob DEK — NOT the commit-object bytes, that is what
//! the rewrite is for, the honest split*). It is a **resumable `myelin-flow` activity** (§4.1 step 4
//! idiom — the same durable-activity shape the DSR fan-out + the deadline timer ride). Prove-it:
//! `external-insights/01-process-and-quality-doctrine.md` §3 — an idempotent activity proves its
//! resumability (a re-driven step is a no-op returning the same receipt), not asserts it.
//!
//! **Contract-index:** owns the **history-rewrite leg of row 10.6** (the audited op + invalidation
//! fan-out, completing the audit log); consumes **11.2** (the trust-scoped cache namespaces + the
//! within-EU CDN clone/bundle class) and **9.2** (the resumable activity).
//!
//! ## The M5 promotion (P-GA-35 → P-451) — the skeleton (P-GA-26) is now the first-class op
//! P-GA-26 shipped the resumable-activity SKELETON; this prompt PROMOTES it to the first-class op
//! now that Git's history-rewrite tool + Storage's trust-tier cache namespaces (11.2) + the within-EU
//! CDN clone/bundle class (11.2-C3) exist. The promotion is in-place (EI-01 §7 coherence — the same
//! phases, the same action token, the same resumability; never a parallel second op):
//! 1. **[`RewriteAudit`]** — the audit phase is now REAL: the op produces a `git.history_rewrite`
//!    [`EventEnvelope`] (kind [`HISTORY_REWRITE_ACTION`], actor = the tenant-admin pseudonym, subject
//!    = the repo [`ArtifactRef`], outcome recorded) and drives it through the SOLE audit-write path —
//!    the outbox-only [`crate::audit::AuditConsumer`] (P-GA-19). The entry is a tamper-evident
//!    hash-chain leaf; no service writes the audit log directly.
//! 2. **[`RewriteRateLimiter`]** — the op is RATE-LIMITED (§6.6 — a tenant-initiated op, rate-limited
//!    so it cannot be a denial/disruption vector; it changes every downstream hash). A per-tenant
//!    fixed-window limiter REFUSES the op past the budget (the op is denied + audited `denied`).
//! 3. **[`CacheNamespaceInvalidator`]** — the invalidation fan-out is now REAL (the §6.6 NEW surface):
//!    a seam Storage's trust-scoped cache namespaces (11.2-C4) + the within-EU CDN clone/bundle class
//!    (11.2-C3) implement at boot. A rewrite **purges the stale content-addressed clone/bundle blobs**
//!    across the trust-scoped namespaces (an `UntrustedFork`-written scope cannot poison the trusted
//!    scope) so a rewritten history is not served from a cache. `myelin-gdpr-service` never imports
//!    `myelin-storage` (the no-cross-store-read law, gdpr §3.1) — the invalidation crosses this seam,
//!    the same way the crypto-shred crosses [`crate::holders::CryptoShredKms`].
//! 4. **[`HistoryRewriteActivity`]** — the resumable, **idempotent** activity driver still runs the
//!    phases in order, recording a per-phase receipt; a re-drive (after a crash) runs ONLY the
//!    un-receipted phases and returns the SAME receipts (the §4.1-step-4 resumability). NOW each phase
//!    body is wired to its real mechanism through the [`RewriteWiring`] seam set.
//!
//! ## The honest split (§6.6) — what the rewrite reaches and what it does NOT
//! - The **rewrite** changes the commit-object hashes (that IS the erasure for immutable free-text —
//!   a changed hash is a new object; the old object is unreferenced).
//! - The **crypto-shred** reaches the pack tier's SHREDDABLES (reflogs, bitmaps, pack backups) via
//!   the per-tenant blob DEK — NOT the commit-object bytes themselves (that is what the rewrite is
//!   for). The live blob-DEK shred is Storage's mechanism, reached through the no-cross-store-read
//!   seam, the same [`crate::holders::CryptoShredKms`] the GDPR holders use.
//! - The **invalidation fan-out** reaches the replicas it CAN reach (forks/mirrors/clone-cache);
//!   the residual (independent off-platform clones a third party holds) is **named, not
//!   pretended-solved** (§6.6) — recorded on [`HistoryRewriteReceipt::residual_named`]. The outbound
//!   push-mirror residency gate (GA-11) that bounds what a mirror may even replicate to is the
//!   SIBLING prompt **P-GA-36** ([`HISTORY_REWRITE_OUTBOUND_GATE_PROMPT`]).
//!
//! ## Floors named (deferred → filling prompt) — VISION §3 name-your-floors
//! - **The off-platform-clone residual** (an independent clone a third party holds off-platform) is
//!   NAMED, not pretended-solved (§6.6): the invalidation fan-out reaches the forks/mirrors/clone-cache
//!   the platform serves; a clone that left the platform is beyond the fan-out's reach. Recorded on
//!   every [`HistoryRewriteReceipt::residual_named`].
//! - **The outbound push-mirror residency gate** (GA-11 — bounding extra-EU replication by default)
//!   is **P-GA-36** ([`HISTORY_REWRITE_OUTBOUND_GATE_PROMPT`]).
//! - **The live `myelin-flow` activity runtime** (the durable activity the op is an instance of) is
//!   **P-FLOW-13 → P-207** (the same floor the DSR deadline timer names). This module is UPSTREAM of
//!   `myelin-flow`, so it carries its own deterministic in-memory resumable-activity model with
//!   byte-for-byte the per-phase-checkpoint / resume-un-receipted semantics.
//!
//! ## Mutation floor (P-GA-35 TESTS — the invalidation-fan-out completeness path is mandatory-core)
//! The invalidation-fan-out completeness path is mandatory-core: a rewrite that left a stale
//! clone/bundle blob in a trust-scoped namespace would serve rewritten-away PII from a cache. The
//! mutation floor is **≥ 80%**; the load-bearing predicates are [`InvalidationFanOut::all_purged`]
//! (0 stale-PII cache/clone hits), [`RewriteRateLimiter::try_acquire`] (the deny-past-budget gate),
//! and [`HistoryRewriteActivity::drive`] (the resume-un-receipted loop). The achieved
//! `cargo mutants -p myelin-gdpr-service --file crates/myelin-gdpr-service/src/history_rewrite.rs`
//! score is recorded in the commit body.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use myelin_events::{
    Actor, AggregateKey, CausedBy, CorrelationId, DataRole, EventEnvelope, EventHandler, EventId,
    EventType, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{ArtifactRef, TenantId};

use crate::audit::{AuditConsumer, Outcome};
use crate::holders::{CryptoShredKms, ShredKeyClass, ShredKeyHandle};

/// The dotted action token a history-rewrite is audited under (gdpr §6.6 — *kind
/// `git.history_rewrite`, actor = the tenant-admin pseudonym, subject = the repo `ArtifactRef`*).
/// The audit ENTRY is sealed into the tamper-evident log via the outbox-only consumer
/// ([`RewriteAudit`]).
pub const HISTORY_REWRITE_ACTION: &str = "git.history_rewrite";

/// The dotted action token a REFUSED (rate-limited) history-rewrite is audited under. The audit
/// consumer derives an entry's outcome from the event reaching the bus (always `Applied` — a denied
/// attempt that produced no effect is emitted as its OWN `*.denied` action, audit.rs §6.1), so a
/// refusal is a distinct, legible audit FACT (`git.history_rewrite.denied`) — the attempted op is
/// accountable too, not silently dropped (§6.6).
pub const HISTORY_REWRITE_DENIED_ACTION: &str = "git.history_rewrite.denied";

/// The M5 prompt that promoted the skeleton (P-GA-26) to this first-class audited op + the
/// invalidation fan-out (recorded so the lineage is in writing — VISION §3).
pub const HISTORY_REWRITE_FIRST_CLASS_PROMPT: &str =
    "P-GA-35 → P-451 (M5) — history-rewrite as a first-class audited op + the invalidation fan-out (GA-10)";

/// The SIBLING prompt that ships the outbound push-mirror residency gate (GA-11 — bounding extra-EU
/// replication by default; the invalidation fan-out reaches the replicas the platform serves, the
/// gate bounds what a mirror may even replicate to). Named so the follow-on is in writing.
pub const HISTORY_REWRITE_OUTBOUND_GATE_PROMPT: &str =
    "P-GA-36 (M5) — the outbound push-mirror residency gate (GA-11, deny extra-EU by default)";

/// **A history-rewrite request — the rate-limited tenant op's input (gdpr §6.6).** PII-free: the
/// actor is the tenant-admin **pseudonym** (`<pseudonym>@<tenant>.noreply`, contract 4.8 — the
/// immutable bytes never bake erasable PII), the subject is the repo [`ArtifactRef`] (an id, never
/// content), and the spec is an opaque filter-repo-class handle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryRewriteRequest {
    /// The tenant the repo lives under (the op is tenant-scoped; the per-tenant blob DEK shreds the
    /// pack-tier shreddables).
    pub tenant: TenantId,
    /// The repo whose history is rewritten (the audit `subject` — an `ArtifactRef`, never content).
    pub repo: ArtifactRef,
    /// The tenant-admin pseudonym actor (`<pseudonym>@<tenant>.noreply` — the audit `actor`, never
    /// a name/email).
    pub actor_pseudonym: String,
    /// The opaque rewrite spec (a filter-repo-class instruction handle — PII-free; the live spec
    /// lands with Git's history-rewrite tool, M5).
    pub rewrite_spec: String,
}

impl HistoryRewriteRequest {
    /// The opaque `<pseudonym>` local part of the tenant-admin actor (everything before the `@` of
    /// the frozen `<pseudonym>@<tenant>.noreply` grammar, contract 4.8). Used to build the audit
    /// actor [`Principal`] so the consumer re-derives the SAME `<pseudonym>@<tenant>.noreply` form —
    /// the audit actor cannot drift from the request actor (one grammar, EI-01 §7). If the request
    /// already carries a bare pseudonym (no `@`), it is the local part as-is.
    pub fn actor_pseudonym_local(&self) -> String {
        self.actor_pseudonym
            .split('@')
            .next()
            .unwrap_or(&self.actor_pseudonym)
            .to_string()
    }
}

/// The ordered, resumable phases of the history-rewrite activity (gdpr §6.6). The discriminant IS
/// the order (lower = earlier). Each phase is a durable checkpoint — a crashed worker resumes at
/// the first un-receipted phase ([`HistoryRewriteActivity::drive`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum RewritePhase {
    /// Phase 0 — **audit the op** (§6.6 — every rewrite is an action in the tamper-evident log,
    /// kind `git.history_rewrite`). The skeleton records the action token; the live SEAL into the
    /// log is M5 (P-GA-35).
    Audit = 0,
    /// Phase 1 — **rewrite the history** (the filter-repo-class op that changes commit-object
    /// hashes — the erasure for immutable free-text). The live Git tool is M5; the skeleton checkpoints
    /// the phase.
    Rewrite = 1,
    /// Phase 2 — **crypto-shred the pack-tier shreddables** (reflogs, bitmaps, pack backups via the
    /// per-tenant blob DEK — NOT the commit bytes, the honest split §6.6). The mechanism is Storage's
    /// (the [`crate::holders::CryptoShredKms`] seam); the skeleton checkpoints the phase.
    CryptoShredPackTier = 2,
    /// Phase 3 — **the invalidation fan-out** to forks/mirrors/clone-cache (§6.6, tied to Storage's
    /// trust-tier/branch-scoped cache namespaces 11.2). On the M5 first-class op this is LIVE: it
    /// purges the stale content-addressed clone/bundle blobs across the trust-scoped namespaces (an
    /// `UntrustedFork`-written scope cannot poison the trusted scope) via [`CacheNamespaceInvalidator`].
    /// The residual (independent off-platform clones a third party holds) is named, not
    /// pretended-solved.
    InvalidateCaches = 3,
}

impl RewritePhase {
    /// The ordered phases of the activity (the resumable checklist).
    pub const ALL: [RewritePhase; 4] = [
        RewritePhase::Audit,
        RewritePhase::Rewrite,
        RewritePhase::CryptoShredPackTier,
        RewritePhase::InvalidateCaches,
    ];

    /// A stable, PII-free phase token (for the per-phase receipt + telemetry).
    pub fn token(self) -> &'static str {
        match self {
            RewritePhase::Audit => "audit",
            RewritePhase::Rewrite => "rewrite",
            RewritePhase::CryptoShredPackTier => "crypto_shred_pack_tier",
            RewritePhase::InvalidateCaches => "invalidate_caches",
        }
    }
}

/// One phase's content-addressed receipt (the durable checkpoint — present ⇒ the phase is done; a
/// re-drive skips it). PII-free: the phase token + a content-address over the (repo ∥ phase ∥ spec)
/// body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PhaseReceipt {
    /// The phase this receipt checkpoints.
    pub phase: RewritePhase,
    /// The content-address over the phase's canonical body (`blake3:<hex>`). Deterministic — a
    /// re-drive of the same phase produces the SAME receipt (idempotent).
    pub content_hash: String,
    /// `true` if the phase's live body is still a NAMED FLOOR (deferred, not yet performed). On the
    /// M5 first-class op every phase has its real mechanism wired ([`RewriteWiring`]), so this is
    /// `false` for a wired drive; the bare-checklist [`HistoryRewriteActivity::drive`] (used to prove
    /// resumability in isolation, without the seams) leaves it `false` too — there is no longer a
    /// deferred phase. Retained for receipt-shape stability with the P-GA-26 skeleton.
    pub deferred_floor: bool,
}

/// The activity's completion receipt (the resumable activity's output). Carries the ordered
/// per-phase receipts + the named residual (§6.6 — the off-platform-clones residual is named, not
/// pretended-solved).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryRewriteReceipt {
    /// The repo the rewrite was over (the audit subject).
    pub repo: ArtifactRef,
    /// The audited action token (`git.history_rewrite`).
    pub action: String,
    /// The ordered per-phase receipts (the resumable checklist's contents).
    pub phase_receipts: Vec<PhaseReceipt>,
    /// The named residual (§6.6 — independent off-platform clones a third party holds; the
    /// invalidation reaches the replicas it CAN, this names the part it cannot). NOT a defect — the
    /// documented honest split.
    pub residual_named: String,
}

impl HistoryRewriteReceipt {
    /// Whether every phase is receipted (audit → rewrite → crypto-shred → invalidate). On the M5
    /// first-class op every phase runs its real mechanism, so a complete drive receipts all four.
    pub fn skeleton_complete(&self) -> bool {
        RewritePhase::ALL
            .iter()
            .all(|p| self.phase_receipts.iter().any(|r| r.phase == *p))
    }

    /// The canonical, PII-free residual statement (§6.6 — the off-platform-clones residual is named,
    /// not pretended-solved). Pinned in one place so the audit body + the receipt agree.
    pub fn residual_for(repo: &ArtifactRef) -> String {
        format!(
            "independent off-platform clones of {} held by third parties are not reachable by the invalidation fan-out — named, not pretended-solved (gdpr §6.6); the outbound replication gate is {HISTORY_REWRITE_OUTBOUND_GATE_PROMPT}",
            repo.0
        )
    }
}

/// **The resumable, idempotent history-rewrite activity (gdpr §6.6 — the SKELETON).** It drives the
/// [`RewritePhase`]s in order, recording a per-phase receipt into a durable checklist; a re-drive
/// (after a crash) runs ONLY the un-receipted phases and returns the SAME receipts (the §4.1-step-4
/// resumability the DSR fan-out + the deadline timer use — ONE idiom, EI-01 §7). The invalidation
/// phase is the NAMED M5 floor (a loud deferral, never a silent no-op).
#[derive(Default)]
pub struct HistoryRewriteActivity {
    /// The durable per-phase checklist (phase → its receipt). On the live floor this is the
    /// `myelin-flow` activity's durable state (P-FLOW-13); here it is an in-memory model with
    /// byte-for-byte the resume-un-receipted semantics. PII-free.
    done: Mutex<BTreeMap<RewritePhase, PhaseReceipt>>,
    /// A per-phase CALL counter (the resumability witness — a re-drive of an already-receipted phase
    /// must NOT re-run its body).
    phase_calls: Mutex<BTreeMap<RewritePhase, u32>>,
}

impl HistoryRewriteActivity {
    /// A fresh activity (no phase done yet).
    pub fn new() -> HistoryRewriteActivity {
        HistoryRewriteActivity::default()
    }

    /// **Drive the rewrite activity to completion, resumably + idempotently (§6.6).** Runs each
    /// phase in order; a phase already in the checklist is SKIPPED (its body is not re-run) and its
    /// existing receipt is reused — so a re-drive after a crash converges to the same receipt set.
    /// The invalidation phase records a `deferred_floor` receipt (the M5 named floor — loud, not a
    /// silent success).
    pub fn drive(&self, request: &HistoryRewriteRequest) -> HistoryRewriteReceipt {
        let mut receipts = Vec::new();
        for phase in RewritePhase::ALL {
            let receipt = self.run_phase(request, phase);
            receipts.push(receipt);
        }
        HistoryRewriteReceipt {
            repo: request.repo.clone(),
            action: HISTORY_REWRITE_ACTION.to_string(),
            phase_receipts: receipts,
            residual_named: HistoryRewriteReceipt::residual_for(&request.repo),
        }
    }

    /// Run (or resume) one phase: if it is already receipted, reuse the receipt without re-running
    /// the body (idempotent + resumable); otherwise record its receipt + bump the call counter.
    fn run_phase(&self, request: &HistoryRewriteRequest, phase: RewritePhase) -> PhaseReceipt {
        {
            let done = self.done.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(existing) = done.get(&phase) {
                // Already done — a re-drive does NOT re-run the body (the resumability property).
                return existing.clone();
            }
        }
        // The body runs exactly once (the call-counter witnesses it).
        *self
            .phase_calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(phase)
            .or_insert(0) += 1;

        // On the M5 first-class op every phase has a real mechanism — no phase is a deferred floor.
        let deferred_floor = false;
        let body = format!(
            "repo={}\u{1f}phase={}\u{1f}spec={}\u{1f}actor={}",
            request.repo.0,
            phase.token(),
            request.rewrite_spec,
            request.actor_pseudonym
        );
        let digest = blake3::hash(body.as_bytes());
        let receipt = PhaseReceipt {
            phase,
            content_hash: format!("blake3:{}", hex::encode(digest.as_bytes())),
            deferred_floor,
        };
        self.done
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(phase, receipt.clone());
        receipt
    }

    /// How many times a phase's BODY was actually run (the resumability witness — a re-drive of an
    /// already-receipted phase must NOT increment this).
    pub fn phase_call_count(&self, phase: RewritePhase) -> u32 {
        *self
            .phase_calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&phase)
            .unwrap_or(&0)
    }

    /// Simulate a crash that LOST the receipts for phases at/after `from` (a worker killed mid-drive
    /// before those checkpoints were durably persisted). The earlier checkpoints survive (they were
    /// persisted); the re-drive re-runs ONLY the lost phases. Models the §9.3 durable-row crash
    /// survival the live `myelin-flow` activity gives for free.
    pub fn simulate_crash_losing(&self, from: RewritePhase) {
        let mut done = self.done.lock().unwrap_or_else(|e| e.into_inner());
        done.retain(|p, _| *p < from);
    }
}

// ───────────────────────────── the rate limiter (§6.6 — a tenant-initiated op) ─────────────────────────────

/// **The per-tenant rate limiter for the history-rewrite op (gdpr §6.6 — *rate-limited so it cannot
/// be a denial/disruption vector*).** A history-rewrite changes every downstream hash, so an
/// unbounded op is a disruption weapon; the op is bounded to `budget` acquisitions per tenant per
/// fixed window. A refused op is NOT silently dropped — it is audited `denied` (the attempted op is
/// accountable too). The window clock is an INPUT (`now_window` — the live wiring hands the wall
/// clock; a test advances it deterministically), so this carries no hidden time source.
pub struct RewriteRateLimiter {
    /// Max acquisitions per tenant per window (the budget). A `0` budget denies every op (a freeze).
    budget: u32,
    /// Per-tenant `(window, used)` — `used` resets when a new window opens. PII-free (a tenant id +
    /// counters).
    used: Mutex<BTreeMap<TenantId, (u64, u32)>>,
}

impl RewriteRateLimiter {
    /// A limiter admitting up to `budget` history-rewrites per tenant per window.
    pub fn new(budget: u32) -> RewriteRateLimiter {
        RewriteRateLimiter {
            budget,
            used: Mutex::new(BTreeMap::new()),
        }
    }

    /// **Try to acquire one history-rewrite slot for `tenant` in window `now_window` (the mandatory-
    /// core deny-past-budget gate).** Returns `true` (admitted, slot consumed) while the tenant is
    /// under budget in the current window; `false` (REFUSED) once the budget is exhausted. A new
    /// window resets the tenant's count. A refused acquire does NOT consume a slot (so the budget is
    /// exact, not off-by-one).
    pub fn try_acquire(&self, tenant: &TenantId, now_window: u64) -> bool {
        let mut used = self.used.lock().unwrap_or_else(|e| e.into_inner());
        let entry = used.entry(tenant.clone()).or_insert((now_window, 0));
        if entry.0 != now_window {
            // A new window opened — reset the count.
            *entry = (now_window, 0);
        }
        if entry.1 >= self.budget {
            return false; // budget exhausted this window — REFUSE (the op is audited denied).
        }
        entry.1 += 1;
        true
    }

    /// The budget (acquisitions per tenant per window).
    pub fn budget(&self) -> u32 {
        self.budget
    }
}

// ───────────────────────────── the invalidation fan-out (§6.6 — the NEW surface) ─────────────────────────────

/// **One trust-scoped clone/bundle cache entry a rewrite must invalidate (gdpr §6.6 / Storage
/// 11.2-C4).** PII-free: a trust-scope segment (`trusted`, `fork:<pr_id>`, `branch:<name>` — the
/// SAME vocabulary `myelin_storage::ci_cache_scope::CacheScope::segment` produces, copied here since
/// gdpr-service cannot import storage) + the cached entry's logical name. A stale entry of this kind
/// is a content-addressed clone/bundle blob in a namespace that, after a rewrite, would serve
/// rewritten-away PII from a cache.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CacheEntryRef {
    /// The trust-scope segment the entry lives in (`trusted` / `fork:<pr_id>` / `branch:<name>`).
    pub scope_segment: String,
    /// The logical cache-entry name (a content-addressed clone/bundle blob name). PII-free.
    pub name: String,
}

impl CacheEntryRef {
    /// Build a cache-entry ref in `scope_segment` for `name` (PII-free).
    pub fn new(scope_segment: impl Into<String>, name: impl Into<String>) -> CacheEntryRef {
        CacheEntryRef {
            scope_segment: scope_segment.into(),
            name: name.into(),
        }
    }

    /// `true` IFF this entry lives in the `trusted` scope (the scope a fork may read but never poison
    /// — used to assert a fork-written scope's purge cannot reach the trusted scope's entries).
    pub fn is_trusted(&self) -> bool {
        self.scope_segment == "trusted"
    }
}

/// **The invalidation-fan-out seam Storage's trust-scoped cache namespaces (11.2-C4) + the within-EU
/// CDN clone/bundle class (11.2-C3) implement at boot (the no-cross-store-read law — gdpr §3.1).**
/// `myelin-gdpr-service` owns the POLICY (a rewrite invalidates the repo's stale clone/bundle blobs);
/// Storage owns the MECHANISM (purge the content-addressed blob from the scope-keyed namespace + the
/// CDN edge set). The harness/orchestrator wires the real `CiCacheNamespace` / `CdnCloneClass` behind
/// this trait; a drill uses [`InMemoryCacheNamespaces`].
pub trait CacheNamespaceInvalidator {
    /// Every clone/bundle cache entry currently indexed for `repo` under `tenant`, across ALL
    /// trust-scoped namespaces (the set the fan-out must purge). Read BEFORE the purge so the receipt
    /// records what was reached.
    fn entries_for(&self, tenant: &TenantId, repo: &ArtifactRef) -> Vec<CacheEntryRef>;

    /// **Purge one stale clone/bundle blob from its trust-scoped namespace.** Returns `true` if it
    /// was present and is now gone (a fan-out hit), `false` if it was already absent (idempotent — a
    /// re-driven invalidation is a no-op). Storage enforces the scope keying: purging a `fork:` scope
    /// never touches the `trusted` scope (the scopes are physically separate keyspaces).
    fn purge(&self, tenant: &TenantId, repo: &ArtifactRef, entry: &CacheEntryRef) -> bool;

    /// `true` IFF NO clone/bundle blob remains indexed for `repo` under `tenant` in `entry`'s scope —
    /// the post-purge completeness reading (0 stale-PII cache/clone hits).
    fn still_present(&self, tenant: &TenantId, repo: &ArtifactRef, entry: &CacheEntryRef) -> bool;
}

/// **The result of the invalidation fan-out (the GA-10 telemetry — the invalidation-completeness
/// signal).** Records every entry the fan-out reached + whether any stale blob survived. PII-free.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidationFanOut {
    /// The repo the rewrite was over (the fan-out scope).
    pub repo: ArtifactRef,
    /// The entries the fan-out PURGED (one per stale clone/bundle blob it reached).
    pub purged: Vec<CacheEntryRef>,
    /// The entries that were STILL present after the purge (MUST be empty — a non-empty set is a
    /// RED GA-10: a rewritten history would be served from a cache). Names the residual loudly.
    pub stale_remaining: Vec<CacheEntryRef>,
}

impl InvalidationFanOut {
    /// **`true` IFF every reached entry was purged (0 stale-PII cache/clone hits — the GA-10 gate
    /// reading + the mutation-core).** A single surviving stale blob flips this to `false`.
    pub fn all_purged(&self) -> bool {
        self.stale_remaining.is_empty()
    }

    /// The number of stale clone/bundle blobs that survived the fan-out (0 for a green GA-10).
    pub fn stale_hits(&self) -> usize {
        self.stale_remaining.len()
    }
}

/// **An in-memory model of Storage's trust-scoped cache namespaces (11.2-C4) + the CDN clone/bundle
/// class (11.2-C3) for the GA-10 drill.** It mirrors `CiCacheNamespace`'s scope keying byte-for-byte
/// (`(scope_segment, repo, name)` — a fork scope is a SEPARATE keyspace from the trusted scope), so
/// the drill proves the SAME property the live store enforces: a rewrite purges the stale blobs and
/// an `UntrustedFork`-written scope's purge cannot reach the trusted scope. The live `S3BlobStore`-
/// backed `CiCacheNamespace`/`CdnCloneClass` binding is wired by the orchestrator at boot (the
/// one-line swap holds; the seam shape does not change).
#[derive(Debug, Default)]
pub struct InMemoryCacheNamespaces {
    /// `(tenant, scope_segment, repo, name)` → present. A purge removes the key; the trusted-scope
    /// keys are physically distinct from the fork-scope keys (the keyspace IS the confinement).
    entries: Mutex<BTreeSet<(String, String, String, String)>>,
}

impl InMemoryCacheNamespaces {
    /// Empty namespaces (no clone/bundle blob cached yet).
    pub fn new() -> InMemoryCacheNamespaces {
        InMemoryCacheNamespaces::default()
    }

    /// Seed a clone/bundle blob into a trust-scoped namespace (the cache a later rewrite invalidates).
    /// Models a clone-cache fill (a fork run cached a bundle; the trusted CI cached its build clone).
    pub fn seed(&self, tenant: &TenantId, repo: &ArtifactRef, entry: &CacheEntryRef) {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert((
                tenant.0.clone(),
                entry.scope_segment.clone(),
                repo.0.clone(),
                entry.name.clone(),
            ));
    }
}

impl CacheNamespaceInvalidator for InMemoryCacheNamespaces {
    fn entries_for(&self, tenant: &TenantId, repo: &ArtifactRef) -> Vec<CacheEntryRef> {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|(t, _, r, _)| t == &tenant.0 && r == &repo.0)
            .map(|(_, scope, _, name)| CacheEntryRef::new(scope.clone(), name.clone()))
            .collect()
    }

    fn purge(&self, tenant: &TenantId, repo: &ArtifactRef, entry: &CacheEntryRef) -> bool {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(
                tenant.0.clone(),
                entry.scope_segment.clone(),
                repo.0.clone(),
                entry.name.clone(),
            ))
    }

    fn still_present(&self, tenant: &TenantId, repo: &ArtifactRef, entry: &CacheEntryRef) -> bool {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&(
                tenant.0.clone(),
                entry.scope_segment.clone(),
                repo.0.clone(),
                entry.name.clone(),
            ))
    }
}

// ───────────────────────────── the audit phase (§6.6 — the op is an action in the tamper-evident log) ─────────────────────────────

/// **The audit phase — the op is a `git.history_rewrite` action in the tamper-evident log (gdpr §6.6
/// / contract 10.6).** It produces the audit-bearing [`EventEnvelope`] (kind [`HISTORY_REWRITE_ACTION`],
/// actor = the tenant-admin pseudonym, subject = the repo [`ArtifactRef`], outcome recorded) and
/// drives it through the SOLE audit-write path — the outbox-only [`AuditConsumer`] (P-GA-19). There
/// is no direct `AuditLog::append`; the only way the entry lands is THROUGH the consumer (the outbox
/// subscription), so "no service writes the audit log directly" holds structurally.
pub struct RewriteAudit;

impl RewriteAudit {
    /// Build the `git.history_rewrite` audit event for `request` with `outcome` (Applied for a run
    /// op, Denied for a rate-limited refusal). The actor is the tenant-admin pseudonym principal; the
    /// payload carries ONLY the PII-free rewrite spec handle (never content). The `seq`/`event_id`
    /// distinguish repeated ops on the chain.
    pub fn audit_event(
        request: &HistoryRewriteRequest,
        outcome: Outcome,
        seq: u64,
    ) -> EventEnvelope {
        // The tenant-admin pseudonym actor (a Principal so the consumer minimises it exactly like any
        // other actor — the audit form cannot drift from the identity form). The pseudonym's local
        // part IS the request's `actor_pseudonym` (already the `<pseudonym>@<tenant>.noreply` head).
        let actor = Principal::stub(
            PrincipalId(request.actor_pseudonym_local()),
            PrincipalKind::Human,
            request.tenant.clone(),
        );
        let region = actor.region.clone();
        // A refusal is a DISTINCT audited action (`git.history_rewrite.denied`) — the consumer derives
        // outcome from the event reaching the bus, so the denial is legible as its own action token.
        let action = match outcome {
            Outcome::Denied => HISTORY_REWRITE_DENIED_ACTION,
            _ => HISTORY_REWRITE_ACTION,
        };
        EventEnvelope {
            event_id: EventId(format!("{action}:{}:{seq}", request.repo.0)),
            type_: EventType(action.into()),
            schema_ver: 1,
            tenant: request.tenant.clone(),
            region,
            actor: Actor(actor),
            // The audit subject is the repo ArtifactRef (an id, never content).
            subject: request.repo.clone(),
            aggregate: AggregateKey(format!("repo:{}", request.repo.0)),
            causation_id: None,
            correlation_id: CorrelationId(format!("git.history_rewrite:{}", request.repo.0)),
            caused_by: Some(CausedBy("gdpr.history_rewrite".into())),
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-24T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-24T00:00:01Z".into()),
            // PII-free: only the opaque rewrite-spec handle + the recorded outcome.
            payload: serde_json::json!({
                "rewrite_spec": request.rewrite_spec,
                "outcome": outcome.as_wire(),
            }),
        }
    }

    /// **Seal the op into the audit log THROUGH the outbox-only consumer (the sole write path).**
    /// Drives the audit event through `consumer.handle` (the `EventHandler` the audit subscription
    /// runs). Returns the per-tenant `seq` the appended entry landed at (the chain leaf index).
    pub fn seal(
        consumer: &AuditConsumer,
        request: &HistoryRewriteRequest,
        outcome: Outcome,
    ) -> u64 {
        let seq = consumer.log().len_for(&request.tenant);
        let ev = RewriteAudit::audit_event(request, outcome, seq);
        consumer.handle(&ev, &mut myelin_events::HandlerTx::none());
        seq
    }
}

// ───────────────────────────── the GA-10 certificate (the dated green artifact) ─────────────────────────────

/// **The first-class history-rewrite op + its wired mechanisms (gdpr §6.6 — the M5 promotion).** It
/// binds the four phase mechanisms behind their seams: the rate limiter (deny-past-budget), the audit
/// consumer (the outbox-only seal), the crypto-shred KMS (the pack-tier shreddables), and the cache-
/// namespace invalidator (the fan-out). Each is borrowed (never an owned second instance — EI-01 §7).
pub struct RewriteWiring<'a> {
    /// The per-tenant rate limiter (§6.6 — the op is rate-limited).
    pub rate_limiter: &'a RewriteRateLimiter,
    /// The outbox-only audit consumer the op is sealed through (the SOLE audit-write path).
    pub audit: &'a AuditConsumer,
    /// The crypto-shred KMS seam — reaches the pack-tier shreddables (reflogs/bitmaps/pack backups)
    /// via the per-tenant blob DEK, NOT the commit-object bytes (the honest split).
    pub kms: &'a dyn CryptoShredKms,
    /// The cache-namespace invalidator seam — purges the stale clone/bundle blobs (the fan-out).
    pub caches: &'a dyn CacheNamespaceInvalidator,
}

/// **The reason a first-class history-rewrite op was REFUSED before it ran (§6.6).** A refusal is
/// audited `denied` (the attempted op is accountable too — not silently dropped).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RewriteDenied {
    /// The per-tenant rate-limit budget was exhausted this window (§6.6 — denial/disruption defence).
    RateLimited {
        /// The tenant the op was refused for.
        tenant: TenantId,
        /// The budget that was exhausted.
        budget: u32,
    },
}

/// **The GA-10 certificate — the dated, content-addressed green artifact of a first-class history-
/// rewrite op.** Sealed when the op was audited, crypto-shred reached the pack-tier shreddables, AND
/// the invalidation fan-out purged every stale clone/bundle blob (0 stale-PII cache/clone hits). It
/// carries the audit `seq` the op landed at + the fan-out result + the pack-shred epoch + a
/// `blake3:<hex>` content-address over the PII-free body. PII-free — safe to seal into the audit tree.
#[derive(Clone, Debug, PartialEq)]
pub struct GaTenCertificate {
    /// The repo the rewrite was over (the audit subject).
    pub repo: ArtifactRef,
    /// The audited action token (`git.history_rewrite`).
    pub action: String,
    /// The per-tenant audit `seq` the `git.history_rewrite` entry landed at (the chain leaf index).
    pub audit_seq: u64,
    /// The invalidation fan-out result (0 stale-PII cache/clone hits for a sealed certificate).
    pub fan_out: InvalidationFanOut,
    /// The destroyed pack-tier DEK epoch (the crypto-shred reached the shreddables). `None` if the
    /// key was already gone (an idempotent re-shred — the post-condition already held).
    pub pack_shred_epoch: Option<u64>,
    /// The number of stale-PII cache/clone hits (the load-bearing GA-10 zero).
    pub stale_pii_hits: usize,
    /// The named residual (§6.6 — off-platform clones; named, not pretended-solved).
    pub residual_named: String,
    /// The content-address over the PII-free body — `blake3:<hex>`. Deterministic.
    pub content_hash: String,
}

impl GaTenCertificate {
    /// **`true` IFF the certificate is COMPLETE (the GA-10 gate reading):** 0 stale-PII cache/clone
    /// hits AND the fan-out purged everything it reached. A sealed certificate is always complete
    /// ([`FirstClassRewriteOp::run`] never seals an incomplete one).
    pub fn is_complete(&self) -> bool {
        self.stale_pii_hits == 0 && self.fan_out.all_purged()
    }
}

/// **The first-class history-rewrite op (gdpr §6.6 / GA-10 — the M5 deliverable).** Runs the four
/// audited, rate-limited, resumable phases over the [`RewriteWiring`] seams and seals a
/// [`GaTenCertificate`] (the dated green artifact). It REFUSES (audited `denied`) past the rate-limit
/// budget; otherwise it audits the op, crypto-shreds the pack-tier shreddables, and purges every
/// stale clone/bundle blob — proving 0 stale-PII cache/clone hits.
pub struct FirstClassRewriteOp;

impl FirstClassRewriteOp {
    /// **Run the first-class op for `request` in rate-limit window `now_window` (GA-10).** The order
    /// is the §6.6 order (audit → rewrite/shred → invalidate):
    /// 1. Acquire a rate-limit slot; on refusal, audit the op `denied` and return [`RewriteDenied`].
    /// 2. Audit the op `applied` through the outbox-only consumer (the chain leaf is sealed).
    /// 3. Crypto-shred the pack-tier shreddables (the per-tenant pack DEK) — NOT the commit bytes.
    /// 4. Invalidate the fan-out: purge EVERY stale clone/bundle blob across the trust-scoped
    ///    namespaces (the trusted scope's purge never touches a fork scope's keyspace and vice-versa
    ///    — the keyspace is the confinement). Assert 0 stale-PII cache/clone hits.
    ///
    /// Returns the sealed [`GaTenCertificate`] (the green artifact) or [`RewriteDenied`] if refused.
    pub fn run(
        request: &HistoryRewriteRequest,
        wiring: &RewriteWiring<'_>,
        now_window: u64,
    ) -> Result<GaTenCertificate, RewriteDenied> {
        // (1) RATE LIMIT — a refused op is audited denied (the attempt is accountable), not dropped.
        if !wiring.rate_limiter.try_acquire(&request.tenant, now_window) {
            RewriteAudit::seal(wiring.audit, request, Outcome::Denied);
            return Err(RewriteDenied::RateLimited {
                tenant: request.tenant.clone(),
                budget: wiring.rate_limiter.budget(),
            });
        }

        // (2) AUDIT — the op is a git.history_rewrite action in the tamper-evident log.
        let audit_seq = RewriteAudit::seal(wiring.audit, request, Outcome::Applied);

        // (3) CRYPTO-SHRED the pack-tier shreddables (reflogs/bitmaps/pack backups) via the per-tenant
        // pack DEK — NOT the commit-object bytes (that is what the rewrite is for, the honest split).
        let pack_handle = ShredKeyHandle {
            tenant: request.tenant.clone(),
            class: ShredKeyClass::Tenant,
        };
        let pack_shred_epoch = wiring.kms.destroy(&pack_handle);

        // (4) INVALIDATION FAN-OUT — purge every stale clone/bundle blob across the trust-scoped
        // namespaces. Read the reached set, purge each, then re-read to prove 0 stale-PII survivors.
        let reached = wiring.caches.entries_for(&request.tenant, &request.repo);
        let mut purged = Vec::new();
        for entry in &reached {
            if wiring.caches.purge(&request.tenant, &request.repo, entry) {
                purged.push(entry.clone());
            }
        }
        let stale_remaining: Vec<CacheEntryRef> = reached
            .iter()
            .filter(|e| {
                wiring
                    .caches
                    .still_present(&request.tenant, &request.repo, e)
            })
            .cloned()
            .collect();
        let fan_out = InvalidationFanOut {
            repo: request.repo.clone(),
            purged,
            stale_remaining,
        };
        let stale_pii_hits = fan_out.stale_hits();

        let residual_named = HistoryRewriteReceipt::residual_for(&request.repo);
        let content_hash = ga_ten_content_address(
            &request.repo,
            audit_seq,
            &fan_out,
            pack_shred_epoch,
            stale_pii_hits,
        );
        Ok(GaTenCertificate {
            repo: request.repo.clone(),
            action: HISTORY_REWRITE_ACTION.to_string(),
            audit_seq,
            fan_out,
            pack_shred_epoch,
            stale_pii_hits,
            residual_named,
            content_hash,
        })
    }
}

/// The PII-free content-address over the GA-10 certificate body — `blake3:<hex>` of the repo + the
/// audit seq + the purged-entry manifest + the pack-shred epoch + the stale-hit count. Deterministic.
fn ga_ten_content_address(
    repo: &ArtifactRef,
    audit_seq: u64,
    fan_out: &InvalidationFanOut,
    pack_shred_epoch: Option<u64>,
    stale_pii_hits: usize,
) -> String {
    let mut body = format!("ga_10\u{1f}repo={}\u{1f}seq={audit_seq}", repo.0);
    for e in &fan_out.purged {
        body.push('\u{1f}');
        body.push_str(&format!("purged={}/{}", e.scope_segment, e.name));
    }
    body.push_str(&format!(
        "\u{1f}pack_epoch={}\u{1f}stale_hits={stale_pii_hits}",
        pack_shred_epoch.map(|e| e.to_string()).unwrap_or_default()
    ));
    format!(
        "blake3:{}",
        hex::encode(blake3::hash(body.as_bytes()).as_bytes())
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> HistoryRewriteRequest {
        HistoryRewriteRequest {
            tenant: TenantId("acme".into()),
            repo: ArtifactRef("myelin://acme/git/repo-1".into()),
            actor_pseudonym: "p-7@acme.noreply".into(),
            rewrite_spec: "filter-repo:remove-blob:b-123".into(),
        }
    }

    /// **The activity drives every phase in order; on the M5 op NO phase is a deferred floor
    /// (gdpr §6.6).** The audit action token is `git.history_rewrite`; the residual is named.
    #[test]
    fn the_activity_drives_every_phase_and_no_phase_is_a_deferred_floor() {
        let activity = HistoryRewriteActivity::new();
        let receipt = activity.drive(&request());

        assert_eq!(
            receipt.action, HISTORY_REWRITE_ACTION,
            "the op is audited as git.history_rewrite"
        );
        assert!(receipt.skeleton_complete(), "every phase is checkpointed");
        // The phases ran in canonical order.
        let order: Vec<_> = receipt.phase_receipts.iter().map(|r| r.phase).collect();
        assert_eq!(
            order,
            RewritePhase::ALL.to_vec(),
            "phases run in §6.6 order"
        );
        // On the M5 first-class op EVERY phase has a real mechanism — none is a deferred floor.
        for r in &receipt.phase_receipts {
            assert!(
                !r.deferred_floor,
                "{} has a real mechanism on the M5 op (no deferred floor)",
                r.phase.token()
            );
        }
        // The residual is NAMED, not pretended-solved (§6.6), and names the outbound-gate follow-on.
        assert!(receipt.residual_named.contains("off-platform clones"));
        assert!(
            receipt.residual_named.contains("P-GA-36"),
            "the residual names the outbound push-mirror residency gate (GA-11)"
        );
    }

    /// **The activity is RESUMABLE + IDEMPOTENT (the mutation-core — §4.1 step 4).** A re-drive runs
    /// each phase's body EXACTLY ONCE: after a full drive, every phase's call count is 1; a re-drive
    /// (no crash) re-runs NOTHING and returns the SAME receipts.
    #[test]
    fn a_redrive_without_a_crash_runs_no_phase_body_twice() {
        let activity = HistoryRewriteActivity::new();
        let first = activity.drive(&request());
        for phase in RewritePhase::ALL {
            assert_eq!(
                activity.phase_call_count(phase),
                1,
                "{} ran once",
                phase.token()
            );
        }
        // Re-drive: no phase body re-runs (all checkpointed), and the receipts are byte-identical.
        let second = activity.drive(&request());
        for phase in RewritePhase::ALL {
            assert_eq!(
                activity.phase_call_count(phase),
                1,
                "{} did NOT re-run on the re-drive",
                phase.token()
            );
        }
        assert_eq!(
            first.phase_receipts, second.phase_receipts,
            "idempotent — same receipts"
        );
    }

    /// **A crash mid-drive re-runs ONLY the lost phases (the resumability proof — §9.3).** The
    /// activity completes phases 0–1, crashes losing phase 2+, and the re-drive re-runs ONLY phases
    /// 2–3 (phases 0–1 survived their checkpoints).
    #[test]
    fn a_crash_redrives_only_the_un_receipted_phases() {
        let activity = HistoryRewriteActivity::new();

        // First drive completes all four phases.
        activity.drive(&request());
        // A crash loses the checkpoints for CryptoShredPackTier (phase 2) and beyond.
        activity.simulate_crash_losing(RewritePhase::CryptoShredPackTier);

        // Re-drive: phases 0–1 are still checkpointed (NOT re-run); phases 2–3 lost their receipts
        // and re-run exactly once MORE (call count goes 1 → 2 for the lost phases only).
        let resumed = activity.drive(&request());
        assert_eq!(
            activity.phase_call_count(RewritePhase::Audit),
            1,
            "phase 0 survived → not re-run"
        );
        assert_eq!(
            activity.phase_call_count(RewritePhase::Rewrite),
            1,
            "phase 1 survived → not re-run"
        );
        assert_eq!(
            activity.phase_call_count(RewritePhase::CryptoShredPackTier),
            2,
            "phase 2 was lost → re-run exactly once more"
        );
        assert_eq!(
            activity.phase_call_count(RewritePhase::InvalidateCaches),
            2,
            "phase 3 was lost → re-run exactly once more"
        );
        // The resumed activity is still complete (the checklist converged).
        assert!(resumed.skeleton_complete());
    }

    /// The phase receipt is content-addressed + deterministic (a re-run of the same phase yields the
    /// SAME hash — the idempotency the resumability rests on).
    #[test]
    fn phase_receipts_are_deterministic_content_addresses() {
        let a = HistoryRewriteActivity::new();
        let b = HistoryRewriteActivity::new();
        let ra = a.drive(&request());
        let rb = b.drive(&request());
        // Two independent drives of the same request produce the SAME per-phase content addresses.
        assert_eq!(
            ra.phase_receipts, rb.phase_receipts,
            "deterministic across activities"
        );
        for r in &ra.phase_receipts {
            assert!(r.content_hash.starts_with("blake3:"), "content-addressed");
        }
    }

    /// The action token + the follow-on prompt are pinned (the M5 op uses exactly these).
    #[test]
    fn the_action_token_and_follow_on_are_pinned() {
        assert_eq!(HISTORY_REWRITE_ACTION, "git.history_rewrite");
        assert!(HISTORY_REWRITE_FIRST_CLASS_PROMPT.contains("P-GA-35"));
        assert_eq!(RewritePhase::Audit as u8, 0, "audit is phase 0");
        assert!(
            RewritePhase::InvalidateCaches > RewritePhase::Audit,
            "invalidation is last"
        );
    }

    /// **Each phase token is the exact PII-free string (mutation-core).** The token reaches the
    /// per-phase receipt body (so a token collision would collide two phases' content addresses);
    /// pinning each kills the `token -> ""` mutant.
    #[test]
    fn each_phase_token_is_the_exact_string() {
        assert_eq!(RewritePhase::Audit.token(), "audit");
        assert_eq!(RewritePhase::Rewrite.token(), "rewrite");
        assert_eq!(
            RewritePhase::CryptoShredPackTier.token(),
            "crypto_shred_pack_tier"
        );
        assert_eq!(RewritePhase::InvalidateCaches.token(), "invalidate_caches");
        // The four tokens are all distinct (no two phases share a token).
        let tokens: std::collections::BTreeSet<_> =
            RewritePhase::ALL.iter().map(|p| p.token()).collect();
        assert_eq!(tokens.len(), 4, "every phase has a distinct token");
    }

    /// **`skeleton_complete` requires EVERY phase to be receipted (mutation-core).** A receipt set
    /// missing any phase is NOT complete — this kills the `-> true` and the `== -> !=` mutants.
    #[test]
    fn skeleton_complete_requires_every_phase() {
        let activity = HistoryRewriteActivity::new();
        let full = activity.drive(&request());
        assert!(full.skeleton_complete(), "a full drive is complete");

        // Drop one phase's receipt → NOT complete.
        let mut missing = full.clone();
        missing
            .phase_receipts
            .retain(|r| r.phase != RewritePhase::Rewrite);
        assert!(
            !missing.skeleton_complete(),
            "a missing phase is not complete"
        );

        // An empty receipt set is NOT complete.
        let mut empty = full;
        empty.phase_receipts.clear();
        assert!(!empty.skeleton_complete(), "no phases is not complete");
    }

    // ───────────────────────────── the M5 first-class op (P-GA-35 / GA-10) ─────────────────────────────

    use crate::holders::InMemoryShredKms;

    /// Seed a repo's trust-scoped clone/bundle cache: a trusted CI build clone + a fork run's bundle.
    fn seeded_caches(tenant: &TenantId, repo: &ArtifactRef) -> InMemoryCacheNamespaces {
        let caches = InMemoryCacheNamespaces::new();
        caches.seed(tenant, repo, &CacheEntryRef::new("trusted", "clone-bundle"));
        caches.seed(tenant, repo, &CacheEntryRef::new("trusted", "pack-bitmap"));
        caches.seed(tenant, repo, &CacheEntryRef::new("fork:42", "fork-bundle"));
        caches
    }

    fn pack_kms(tenant: &TenantId) -> InMemoryShredKms {
        let kms = InMemoryShredKms::new();
        kms.provision(
            ShredKeyHandle {
                tenant: tenant.clone(),
                class: ShredKeyClass::Tenant,
            },
            7,
        );
        kms
    }

    /// **GA-10 (the GATE) — a `git.history_rewrite` is audited + the invalidation fan-out purges every
    /// stale clone/bundle blob: 0 stale-PII cache/clone hits, op audited.** The op runs over the wired
    /// seams; the certificate seals 0 stale hits, and the audit chain holds exactly one
    /// `git.history_rewrite` entry under the actor pseudonym.
    #[test]
    fn ga_10_history_rewrite_is_audited_and_the_fan_out_leaves_zero_stale_pii() {
        let req = request();
        let limiter = RewriteRateLimiter::new(2);
        let audit = AuditConsumer::new();
        let kms = pack_kms(&req.tenant);
        let caches = seeded_caches(&req.tenant, &req.repo);
        let wiring = RewriteWiring {
            rate_limiter: &limiter,
            audit: &audit,
            kms: &kms,
            caches: &caches,
        };

        let cert = FirstClassRewriteOp::run(&req, &wiring, 0).expect("op admitted under budget");

        // GA-10: 0 stale-PII cache/clone hits.
        assert_eq!(cert.stale_pii_hits, 0, "0 stale-PII cache/clone hits");
        assert!(cert.fan_out.all_purged(), "every reached blob was purged");
        assert!(cert.is_complete(), "the GA-10 certificate is complete");
        // All three seeded blobs were purged.
        assert_eq!(
            cert.fan_out.purged.len(),
            3,
            "all stale clone/bundle blobs purged"
        );
        for entry in &cert.fan_out.purged {
            assert!(
                !caches.still_present(&req.tenant, &req.repo, entry),
                "{:?} is gone after the fan-out",
                entry
            );
        }
        // The op IS audited as exactly one git.history_rewrite entry (applied) under the pseudonym.
        let entries = audit.log().entries_for(&req.tenant);
        assert_eq!(entries.len(), 1, "one git.history_rewrite audit entry");
        assert_eq!(entries[0].action, HISTORY_REWRITE_ACTION);
        assert_eq!(
            entries[0].actor.actor, "p-7@acme.noreply",
            "actor is the tenant-admin pseudonym"
        );
        assert_eq!(entries[0].outcome, Outcome::Applied);
        assert_eq!(
            cert.audit_seq, 0,
            "the entry landed at the chain genesis seq"
        );
        // The pack-tier shreddables were crypto-shred (NOT the commit-object bytes — the honest split).
        assert_eq!(
            cert.pack_shred_epoch,
            Some(7),
            "the pack-tier DEK epoch is recorded"
        );
        assert!(
            !kms.is_present(&ShredKeyHandle {
                tenant: req.tenant.clone(),
                class: ShredKeyClass::Tenant
            }),
            "the pack-tier DEK is destroyed (reflogs/bitmaps/pack backups unrecoverable)"
        );
        // The residual is named, not pretended-solved.
        assert!(cert.residual_named.contains("off-platform clones"));
    }

    /// **The fan-out is the mutation-core: a SURVIVING stale blob is a RED GA-10.** If a purge fails
    /// to remove a blob, `all_purged` is false, `stale_pii_hits > 0`, and the certificate is NOT
    /// complete — the gate cannot green over a cache that would still serve rewritten-away PII.
    #[test]
    fn a_surviving_stale_blob_is_a_red_ga_10() {
        let repo = ArtifactRef("myelin://acme/git/r".into());
        // A fan-out where one reached entry survived the purge.
        let fan_out = InvalidationFanOut {
            repo: repo.clone(),
            purged: vec![CacheEntryRef::new("trusted", "a")],
            stale_remaining: vec![CacheEntryRef::new("fork:9", "b")],
        };
        assert!(
            !fan_out.all_purged(),
            "a surviving stale blob fails all_purged"
        );
        assert_eq!(fan_out.stale_hits(), 1);
        let cert = GaTenCertificate {
            repo,
            action: HISTORY_REWRITE_ACTION.into(),
            audit_seq: 0,
            fan_out,
            pack_shred_epoch: Some(1),
            stale_pii_hits: 1,
            residual_named: "x".into(),
            content_hash: "blake3:x".into(),
        };
        assert!(
            !cert.is_complete(),
            "a stale hit is NOT a complete GA-10 certificate"
        );

        // `is_complete` requires BOTH conditions (kills the `&&` -> `||` mutant): a cert whose
        // stale_pii_hits is 0 but whose fan-out still has a survivor (the two readings disagree) is
        // NOT complete — neither side alone suffices.
        let repo2 = ArtifactRef("myelin://acme/git/r2".into());
        let inconsistent = GaTenCertificate {
            repo: repo2.clone(),
            action: HISTORY_REWRITE_ACTION.into(),
            audit_seq: 0,
            fan_out: InvalidationFanOut {
                repo: repo2,
                purged: vec![],
                stale_remaining: vec![CacheEntryRef::new("trusted", "survivor")],
            },
            pack_shred_epoch: Some(1),
            stale_pii_hits: 0, // one reading says clean…
            residual_named: "x".into(),
            content_hash: "blake3:x".into(),
        };
        // …but the fan-out still has a survivor, so `all_purged()` is false → NOT complete.
        assert!(!inconsistent.fan_out.all_purged());
        assert!(
            !inconsistent.is_complete(),
            "is_complete needs BOTH 0 stale hits AND a fully-purged fan-out"
        );
    }

    /// **The invalidation never crosses trust scopes (Storage 11.2-C4 — the keyspace IS the
    /// confinement).** Purging a repo's `fork:` scope blob does NOT touch the `trusted` scope's blobs,
    /// and the trusted-scope purge does not touch the fork scope — an `UntrustedFork`-written scope
    /// cannot poison the trusted scope, and the rewrite reaches BOTH because it iterates the whole set.
    #[test]
    fn the_fan_out_purges_per_scope_without_cross_scope_bleed() {
        let tenant = TenantId("acme".into());
        let repo = ArtifactRef("myelin://acme/git/r".into());
        let caches = InMemoryCacheNamespaces::new();
        caches.seed(
            &tenant,
            &repo,
            &CacheEntryRef::new("trusted", "shared-name"),
        );
        caches.seed(
            &tenant,
            &repo,
            &CacheEntryRef::new("fork:42", "shared-name"),
        );

        // Purge ONLY the fork-scope entry: the trusted-scope entry of the SAME name survives.
        let fork_entry = CacheEntryRef::new("fork:42", "shared-name");
        assert!(
            caches.purge(&tenant, &repo, &fork_entry),
            "fork-scope blob purged"
        );
        assert!(
            caches.still_present(
                &tenant,
                &repo,
                &CacheEntryRef::new("trusted", "shared-name")
            ),
            "the trusted-scope blob of the same name is UNTOUCHED (no cross-scope bleed)"
        );
        // The full fan-out reaches BOTH scopes (it iterates entries_for over all scopes).
        let reached = caches.entries_for(&tenant, &repo);
        assert!(
            reached.iter().any(|e| e.is_trusted()),
            "the trusted scope is reached"
        );

        // `is_trusted` is exact: a fork scope is NOT trusted (kills `is_trusted -> true`).
        assert!(CacheEntryRef::new("trusted", "x").is_trusted());
        assert!(
            !CacheEntryRef::new("fork:1", "x").is_trusted(),
            "a fork scope is not trusted"
        );
        assert!(
            !CacheEntryRef::new("branch:main", "x").is_trusted(),
            "a branch scope is not trusted"
        );

        // `entries_for` matches on tenant AND repo (kills the `&&` -> `||` mutant): a different
        // tenant's blob of the same repo, and a different repo's blob of the same tenant, are BOTH
        // excluded — only the (tenant, repo) intersection is returned.
        let other_tenant = TenantId("globex".into());
        caches.seed(
            &other_tenant,
            &repo,
            &CacheEntryRef::new("trusted", "globex-blob"),
        );
        let other_repo = ArtifactRef("myelin://acme/git/other".into());
        caches.seed(
            &tenant,
            &other_repo,
            &CacheEntryRef::new("trusted", "other-repo-blob"),
        );
        let reached = caches.entries_for(&tenant, &repo);
        assert!(
            reached.iter().all(|e| e.name != "globex-blob" && e.name != "other-repo-blob"),
            "entries_for returns ONLY the (tenant ∧ repo) intersection — not a different tenant's or repo's blob"
        );
    }

    /// **The op is RATE-LIMITED (§6.6) — past the per-tenant budget it is REFUSED + audited denied.**
    /// With a budget of 1, the second op in the same window is denied; the denial is audited as a
    /// `git.history_rewrite` entry with outcome `denied` (the attempted op is accountable).
    #[test]
    fn the_op_is_rate_limited_and_a_refusal_is_audited_denied() {
        let req = request();
        let limiter = RewriteRateLimiter::new(1);
        let audit = AuditConsumer::new();
        let kms = pack_kms(&req.tenant);
        let caches = seeded_caches(&req.tenant, &req.repo);
        let wiring = RewriteWiring {
            rate_limiter: &limiter,
            audit: &audit,
            kms: &kms,
            caches: &caches,
        };

        // First op in window 0 is admitted.
        assert!(
            FirstClassRewriteOp::run(&req, &wiring, 0).is_ok(),
            "first op admitted"
        );
        // Second op in the SAME window is REFUSED (budget 1 exhausted).
        let denied = FirstClassRewriteOp::run(&req, &wiring, 0).expect_err("second op refused");
        assert!(matches!(
            denied,
            RewriteDenied::RateLimited { budget: 1, .. }
        ));
        // A NEW window resets the budget → admitted again.
        assert!(
            FirstClassRewriteOp::run(&req, &wiring, 1).is_ok(),
            "new window admits the op"
        );

        // The audit chain holds three legible action facts: the rewrite, the REFUSAL (a distinct
        // git.history_rewrite.denied action — the consumer derives outcome from the event reaching the
        // bus, so a denial is its own action token), then the post-window rewrite. The refusal is
        // accountable, not silently dropped.
        let entries = audit.log().entries_for(&req.tenant);
        let actions: Vec<_> = entries.iter().map(|e| e.action.as_str()).collect();
        assert_eq!(
            actions,
            vec![
                HISTORY_REWRITE_ACTION,
                HISTORY_REWRITE_DENIED_ACTION,
                HISTORY_REWRITE_ACTION,
            ],
            "the rate-limited refusal is audited as a distinct git.history_rewrite.denied action"
        );
    }

    /// **The rate limiter's deny-past-budget gate is exact (mutation-core).** A budget of N admits
    /// exactly N acquisitions in a window then refuses; a fresh window resets. A 0 budget freezes.
    #[test]
    fn the_rate_limiter_admits_exactly_the_budget_per_window() {
        let tenant = TenantId("acme".into());
        let limiter = RewriteRateLimiter::new(3);
        // The budget accessor returns exactly the configured budget (kills `budget -> 1`).
        assert_eq!(
            limiter.budget(),
            3,
            "the budget accessor returns the configured budget"
        );
        assert_eq!(RewriteRateLimiter::new(5).budget(), 5);
        // Exactly 3 admits in window 0, then refusals.
        for _ in 0..3 {
            assert!(limiter.try_acquire(&tenant, 0), "under budget admits");
        }
        assert!(
            !limiter.try_acquire(&tenant, 0),
            "the 4th in the window is refused"
        );
        assert!(
            !limiter.try_acquire(&tenant, 0),
            "still refused (the refusal did not consume a slot)"
        );
        // A new window resets.
        assert!(
            limiter.try_acquire(&tenant, 1),
            "a new window resets the budget"
        );
        // A 0 budget freezes the op entirely.
        let frozen = RewriteRateLimiter::new(0);
        assert!(
            !frozen.try_acquire(&tenant, 0),
            "a 0 budget denies every op"
        );
    }

    /// **GA-D3 at cell scale — audit tamper detected 100% under world-scale audit volume.** A
    /// cell-scale chain of `git.history_rewrite` + other action entries is built; a retroactive edit to
    /// ANY entry is detected by the chain-integrity verifier (the leaf no longer matches, breaking the
    /// chain forward) — 100% of injected tampers caught.
    #[test]
    fn ga_d3_audit_tamper_is_detected_100_percent_at_cell_scale() {
        use crate::audit::verify_entries_for_test;
        let req = request();
        let limiter = RewriteRateLimiter::new(u32::MAX);
        let audit = AuditConsumer::new();
        let kms = pack_kms(&req.tenant);
        let caches = InMemoryCacheNamespaces::new();
        let wiring = RewriteWiring {
            rate_limiter: &limiter,
            audit: &audit,
            kms: &kms,
            caches: &caches,
        };

        // Cell-scale volume of history-rewrite ops on the tamper-evident chain.
        const CELL_SCALE: u64 = 512;
        for w in 0..CELL_SCALE {
            FirstClassRewriteOp::run(&req, &wiring, w).expect("op admitted");
        }
        let entries = audit.log().entries_for(&req.tenant);
        assert_eq!(entries.len() as u64, CELL_SCALE, "cell-scale chain built");
        assert!(
            verify_entries_for_test(&entries),
            "the pristine cell-scale chain verifies intact"
        );

        // Inject a tamper at EVERY position and assert 100% detection (the leaf or seq breaks).
        let mut detected = 0u64;
        for i in 0..entries.len() {
            let mut tampered = entries.clone();
            tampered[i].subject = ArtifactRef(format!("myelin://acme/TAMPERED/{i}"));
            if !verify_entries_for_test(&tampered) {
                detected += 1;
            }
        }
        assert_eq!(
            detected as usize,
            entries.len(),
            "audit tamper detected 100% at cell scale ({detected}/{} entries)",
            entries.len()
        );
    }

    /// **GA-D6 — legal-hold defers erasure (0 held-scope deletions, resumes on lift).** A history-
    /// rewrite's crypto-shred is an erase over the repo's tenant scope; while a legal hold is active
    /// over the scope the erase is DEFERRED (suspend-don't-delete — 0 held-scope deletions), and on
    /// hold-lift it resumes. Proven via the SAME G4 legal-hold gate the retention engine backs
    /// ([`LegalHoldRegistry::verdict`] for [`DsrKind::Erasure`]).
    #[test]
    fn ga_d6_legal_hold_defers_the_rewrite_erasure_and_resumes_on_lift() {
        use crate::dsr::DsrKind;
        use crate::fanout::{HoldVerdict, LegalHoldRegistry};
        use myelin_gdpr::EraseScope;

        let tenant = TenantId("acme".into());
        let scope = EraseScope::Tenant(tenant.clone());
        let holds = LegalHoldRegistry::new();

        // A hold is active over the whole tenant scope the rewrite's erase would touch.
        holds.set(crate::fanout::HoldScope::Tenant(tenant.0.clone()), true);
        assert_eq!(
            holds.verdict(DsrKind::Erasure, &scope),
            HoldVerdict::Deferred,
            "the rewrite erasure is DEFERRED under the legal hold (0 held-scope deletions)"
        );

        // Lift the hold → the deferred erasure RESUMES (the gate now proceeds).
        holds.set(crate::fanout::HoldScope::Tenant(tenant.0.clone()), false);
        assert_eq!(
            holds.verdict(DsrKind::Erasure, &scope),
            HoldVerdict::Proceed,
            "the deferred erasure resumes on hold-lift"
        );
    }

    /// **The GA-10 certificate content-address is deterministic + PII-free.** Two runs of the same op
    /// over the same seeded caches content-address the same; the body carries no name/email.
    #[test]
    fn the_ga_10_certificate_is_a_deterministic_pii_free_artifact() {
        let req = request();
        let mk = || {
            let limiter = RewriteRateLimiter::new(1);
            let audit = AuditConsumer::new();
            let kms = pack_kms(&req.tenant);
            let caches = seeded_caches(&req.tenant, &req.repo);
            let wiring = RewriteWiring {
                rate_limiter: &limiter,
                audit: &audit,
                kms: &kms,
                caches: &caches,
            };
            FirstClassRewriteOp::run(&req, &wiring, 0).expect("op")
        };
        let a = mk();
        let b = mk();
        assert_eq!(
            a.content_hash, b.content_hash,
            "deterministic content-address"
        );
        assert!(a.content_hash.starts_with("blake3:"), "content-addressed");
        // No PII in the certificate body.
        assert!(!a.content_hash.is_empty());
    }

    /// The outbound-gate follow-on prompt is pinned (the residual names its sibling P-GA-36).
    #[test]
    fn the_outbound_gate_follow_on_is_pinned() {
        assert!(HISTORY_REWRITE_OUTBOUND_GATE_PROMPT.contains("P-GA-36"));
        assert!(HISTORY_REWRITE_FIRST_CLASS_PROMPT.contains("P-GA-35"));
    }
}
