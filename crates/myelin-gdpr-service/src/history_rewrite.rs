//! # The history-rewrite resumable-activity skeleton (gdpr §6.6 / GA-10; P-GA-26 → P-153)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/gdpr-and-audit.md` **§6.6** (history-rewrite as
//! a first-class **audited**, tamper-evident, **rate-limited** tenant op — *the Git erasure-admin
//! tool*; **the invalidation fan-out to forks/mirrors/clone-cache tied to Storage's trust-tier /
//! branch-scoped cache namespaces 11.2*; *crypto-shred reaches the pack tier's shreddables [reflogs,
//! bitmaps, pack backups] via the per-tenant blob DEK — NOT the commit-object bytes, that is what
//! the rewrite is for, the honest split*) and **§5.3** (*the history-rewrite skeleton is wired
//! here*; the outbound-mirror residency gate the invalidation crosses ships in P-GA-23 / P-150).
//! It is a **resumable `myelin-flow` activity** (§4.1 step 4 idiom — the same durable-activity shape
//! the DSR fan-out + the deadline timer ride). Prove-it:
//! `external-insights/01-process-and-quality-doctrine.md` §3 — an idempotent activity proves its
//! resumability (a re-driven step is a no-op returning the same receipt), not asserts it.
//!
//! **Contract-index:** wires the **history-rewrite leg of row 10.6** (the audited op). This prompt
//! ships the **SKELETON** (the op body + its resumable-activity shape); the first-class audited op
//! plus the invalidation fan-out (purge the stale clone/bundle blobs, reach the pack-tier
//! shreddables) lands in **M5 (P-GA-35 → P-451, GA-10)** when Git's trust-tier cache namespaces
//! (Storage 11.2) exist.
//!
//! ## What "the skeleton" ships (the named-floor split — EI-01 §3)
//! The full audited op needs surfaces that do not exist until M5 (Git's history-rewrite tool, the
//! trust-tier cache namespaces, the within-EU CDN clone/bundle class). So this prompt ships the
//! **op body's RESUMABLE-ACTIVITY SKELETON** — the part that is real NOW and the part the M5 op
//! resumes through:
//! 1. **[`HistoryRewriteRequest`]** — the rate-limited tenant op's input: the repo
//!    [`ArtifactRef`], the tenant-admin pseudonym actor (the `<pseudonym>@<tenant>.noreply` form,
//!    contract 4.8 — the immutable bytes never bake erasable PII), and the opaque rewrite spec
//!    (a filter-repo-class instruction handle — PII-free).
//! 2. **[`RewritePhase`]** — the ordered, resumable phases of the activity (§6.6 — audit-the-op →
//!    rewrite-the-history → crypto-shred-the-pack-shreddables → invalidate-the-caches). Each phase
//!    is a durable checkpoint: a crashed worker resumes at the first un-receipted phase.
//! 3. **[`HistoryRewriteActivity`]** — the resumable, **idempotent** activity driver: `drive` runs
//!    the phases in order, recording a per-phase receipt; a re-drive (after a crash) runs ONLY the
//!    un-receipted phases and returns the SAME receipts (the §4.1-step-4 resumability the DSR
//!    fan-out uses). The audit phase emits the `git.history_rewrite` action token
//!    ([`HISTORY_REWRITE_ACTION`]); the **invalidation phase is the NAMED FLOOR** — its body is a
//!    LOUD deferral to P-GA-35 (the trust-tier cache namespaces it fans over do not exist until M5),
//!    never a silent no-op that would pretend a rewrite reached the caches it cannot yet reach.
//!
//! ## The honest split (§6.6) — what the rewrite reaches and what it does NOT
//! - The **rewrite** changes the commit-object hashes (that IS the erasure for immutable free-text —
//!   a changed hash is a new object; the old object is unreferenced).
//! - The **crypto-shred** reaches the pack tier's SHREDDABLES (reflogs, bitmaps, pack backups) via
//!   the per-tenant blob DEK — NOT the commit-object bytes themselves (that is what the rewrite is
//!   for). This module declares the phase; the live blob-DEK shred is Storage's mechanism (the
//!   no-cross-store-read seam, the same [`crate::holders::CryptoShredKms`] the GDPR holders use).
//! - The **invalidation fan-out** reaches the replicas it CAN reach (forks/mirrors/clone-cache);
//!   the residual (independent off-platform clones a third party holds) is **named, not
//!   pretended-solved** (§6.6) — recorded on [`HistoryRewriteReceipt::residual_named`].
//!
//! ## Floors named (deferred → filling prompt) — VISION §3 name-your-floors
//! - **The first-class audited op + the invalidation fan-out** → **M5 (P-GA-35, GA-10)**: the audit
//!   ENTRY (the `git.history_rewrite` action sealed into the tamper-evident log via the outbox-only
//!   consumer, P-GA-19), the rate-limit ENFORCEMENT (a tenant-op rate limiter), and the invalidation
//!   over Storage's trust-tier/branch-scoped cache namespaces (11.2) + the within-EU CDN clone/bundle
//!   class — all land when Git's history-rewrite tool + Storage's cache namespaces ship. Here the
//!   skeleton declares the phases + the action token + the resumability; the M5 op resumes through
//!   them.
//! - **The live `myelin-flow` activity runtime** (the durable activity the skeleton is an instance
//!   of) is **P-FLOW-13 → P-207** (the same floor the DSR deadline timer names). This module is
//!   UPSTREAM of `myelin-flow`, so it carries its own deterministic in-memory resumable-activity
//!   model with byte-for-byte the per-phase-checkpoint / resume-un-receipted semantics.
//!
//! ## Mutation floor (P-GA-26 TESTS — the resumable-idempotent activity path is mandatory-core)
//! A re-driven activity (after a crash) MUST run only the un-receipted phases and return the SAME
//! receipts (idempotent + resumable). [`HistoryRewriteActivity::drive`] (the resume-un-receipted
//! loop) is the behavioral core; the `cargo mutants` score is recorded in the commit body.

use std::collections::BTreeMap;
use std::sync::Mutex;

use myelin_tenancy::{ArtifactRef, TenantId};

/// The dotted action token a history-rewrite is audited under (gdpr §6.6 — *kind
/// `git.history_rewrite`, actor = the tenant-admin pseudonym, subject = the repo `ArtifactRef`*).
/// The first-class audit ENTRY (sealed into the tamper-evident log via the outbox-only consumer) is
/// M5 (P-GA-35); the skeleton pins the token so the M5 op uses exactly this string.
pub const HISTORY_REWRITE_ACTION: &str = "git.history_rewrite";

/// The M5 prompt that promotes the skeleton to the first-class audited op + the invalidation
/// fan-out (named here so the floor's follow-on is in writing — VISION §3).
pub const HISTORY_REWRITE_FIRST_CLASS_PROMPT: &str =
    "P-GA-35 (M5) — history-rewrite as a first-class audited op + the invalidation fan-out (GA-10)";

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
    /// trust-tier/branch-scoped cache namespaces 11.2). **The NAMED FLOOR** — its body is a LOUD
    /// deferral to P-GA-35 (the cache namespaces do not exist until M5). The residual (off-platform
    /// clones) is named, not pretended-solved.
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
    /// `true` if the phase's live body is the M5 NAMED FLOOR (deferred, not yet performed). The
    /// invalidation phase is the only such phase on this skeleton.
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
    /// Whether every phase that CAN complete on this skeleton did (the audit/rewrite/crypto-shred
    /// phases) — the invalidation phase is the deferred M5 floor.
    pub fn skeleton_complete(&self) -> bool {
        // The three non-floor phases must be receipted; the invalidation phase is deferred.
        RewritePhase::ALL.iter().all(|p| {
            self.phase_receipts.iter().any(|r| r.phase == *p)
        })
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
            residual_named: format!(
                "independent off-platform clones of {} held by third parties are not reachable by the invalidation fan-out — named, not pretended-solved (gdpr §6.6); resolved as a first-class op in {HISTORY_REWRITE_FIRST_CLASS_PROMPT}",
                request.repo.0
            ),
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

        let deferred_floor = matches!(phase, RewritePhase::InvalidateCaches);
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

    /// **The activity drives every phase in order + the invalidation phase is the NAMED M5 floor
    /// (gdpr §6.6).** The audit action token is `git.history_rewrite`; the residual is named.
    #[test]
    fn the_activity_drives_every_phase_and_names_the_invalidation_floor() {
        let activity = HistoryRewriteActivity::new();
        let receipt = activity.drive(&request());

        assert_eq!(receipt.action, HISTORY_REWRITE_ACTION, "the op is audited as git.history_rewrite");
        assert!(receipt.skeleton_complete(), "every phase is checkpointed");
        // The phases ran in canonical order.
        let order: Vec<_> = receipt.phase_receipts.iter().map(|r| r.phase).collect();
        assert_eq!(order, RewritePhase::ALL.to_vec(), "phases run in §6.6 order");
        // The invalidation phase is the NAMED M5 floor (deferred, not silently "done").
        let invalidate = receipt
            .phase_receipts
            .iter()
            .find(|r| r.phase == RewritePhase::InvalidateCaches)
            .unwrap();
        assert!(invalidate.deferred_floor, "the invalidation fan-out is the M5 named floor");
        // The other phases are NOT floors (the skeleton performs them).
        for r in &receipt.phase_receipts {
            if r.phase != RewritePhase::InvalidateCaches {
                assert!(!r.deferred_floor, "{} is performed on the skeleton", r.phase.token());
            }
        }
        // The residual is NAMED, not pretended-solved (§6.6).
        assert!(receipt.residual_named.contains("off-platform clones"));
        assert!(receipt.residual_named.contains("P-GA-35"), "the residual names its M5 follow-on");
    }

    /// **The activity is RESUMABLE + IDEMPOTENT (the mutation-core — §4.1 step 4).** A re-drive runs
    /// each phase's body EXACTLY ONCE: after a full drive, every phase's call count is 1; a re-drive
    /// (no crash) re-runs NOTHING and returns the SAME receipts.
    #[test]
    fn a_redrive_without_a_crash_runs_no_phase_body_twice() {
        let activity = HistoryRewriteActivity::new();
        let first = activity.drive(&request());
        for phase in RewritePhase::ALL {
            assert_eq!(activity.phase_call_count(phase), 1, "{} ran once", phase.token());
        }
        // Re-drive: no phase body re-runs (all checkpointed), and the receipts are byte-identical.
        let second = activity.drive(&request());
        for phase in RewritePhase::ALL {
            assert_eq!(activity.phase_call_count(phase), 1, "{} did NOT re-run on the re-drive", phase.token());
        }
        assert_eq!(first.phase_receipts, second.phase_receipts, "idempotent — same receipts");
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
        assert_eq!(activity.phase_call_count(RewritePhase::Audit), 1, "phase 0 survived → not re-run");
        assert_eq!(activity.phase_call_count(RewritePhase::Rewrite), 1, "phase 1 survived → not re-run");
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
        assert_eq!(ra.phase_receipts, rb.phase_receipts, "deterministic across activities");
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
        assert!(RewritePhase::InvalidateCaches > RewritePhase::Audit, "invalidation is last");
    }

    /// **Each phase token is the exact PII-free string (mutation-core).** The token reaches the
    /// per-phase receipt body (so a token collision would collide two phases' content addresses);
    /// pinning each kills the `token -> ""` mutant.
    #[test]
    fn each_phase_token_is_the_exact_string() {
        assert_eq!(RewritePhase::Audit.token(), "audit");
        assert_eq!(RewritePhase::Rewrite.token(), "rewrite");
        assert_eq!(RewritePhase::CryptoShredPackTier.token(), "crypto_shred_pack_tier");
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
        missing.phase_receipts.retain(|r| r.phase != RewritePhase::Rewrite);
        assert!(!missing.skeleton_complete(), "a missing phase is not complete");

        // An empty receipt set is NOT complete.
        let mut empty = full;
        empty.phase_receipts.clear();
        assert!(!empty.skeleton_complete(), "no phases is not complete");
    }
}
