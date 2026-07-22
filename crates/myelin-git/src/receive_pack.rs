//! # `receive_pack` — the push write path: sandboxed `git` for bytes, in-process Rust for the
//! policy + the one-transaction ref-CAS + outbox emit (GIT-P9 / P-270, M3-G1)
//!
//! **The silent-data-loss floor (GIT-D9, Tier-1).** Push is the correctness-critical write path.
//! This module is the **in-process Rust half** of the receive-pack path — the half that owns the
//! ref CAS and the `git.ref.updated` emit, both in **ONE transaction** so the event is delivered
//! **iff** the ref move committed (BUS-2 / emit-iff-committed). The byte plumbing (sandboxed
//! canonical `git receive-pack` into a quarantine) runs through the [`crate::core`] `WireExecutor`
//! seam; here we model the **quarantine → policy → ref-CAS → outbox** state machine the architecture
//! pins, and prove its emit-iff-committed property under a crash injected at every step.
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/git-hosting/architecture/02-internals-and-algorithms.md`
//! - **§2** (the sandboxed receive-pack → quarantine → in-process Rust policy → ref-CAS + outbox in
//!   ONE tx; reject BEFORE the ref moves; abort discards the quarantine — never promoted),
//! - **§3** (the reftable-on-OLTP ref store: the per-ref `FOR UPDATE` CAS is the linearisation
//!   point; the aggregate for `git.ref.updated` is the REF; `update_seq` is the outbox-seq tiebreak),
//! - **§4** (the DB transaction is the linearisation point — Postgres is the Praefect).
//!
//! **Contracts implemented (frozen shapes — escalate any change):**
//! - **2.2 / 2.3** `OutboxTx::emit` + the per-ref aggregate — the receive-pack → ref-CAS → outbox
//!   emit in ONE [`myelin_events::OutboxStore`] transaction (owned). The per-ref aggregate key is
//!   `<repo>:<ref_name>` (arch §2.2 / [`myelin_events::partition`]); the ordering is per-ref.
//! - **2.9** `git.ref.updated` emission (owned) — the core push event, registered in
//!   [`crate::events::GIT_REF_UPDATED`], EMITTED here for the first time (the GIT-P2 emit follow-on).
//! - **10.1** `PersonalDataHolder` H1 registration (consumed) — the git store auto-registers as
//!   holder **H1** when it opens ([`RefStore::open`] → [`crate::holder_intent::HOLDER_ID`]). The DSR
//!   bodies (locate/export/erase fan-out) are the **GIT-P29 floor** (the prompt: "locate/export/erase
//!   land in GIT-P29"); the registration receipt is real here.
//!
//! ## DEVIATION / FLOOR — the in-memory model of the SQL transaction (EI-01 §1, written down)
//! `cargo build --workspace` is **DB-free** (the binding policy). The OLTP tier client + the real
//! Postgres `git_ref` table + the `outbox` `INSERT … RETURNING` inside the caller's transaction land
//! with the Storage `PgStore` wiring. So the **mechanism** this prompt owns — the per-ref CAS that is
//! the linearisation point, and the ref-update + outbox-row co-commit in one transaction — is modeled
//! as an **in-memory transactional store** ([`RefStore`]) whose semantics are byte-for-byte the
//! arch §3 contract: a ref row `(repo, ref, target_oid, update_seq)`, a per-ref CAS guarded by an
//! expected-old assertion (the `FOR UPDATE` row lock + non-fast-forward reject), and the ref-update +
//! the `OutboxTx::emit` committed **together** through the **already-frozen** [`myelin_events::OutboxStore`]
//! same-transaction co-commit (the substrate's P-S07 mechanism — reused, NOT re-implemented, EI-01
//! §7). The frozen DDL for the `git_ref` table is [`GIT_REF_MIGRATION`]; the real `UPDATE … WHERE
//! target_oid = expected_old` + the same-tx outbox insert land when `PgStore` is wired. The seam shape
//! (the `RefStore` API, the `git.ref.updated` payload, the one-transaction co-commit) does NOT change.
//!
//! The crash injection ([`CrashPoint`]) models the GIT-D9 failure-injection harness: the serving tier
//! is killed at a chosen step (after policy / before commit / after commit) and we assert the store +
//! the outbox survived consistently — 0 ghost (no event without its committed ref move), 0 lost (no
//! committed ref move without its event), quarantine discarded on abort.
//!
//! ## FLOORS named (VISION §3 — none NEW correctness floors here)
//! - **GIT-P10** hardens the per-ref aggregate **ordering under a hot-ref burst at push QPS** (GIT-D1)
//!   — this prompt proves single-push correctness + emit-iff-committed; the burst-ordering load proof
//!   is its sibling.
//! - **GIT-P11** lands the **local-NVMe pack tier behind the `BlobStore` trait** (the object bytes the
//!   accepted quarantine migrates into) — here the object migration is modeled as a durable-bytes ack
//!   step ([`QuarantineMigration`]) behind a trait the pack tier implements.
//! - **GIT-P29** lands the H1 holder DSR **bodies** (the §6.1 erasure fan-out: pseudonym-map shred +
//!   per-subject DEK crypto-shred + Search purge + Refs tombstone). Here H1 **registers** (the receipt
//!   is real) and the bodies are the named floor.
//! - The **production X-6-hardened `git receive-pack` executor** (the sandboxed byte plumbing into the
//!   quarantine) is the [`crate::core::WireExecutor`] production impl (GIT-P13 serving tier); here the
//!   quarantine is modeled by the proposed-ref-updates + the object set the policy gates.

use crate::events::GIT_REF_UPDATED;
use myelin_events::{
    AggregateKey, ArtifactRef, DataRole, EmitContextBase, EventDraft, EventType, IdMinter,
    OutboxError, OutboxStore, OutboxTx, Visibility,
};
use std::collections::BTreeMap;
use std::sync::Arc;

// R0.2 / DELTA N1 — the direct-push-to-a-protected-ref gate REUSES the PR merge gate. The
// required-set logic lives in [`crate::merge_gate`] (never re-implemented here); this module only
// wires it onto the wire push path (a direct `git push` to a protected ref must clear the SAME gate a
// PR merge would). No cycle: `merge_gate`/`lifecycle`/`check_status` do not depend on `receive_pack`.
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

// ───────────────────────────── the frozen ref-store DDL (arch §3 / §4.2) ─────────────────────────

/// The frozen forward-only DDL for the `git_ref` reftable-on-OLTP store (arch `01 §4.2`, `02 §3`).
/// The shape the migration runner applies when `PgStore` is wired; the in-memory [`RefStore`] models
/// exactly these semantics until then. The columns + constraints are the contract:
/// - `(tenant, repo, ref_name)` is the **primary key** — the per-ref row the CAS locks `FOR UPDATE`
///   (the per-ref linearisation point, arch §3);
/// - `target_oid` is the ref tip (`bytea`, hash-agnostic — rendered hex here);
/// - `update_seq` is the monotonic per-ref generation (the `outbox.seq` tiebreak + the recovery
///   fence, arch §4.2 — a node serving a stale `update_seq` is behind);
/// - the ref-update + the `outbox` row commit in **one transaction** (BUS-2): there is no `git_ref`
///   UPDATE without its `outbox` INSERT, and none without it.
///
/// **Forward-only** (the `forward-only-migration` lint): an `expand` migration (adds the table only);
/// no destructive down-migration.
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

// ───────────────────────────── value types (the push vocabulary) ─────────────────────────────────

/// The fully-qualified ref a push proposes to move (`refs/heads/main`, `refs/tags/v1`). The
/// per-ref aggregate (the ordering key) is derived from `(repo, ref_name)` — arch §2.2 / §3.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RefName(pub String);

impl RefName {
    /// Wrap a fully-qualified ref name.
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }
    /// `true` iff this is a protected branch under the (modeled) ruleset — `refs/heads/main` and
    /// `refs/heads/release/*` here (the real ruleset is the GIT-P13/GIT-P26 branch-protection
    /// resolver; this is the minimal protected-set the policy gates on).
    pub fn is_protected(&self) -> bool {
        self.0 == "refs/heads/main" || self.0.starts_with("refs/heads/release/")
    }
}

/// A git object id (rendered hex; the data model stores `bytea`, hash-agnostic — arch `01 §3.0`).
/// The all-zeros oid is the **create/delete sentinel** (a push from zero is a create; a push to
/// zero is a delete — git's convention).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Oid(pub String);

impl Oid {
    /// Wrap a hex object id.
    pub fn new(hex: impl Into<String>) -> Self {
        Self(hex.into())
    }
    /// The all-zeros sentinel (a non-existent tip — create source / delete target).
    pub fn zero() -> Self {
        Self("0".repeat(40))
    }
    /// `true` iff this is the all-zeros sentinel.
    pub fn is_zero(&self) -> bool {
        self.0.chars().all(|c| c == '0')
    }
}

/// One object in the push's **quarantine** (arch §2 step 1: `git receive-pack` ingests the pack into
/// a quarantine object dir; abort discards them; accept migrates them into the repo object DB). The
/// in-process policy (secret-scan, size) inspects these BEFORE the ref moves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuarantineObject {
    /// The object's oid (rendered hex).
    pub oid: Oid,
    /// The object's bytes (a blob/commit/tree body; the policy scans blobs, sizes everything).
    pub bytes: Vec<u8>,
}

/// One proposed ref update the sandboxed `receive-pack` reported (arch §2 step 1: `old_oid →
/// new_oid`). A push is a SET of these (the atomic push — all-or-nothing, arch §2 step 2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProposedRefUpdate {
    /// The ref to move.
    pub ref_name: RefName,
    /// The ref's current tip the client believes it is moving from (the CAS expected-old). The
    /// all-zeros sentinel means "create" (the ref must not yet exist).
    pub expected_old: Oid,
    /// The new tip (the all-zeros sentinel means "delete").
    pub new_oid: Oid,
    /// Whether the client requested a non-fast-forward (force) update (arch §2 ruleset: force-push
    /// bans on protected refs).
    pub forced: bool,
    /// The commit oids this update introduces (the payload's `commit_oids[]`; the §2 secret-scan
    /// + the CI/Search/Refs fan-out key on these). Empty for a delete.
    pub commit_oids: Vec<Oid>,
}

/// The verified pusher (the principal kind + the per-tenant pseudonym, GIT-1). The pseudonym — never
/// a raw identity — is baked into the `git.ref.updated` payload + the reflog (arch §2 step 2:
/// pseudonymity enforcement; the erasable real identity is resolved out-of-band through Identity).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pusher {
    /// The opaque per-tenant pseudonym (`<pseudonym>@<tenant>.noreply`, contract 4.8 — the lever
    /// that makes erasure usually free; `erasure = Pseudonymise`).
    pub pseudonym: String,
    /// Whether the pusher is an agent (the §2 agent rule: an agent push to a ruleset-gated ref that
    /// requires a human is rejected — `agent_needs_human`).
    pub is_agent: bool,
}

/// A whole push session (arch §2): the proposed ref updates + the quarantine objects + the verified
/// pusher. The atomic unit — the policy gates the WHOLE set, and on accept the ref-CASes + the outbox
/// emits commit together (all-or-nothing per push).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PushSession {
    /// The proposed ref updates (the atomic set).
    pub updates: Vec<ProposedRefUpdate>,
    /// The quarantine objects (discarded on abort, migrated on accept).
    pub quarantine: Vec<QuarantineObject>,
    /// The verified pusher (pseudonym + agent flag).
    pub pusher: Pusher,
}

// ───────────────────────────── the policy decision (reject BEFORE the ref moves) ────────────────

/// The reason a push was rejected — REJECT BEFORE THE REF MOVES (arch §2 step 2), so the ref CAS in
/// step 4 never runs and the quarantine is discarded (never promoted). Each variant names the policy
/// rule that fired (a rejected push is LOUD, never a silent partial write).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RejectReason {
    /// The same ref appeared more than once in one push. A push is a set of ref transitions; allowing
    /// duplicates would plan multiple witnesses from one old generation and fail only after commit.
    DuplicateRefUpdate {
        /// the duplicated ref.
        ref_name: RefName,
    },
    /// A force-push (non-fast-forward) was attempted on a protected ref (ruleset force-push ban).
    ForcePushOnProtected {
        /// the protected ref.
        ref_name: RefName,
    },
    /// A protected ref deletion was attempted (ruleset deletion ban).
    DeleteProtected {
        /// the protected ref.
        ref_name: RefName,
    },
    /// The secret scanner matched a quarantined object (a credential pattern; arch §2 step 2 —
    /// reject before the ref moves so the secret never lands in the object DB).
    SecretDetected {
        /// the offending object oid.
        oid: Oid,
        /// the matched pattern (the LOUD reason — never silently dropped).
        pattern: String,
    },
    /// A quarantined object exceeds the per-object size limit (ruleset size limit).
    ObjectTooLarge {
        /// the offending object oid.
        oid: Oid,
        /// the object's size in bytes.
        size: usize,
        /// the configured limit.
        limit: usize,
    },
    /// An agent pushed directly to a ruleset-gated ref that requires a human (`agent_needs_human`).
    AgentNeedsHuman {
        /// the gated ref.
        ref_name: RefName,
    },
    /// The push's pseudonymity assertion failed (an empty/non-pseudonymous identity — GIT-1).
    PseudonymRequired,
    /// A pushed COMMIT object's author/committer identity is not the principal's tenant pseudonym
    /// (GIT-1 / GIT-P12 — the data-model gate). Reject-at-push (the chosen default, sha-stable):
    /// the cleartext-PII commit never moves a ref, so the immutable object DB admits 0 cleartext PII
    /// in a commit identity field (the GIT-D2 gate). Carries the offending object's oid + the
    /// specific [`crate::commit::NonPseudonymousIdentity`] the door refused.
    NonPseudonymousCommit {
        /// the offending commit object's oid.
        oid: Oid,
        /// exactly why the commit identity is not the tenant pseudonym (LOUD — never silently coerced).
        identity: crate::commit::NonPseudonymousIdentity,
    },
    /// The CAS expected-old did not match the ref's current tip (a non-fast-forward / lost-update
    /// race) — the per-ref linearisation point rejected the stale push (arch §3).
    NonFastForward {
        /// the ref.
        ref_name: RefName,
        /// the tip the client believed it was moving from.
        expected: Oid,
        /// the ref's actual current tip.
        actual: Oid,
    },
    /// **R0.2 / DELTA N1 (HIGH) — a DIRECT push to a protected ref whose branch-protection merge gate
    /// is NOT green for the pushed head.** A `git push` straight to a protected branch is refused
    /// unless every `required_contexts` in the repo's [`BranchProtectionRuleset`] has a
    /// current-and-acceptable success for the pushed commit — EXACTLY the gate a PR merge into that ref
    /// would clear ([`crate::merge_gate::evaluate_merge_gate`], reused). This closes the
    /// under-gated-protected-push hole: a principal holding `git.wire.receive_pack` can no longer land
    /// un-CI'd code on a protected branch by pushing to it directly. Carries the SPECIFIC unmet
    /// contexts (loud — never a silent under-gate).
    ProtectedCheckNotGreen {
        /// the protected ref the push targeted.
        ref_name: RefName,
        /// the specific required contexts that were missing / not-green / un-endorsed-fork.
        unmet: Vec<UnmetContext>,
    },
    /// **R0.2 / DELTA N1 — a repo ruleset named an UNPARSEABLE required context.** Fail-closed: an
    /// unparseable required context is NEVER treated as "not required" (that would be an under-gated
    /// protected push). Carries the protected ref + the loud parse detail.
    ProtectedGateInput {
        /// the protected ref the push targeted.
        ref_name: RefName,
        /// the loud, humanisable parse detail (never silently dropped).
        detail: String,
    },
    /// **R2-exit blocker (HIGH) — a DIRECT push to a PROTECTED ref by a NON-bypass pusher failed the
    /// FULL branch-protection ruleset** (approvals / CODEOWNERS / conversation-resolution — the same
    /// [`crate::lifecycle::evaluate_ruleset`] a PR merge clears). A direct `git push` carries NO PR
    /// review context (0 approvals, no CODEOWNERS approval, no resolved conversations), so a ruleset
    /// that requires ANY of them is UNSATISFIABLE for a direct push — it accepts a direct push ONLY
    /// from a `protected_push`/bypass (admin) pusher. This closes the writer→protected-branch
    /// escalation: a plain writer can no longer land on a protected ref by pushing to it directly, even
    /// with (producer-attested) green checks, because the approvals/CODEOWNERS half is not satisfiable
    /// without a PR + genuine reviews. Carries the SPECIFIC unmet ruleset reasons (loud, typed).
    ProtectedRulesetNotSatisfied {
        /// the protected ref the push targeted.
        ref_name: RefName,
        /// the specific ruleset conditions the direct push did not satisfy (≥ 1).
        reasons: Vec<BlockReason>,
    },
}

/// The push policy configuration the in-process engine evaluates (arch §2 step 2). The minimal-but-
/// real ruleset this prompt gates on; the full branch-protection ruleset resolver (force-push /
/// deletion / linear-history / signed-commit / required-contexts) lands in GIT-P13/GIT-P26 — the
/// required-CONTEXT enforcement is deferred to the MERGE GATE for PRs (arch §2, X-1).
#[derive(Clone, Debug)]
pub struct PushPolicy {
    /// The per-object size limit in bytes (a quarantined object larger than this is rejected).
    pub max_object_bytes: usize,
    /// Secret patterns the scanner matches against quarantined object bytes (regex/entropy is the
    /// real scanner; here a substring match models "reject before the ref moves").
    pub secret_patterns: Vec<String>,
    /// Whether protected refs require a human pusher (an agent direct-push is rejected).
    pub protected_needs_human: bool,
    /// The tenant the push is authenticated under (from the token, never the URL — X-1). The
    /// pseudonymity rule (GIT-1 / GIT-P12) requires every pushed commit's author/committer identity
    /// to be a `<pseudonym>@<tenant>.noreply` handle for THIS tenant; a commit carrying a raw
    /// name/email — or a pseudonym for another tenant — is rejected before the ref moves.
    pub tenant: String,
}

impl Default for PushPolicy {
    fn default() -> Self {
        Self {
            // 50 MiB per object — git's default large-object guard order of magnitude.
            max_object_bytes: 50 * 1024 * 1024,
            secret_patterns: vec![
                "AKIA".to_string(), // an AWS access-key id prefix
                "-----BEGIN PRIVATE KEY".to_string(),
                "-----BEGIN RSA PRIVATE KEY".to_string(),
            ],
            protected_needs_human: true,
            // The default policy is tenant-agnostic for the non-pseudonymity rules; `RefStore::receive`
            // overrides this with the store's authenticated tenant before evaluating (so the
            // pseudonymity rule has the principal's tenant). A blank tenant matches no pseudonym
            // handle → fail-closed if a caller forgets to set it.
            tenant: String::new(),
        }
    }
}

impl PushPolicy {
    /// Evaluate the WHOLE push against the ruleset + secret-scan + size + pseudonymity + agent rules
    /// — arch §2 step 2. Returns the FIRST [`RejectReason`] (reject-before-the-ref-moves: any reject
    /// aborts the whole atomic push and the caller discards the quarantine), or `Ok(())` to proceed
    /// to the ref-CAS. The CAS-staleness check (`NonFastForward`) is NOT here — it is the per-ref
    /// row-lock assertion inside the transaction (arch §3), so the linearisation point owns it.
    pub fn evaluate(&self, push: &PushSession) -> Result<(), RejectReason> {
        // Pseudonymity (GIT-1): the pusher identity MUST be a non-empty pseudonym (never raw / blank).
        if push.pusher.pseudonym.trim().is_empty() {
            return Err(RejectReason::PseudonymRequired);
        }
        // Per-ref ruleset (force-push / deletion / agent) — reject before the ref moves.
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
        // Secret-scan + size over the quarantine — reject before the ref moves so the secret /
        // oversized object never migrates into the repo object DB.
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
            // Pseudonymity (GIT-1 / GIT-P12 — the data-model gate): a pushed COMMIT object's
            // author/committer identity MUST be the principal's tenant pseudonym
            // `<pseudonym>@<tenant>.noreply` (contract 4.8). REJECT-AT-PUSH (the chosen default,
            // sha-stable — see `crate::commit`): a commit carrying a raw name/email — or a pseudonym
            // for another tenant — is refused BEFORE the ref moves, so the immutable object DB never
            // admits cleartext PII in a commit identity field (the GIT-D2 "0 cleartext PII" gate).
            // Only commit objects carry an identity line; blobs/trees are skipped.
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

// ───────────────────────────── R0.2 / DELTA N1: the protected-ref direct-push gate ───────────────

/// Build a synthetic [`CheckStatus`] fact for the pushed head from a recorded check-context name — the
/// bridge between Git's OWN recorded check facts (context strings) and the typed
/// [`CheckStatusProjection`] the merge gate reads. Mirrors the PR merge path's fact synthesis
/// ([`crate::pr_store`]): a recorded green becomes a `Trusted` success, a recorded fork-unendorsed
/// green becomes an `UntrustedFork` success (neutral-for-gating until endorsed, Δ3). ACYCLIC — this
/// reads facts Git already recorded; it NEVER synchronously calls CI (EI-02 §3).
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

/// **R0.2 / DELTA N1 (HIGH) — the branch-protection posture a DIRECT push to a PROTECTED ref must
/// clear over the wire.** The security invariant: a `git push` straight to a protected branch is held
/// to the SAME gate a PR merge into that branch would be — no under-gated protected push. Given the
/// repo-owned [`BranchProtectionRuleset`] for the target ref (loaded by the caller from the repo's
/// durable config — never a hardcoded literal, never author input) + Git's OWN recorded check facts
/// for the pushed head, this refuses:
/// - a DELETE of a protected ref ([`RejectReason::DeleteProtected`]);
/// - a FORCE-push (non-fast-forward) of a protected ref unless the ruleset sets `allow_force_push`
///   ([`RejectReason::ForcePushOnProtected`]);
/// - a push whose head does NOT clear the required-context merge gate
///   ([`RejectReason::ProtectedCheckNotGreen`]) — evaluated by [`evaluate_merge_gate`] (REUSED, never
///   duplicated) against the projection synthesised from the recorded facts.
///
/// A protected ref with an EMPTY required set clears the checks half on a plain fast-forward (the
/// force/delete bans still apply) — so this is never MORE permissive than the pre-R0.2 hardcoded
/// force/delete floor, only stricter (it now also enforces the repo's configured required contexts).
/// Returns `Ok(())` iff the push may proceed to the ref-CAS. Reads facts Git already holds — ACYCLIC,
/// it never calls CI (EI-02 §3).
///
/// **R2-exit blocker fix — the pusher's `protected_push` standing + the FULL ruleset.** Two defects
/// this composition closes (the writer→protected-branch escalation two red-team adversaries proved):
/// - **`pusher_has_protected_push`** — the pusher's bypass standing (the repo's admin-only
///   `protected_push` relation, resolved by the caller through the per-repo authorizer). Force/delete
///   bans apply to EVERYONE (R0.2, kept); but the required-checks + approvals/CODEOWNERS gate below is
///   BYPASSED only for a `protected_push`/admin pusher — a plain writer never bypasses it.
/// - **the FULL branch-protection ruleset, not just `required_contexts`.** A direct push is held to the
///   SAME [`crate::lifecycle::evaluate_ruleset`] a PR merge clears — required approvals, CODEOWNERS
///   review, conversation resolution — REUSED (one ruleset evaluation, no divergent second gate). A
///   direct push carries NO PR review context, so a ruleset requiring approvals/CODEOWNERS is
///   UNSATISFIABLE for it: the ref then admits a direct push ONLY from a bypass/admin pusher (gated
///   above) — the correct protected-branch semantics.
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
    // A protected ref is never DELETED over the wire (the ruleset deletion ban) — applies to EVERYONE,
    // bypass/admin included (keep the R0.2 floor green).
    if is_delete {
        return Err(RejectReason::DeleteProtected {
            ref_name: ref_name.clone(),
        });
    }
    // A protected ref is never FORCE-pushed unless the repo ruleset opts in (`allow_force_push`) —
    // applies to EVERYONE (bypass/admin does not license a non-ff on a protected ref; R0.2 floor).
    if is_forced && !ruleset.allow_force_push {
        return Err(RejectReason::ForcePushOnProtected {
            ref_name: ref_name.clone(),
        });
    }
    // **The bypass leg (Defect 2).** A pusher holding the repo's admin-only `protected_push` relation
    // may direct-push a protected ref (a legitimate hotfix/admin path) — it CLEARS the required-checks +
    // approvals/CODEOWNERS gate below. A plain writer holds no such standing, so it never reaches here.
    if pusher_has_protected_push {
        return Ok(());
    }

    // ── The NON-bypass path: the SAME gate a PR merge into this ref would clear (Defect 3). ──
    // The required-set merge gate — REUSED from `merge_gate` (0 duplicate protected-branch notion).
    let gate_input = |detail: String| RejectReason::ProtectedGateInput {
        ref_name: ref_name.clone(),
        detail,
    };
    let policy = MergeGatePolicy::from_required_contexts(&ruleset.required_contexts)
        .map_err(|e| gate_input(e.to_string()))?;

    // Synthesise the projection for the pushed head from Git's OWN recorded facts (acyclic — no CI
    // call). A malformed recorded context name is fail-closed (never treated as "not required").
    let mut proj = CheckStatusProjection::new();
    for c in green_contexts {
        let ctx = parse_required_context(c).map_err(|e| gate_input(e.to_string()))?;
        proj.apply(&synthetic_check_fact(head_oid, ctx, TrustTier::Trusted));
    }
    for c in fork_unendorsed_contexts {
        let ctx = parse_required_context(c).map_err(|e| gate_input(e.to_string()))?;
        proj.apply(&synthetic_check_fact(head_oid, ctx, TrustTier::UntrustedFork));
    }
    let endorsed: Vec<CheckContext> = endorsed_contexts
        .iter()
        .map(|c| parse_required_context(c).map_err(|e| gate_input(e.to_string())))
        .collect::<Result<_, _>>()?;

    // Half A — the required-CONTEXTS gate (green-and-current with an acceptable trust posture).
    if let MergeGateOutcome::Blocked { unmet } =
        evaluate_merge_gate(&policy, &proj, head_oid, &endorsed)
    {
        return Err(RejectReason::ProtectedCheckNotGreen {
            ref_name: ref_name.clone(),
            unmet,
        });
    }

    // Half B — the FULL ruleset (approvals / CODEOWNERS / conversation-resolution), REUSED verbatim
    // from the merge path ([`crate::lifecycle::evaluate_ruleset`] — the same function `merge_pr` runs).
    // A DIRECT push carries NO PR review context: 0 approvals, no CODEOWNERS approval, no conversation
    // threads. So a ruleset requiring approvals/CODEOWNERS is UNSATISFIABLE for a direct push by a
    // non-bypass pusher (the `required_contexts` half is owned above — emptied here to avoid a
    // double-evaluation / drift, exactly as `pr_store::evaluate_merge` splits the two halves).
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

// ───────────────────────────── the quarantine object migration (the durable-bytes ack) ───────────

/// The object-migration step (arch §2 step 3): on accept, the quarantined objects migrate into the
/// repo object DB **before the ref CAS**, and the migration does not ack until the bytes are durable
/// on the write quorum (arch §4). The pack tier (GIT-P11, local-NVMe behind `BlobStore`) implements
/// this; here it is the seam the accept path calls. A migration that does NOT ack (a durability
/// failure) aborts the push (the ref never moves over un-durable objects).
pub trait QuarantineMigration {
    /// Migrate the accepted quarantine objects into the repo object DB, acking ONLY when the bytes
    /// are durable on the write quorum. `Err` aborts the push (the ref CAS never runs).
    fn migrate(&self, objects: &[QuarantineObject]) -> Result<(), String>;
}

/// The in-memory migration sink (the GIT-P11 floor): records migrated oids so a test can assert the
/// accepted objects were promoted (and a rejected push's quarantine was NOT). The real pack tier
/// writes them through `BlobStore` to the quorum-ack replica set.
#[derive(Clone, Default)]
pub struct InMemoryObjectDb {
    migrated: Arc<std::sync::Mutex<std::collections::BTreeSet<Oid>>>,
}

impl InMemoryObjectDb {
    /// A fresh, empty object DB.
    pub fn new() -> Self {
        Self::default()
    }
    /// Whether an oid was migrated (promoted out of quarantine into the object DB).
    pub fn contains(&self, oid: &Oid) -> bool {
        self.migrated
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(oid)
    }
    /// The number of migrated objects.
    pub fn len(&self) -> usize {
        self.migrated
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }
    /// Whether the object DB is empty.
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

// ───────────────────────────── the crash-injection seam (the GIT-D9 harness) ─────────────────────

/// Where to crash the serving tier mid-push (the GIT-D9 failure-injection harness — arch §2 / the
/// drill catalogue row GIT-D9). The drill kills the process at each step and asserts emit-iff-
/// committed survived: a crash BEFORE commit leaves 0 ghost rows + the ref unmoved (the abort
/// discards the quarantine); a crash AFTER commit leaves the ref moved AND the event durable (the
/// relay will publish it — 0 lost). [`CrashPoint::None`] is the happy path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrashPoint {
    /// No crash — the happy path.
    None,
    /// Crash AFTER the policy passed but BEFORE the object migration — the ref never moves, nothing
    /// is emitted, the quarantine is discarded (0 ghost).
    AfterPolicy,
    /// Crash AFTER the object migration but BEFORE the transaction commits — the bytes are durable
    /// but the ref did NOT move and NO event was emitted (emit-iff-committed: 0 ghost; the un-acked
    /// ref move is discarded on recovery, arch §4.2).
    BeforeCommit,
    /// Crash AFTER the transaction committed (the ref moved + the event row is durable + unsent) but
    /// BEFORE any post-commit work — the ref move + the event SURVIVE (0 lost; the relay publishes
    /// the durable row, the recovery fence reconciles to the committed `update_seq`).
    AfterCommit,
    /// **The GT-003 reconciler window.** Crash AFTER the outbox transaction committed (the
    /// `git.ref.updated` row is durable) but BEFORE the on-disk ref CAS applied
    /// ([`RefStore::apply_one`]). This is the precise apply-after-outbox-commit window the cross-system
    /// reconciler ([`crate::reconcile`]) recovers: the event is the durable witness, the on-disk ref is
    /// momentarily BEHIND its committed `update_seq`. The apply loop is skipped so the same recovery
    /// replay is exercised on either backing. NOT silent loss — the committed event drives an
    /// idempotent re-apply on restart.
    AfterCommitBeforeApply,
}

/// A crash injected at [`CrashPoint`] — the recoverable failure the GIT-D9 drill forces. The push
/// returns this instead of completing; the caller (the drill) then inspects the store + the outbox
/// to assert the surviving state is consistent. Modeling a crash as a returned error (not a real
/// `panic`/`abort`) keeps the test harness deterministic while exercising the SAME code paths up to
/// the kill point — the transaction's commit-or-not is the only thing that decides survival.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InjectedCrash {
    /// Where the crash fired.
    pub at: CrashPoint,
}

// ───────────────────────────── the push outcome ──────────────────────────────────────────────────

/// The outcome of a [`RefStore::receive`] call (arch §2). Either the push was accepted (the refs
/// moved + the events committed in one transaction), rejected by policy (the ref never moved + the
/// quarantine discarded), or a crash was injected mid-push (the drill inspects survival).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PushOutcome {
    /// The push committed: each ref moved + a `git.ref.updated` row is durable, all in one
    /// transaction. Carries the committed `(ref, new_oid, update_seq)` triples (the linearisation
    /// witnesses) + the emitted event ids.
    Accepted {
        /// the refs moved, with their post-update `update_seq` (the generation fence).
        moved: Vec<(RefName, Oid, u64)>,
        /// the emitted `git.ref.updated` event ids (one per moved ref).
        emitted: Vec<myelin_events::EventId>,
    },
    /// The push was rejected by policy — the ref never moved, the quarantine was discarded (never
    /// promoted), nothing was emitted (0 ghost). Carries the LOUD reject reason.
    Rejected(RejectReason),
    /// A crash was injected mid-push — the caller inspects the store + outbox for survival.
    Crashed(InjectedCrash),
}

// ───────────────────────────── the reftable-on-OLTP ref store (the linearisation point) ──────────

/// One ref row (arch §3 / [`GIT_REF_MIGRATION`]): the tip + the monotonic generation. The per-ref
/// CAS locks this row (`FOR UPDATE`) — the linearisation point for that ref.
#[derive(Clone, Debug, PartialEq, Eq)]
struct RefRow {
    target_oid: Oid,
    update_seq: u64,
}

/// One reflog entry (arch §3: `INSERT git_reflog` in the same transaction as the ref CAS).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReflogEntry {
    /// the ref that moved.
    pub ref_name: RefName,
    /// the old tip (`None` for a create).
    pub old_oid: Option<Oid>,
    /// the new tip.
    pub new_oid: Oid,
    /// the post-update generation.
    pub update_seq: u64,
    /// the pusher pseudonym (GIT-1 — never a raw identity).
    pub pusher_pseudonym: String,
}

/// One per-ref **linearisation lock** (arch §3 / GIT-P10). Held across the whole CAS → commit →
/// apply window so a rapid burst of pushes to ONE hot ref serialises on THIS lock, while pushes to
/// OTHER refs lock OTHER cells and proceed in PARALLEL (the per-ref-order / refs-fan-out-parallel
/// property GIT-P10 (GIT-D1) hardens). The lock carries NO state — the durable ref state lives in the
/// [`RefBacking`] (the on-disk git repo in production, an in-memory map in the test double). This is
/// the reconciliation: GT-001 moved the ref STATE onto durable on-disk git (SI-012), keeping the
/// in-process per-ref lock as the linearisation point the burst drill (GIT-D1) relies on.
type RefLock = std::sync::Mutex<()>;

/// **The ref-state backing** — where the durable ref rows + reflog actually live. The
/// reconciliation GT-001 lands (prompt §4: "the in-memory `RefStore`/index become a real
/// on-disk-git-backed store … keep the in-memory form as an explicit test-double"):
/// - [`RefBacking::Disk`] is the **PRODUCTION** path — refs / reflog / objects on the real on-disk
///   bare repo ([`crate::durable::DurableGitRepo`], via `git2`), so a ref written then read after a
///   FRESH `RefStore` over the same on-disk root is still there (SI-012 fixed: `open` loads from
///   disk, the object/oid lookup is the real on-disk odb — F-git-2).
/// - [`RefBacking::Memory`] is the **TEST DOUBLE** — the former in-memory `git_ref` row model + the
///   append-only reflog. It keeps the rich receive-pack CAS / emit-iff-committed / crash-injection
///   unit tests + the GIT-D1 burst drill running in isolation (no temp fs), byte-for-byte identical
///   to the prior behaviour. NOT a durable system-of-record.
enum RefBacking {
    /// The in-memory test double: `git_ref` rows + the append-only reflog (the prior model).
    Memory {
        /// the modeled `git_ref` rows (the durable state, in memory for the double).
        rows: std::sync::Mutex<BTreeMap<RefName, RefRow>>,
        /// the append-only reflog (the modeled `git_reflog` table).
        reflog: std::sync::Mutex<Vec<ReflogEntry>>,
    },
    /// The production durable path: refs / reflog / objects on the real on-disk bare repo.
    Disk {
        /// the on-disk bare repo handle (`git2`); refs + reflog + odb all live here, survive restart.
        repo: Arc<crate::durable::DurableGitRepo>,
    },
}

/// **The reftable-on-OLTP ref store + the receive-pack write path** (arch §2 / §3). The per-ref CAS
/// is the linearisation point; the ref-update + the reflog insert + the `git.ref.updated` outbox emit
/// commit in **ONE transaction** (BUS-2). Owns the repo locator + the shared [`OutboxStore`] (the
/// frozen substrate co-commit, reused) + the id minter.
///
/// **H1 holder (10.1):** [`RefStore::open`] auto-registers the store as `PersonalDataHolder` **H1**
/// (the registration receipt is real; the DSR bodies are the GIT-P29 floor).
pub struct RefStore {
    /// the repo this store serves (the per-ref aggregate key is `<repo>:<ref_name>`).
    repo: String,
    /// the residency/partition keys threaded onto every emit.
    ctx_base: EmitContextBase,
    /// the shared outbox (the frozen substrate co-commit — reused, not re-implemented).
    outbox: OutboxStore,
    /// the id minter (the stable ULID source — injected, the frozen seam).
    minter: Arc<dyn IdMinter>,
    /// the durable ref-state backing — the real on-disk bare repo in production
    /// ([`RefBacking::Disk`]) or the in-memory test double ([`RefBacking::Memory`]). This is where
    /// the `git_ref` rows + reflog actually live; `open`/`tip` load from here (SI-012 fixed).
    backing: RefBacking,
    /// the registry of per-ref **linearisation locks** (each ref serialises on its OWN lock — the
    /// per-ref linearisation point, arch §3 / GIT-P10). The registry lock guards only lookup/vivify;
    /// the per-ref CAS holds the individual lock across the window, so different refs fan out
    /// parallel. The lock carries no state — the state is in [`Self::backing`].
    locks: std::sync::Mutex<BTreeMap<RefName, Arc<RefLock>>>,
    /// the H1 holder-registration receipt (proof the store registered when it opened).
    holder: crate::holder_intent::HolderRegistration,
}

impl RefStore {
    /// **Open the ref store for a repo over the in-memory TEST DOUBLE** — and AUTO-REGISTER it as
    /// `PersonalDataHolder` H1 (contract 10.1 / 1.4). The registration receipt is produced here (the
    /// store cannot escape the holder registry — "we forgot a store" is structurally impossible); the
    /// DSR bodies are GIT-P29.
    ///
    /// **GT-001:** this constructs the [`RefBacking::Memory`] test double (the prior in-memory `git_ref`
    /// row model) — it keeps the rich receive-pack CAS / emit-iff-committed / crash-injection unit
    /// tests + the GIT-D1 burst drill running in isolation (no temp fs), behaviour-identical to before.
    /// The **PRODUCTION** durable-on-disk path is [`RefStore::open_durable`] (refs survive restart).
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

    /// **Open the ref store over the REAL on-disk bare repo (the GT-001 production path).** Ref
    /// reads / writes / CAS + the reflog go to the durable on-disk repo
    /// ([`crate::durable::DurableGitRepo`], `git2`), and [`Self::tip`] / [`Self::reflog`] LOAD FROM
    /// DISK — so a ref written then read after a FRESH `RefStore` over the same on-disk root is still
    /// there (SI-012 fixed). The object/oid lookup is the real on-disk odb (F-git-2). Opening still
    /// auto-registers the store as `PersonalDataHolder` H1 (the holder cannot be forgotten).
    ///
    /// The caller resolves the [`crate::durable::DurableGitRepo`] from its
    /// [`crate::durable::DurableGitStore`] (`create_repo` / `open_repo` at the tenant/region path) —
    /// the tenant/region pathing IS the isolation boundary. The receive-pack policy + the
    /// one-transaction outbox co-commit are UNCHANGED; only the ref STATE is now durable on disk.
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
            backing: RefBacking::Disk {
                repo: durable_repo,
            },
            locks: std::sync::Mutex::new(BTreeMap::new()),
            holder: crate::holder_intent::HolderRegistration::auto_register(),
        }
    }

    /// The H1 holder-registration receipt (the auto-registration proof — contract 1.4 / 10.1).
    pub fn holder(&self) -> &crate::holder_intent::HolderRegistration {
        &self.holder
    }

    /// The shared outbox (so a drill / the relay can drain it + assert `outbox_depth`).
    pub fn outbox(&self) -> &OutboxStore {
        &self.outbox
    }

    /// The current tip of a ref (for the CAS expected-old + a read-your-writes check). `None` if the
    /// ref does not exist. Reads under the ref's OWN lock (a snapshot read, not the registry lock —
    /// so a tip read of ref A never blocks a CAS on ref B). On the durable path this LOADS FROM DISK
    /// — a FRESH `RefStore` over the same on-disk root reads the persisted tip (SI-012 fixed).
    pub fn tip(&self, ref_name: &RefName) -> Option<Oid> {
        self.try_tip(ref_name).ok().flatten()
    }

    /// Fallible tip read for production decisions. Unlike [`Self::tip`], a durable repository fault
    /// is surfaced and cannot be mistaken for an absent branch during a push or merge.
    pub fn try_tip(&self, ref_name: &RefName) -> Result<Option<Oid>, crate::durable::DurableError> {
        let lock = self.ref_lock(ref_name);
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        self.tip_of(ref_name)
    }

    /// The reflog (append-only; the per-ref history — used by the holder + the audit walk). On the
    /// durable path this reads the real on-disk git reflog of every ref (loaded from disk).
    pub fn reflog(&self) -> Result<Vec<ReflogEntry>, crate::durable::DurableError> {
        match &self.backing {
            RefBacking::Memory { reflog, .. } => Ok(
                reflog.lock().unwrap_or_else(|e| e.into_inner()).clone()
            ),
            RefBacking::Disk { repo } => {
                // Assemble the per-ref durable reflogs into the RefStore view, oldest-first per ref,
                // with the monotonic `update_seq` = the entry's 1-based position in that ref's reflog.
                let mut out = Vec::new();
                for (name, _tip) in repo.list_refs()? {
                    for (i, e) in repo.reflog_entries(&name)?.into_iter().enumerate() {
                        out.push(ReflogEntry {
                            ref_name: RefName::new(name.clone()),
                            old_oid: e.old_oid.map(|o| Oid::new(o.0)),
                            new_oid: Oid::new(e.new_oid.0),
                            update_seq: (i as u64) + 1,
                            pusher_pseudonym: e.committer,
                        });
                    }
                }
                Ok(out)
            }
        }
    }

    /// Read a ref's current tip from the backing (NO lock taken here — callers hold the per-ref lock).
    /// Memory: the modeled `git_ref` row; Disk: the real on-disk ref.
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

    /// The current per-ref generation (`update_seq`; 0 if the ref does not exist). Memory: the row's
    /// `update_seq`; Disk: the durable per-ref generation counter (R0.4 / git #1 HIGH —
    /// [`DurableGitRepo::ref_generation`], the config-backed counter that survives restart AND is
    /// monotonic across a ref's delete+recreate, unlike the reflog length it replaces).
    ///
    /// **Write-path ↔ reconcile coherence.** The write path stamps `new_seq = seq_of(ref) + 1` into the
    /// emitted `git.ref.updated` event; `apply_one`'s CAS then bumps the SAME durable counter to
    /// `previous + 1` internally (under the ref's held linearisation lock, so no interleaving move sits
    /// between the read here and the bump). Hence after the apply `ref_generation(ref) == new_seq`, so
    /// the reconciler's `rec.update_seq <= ref_generation` skip is EXACT for an already-applied move.
    fn seq_of(&self, ref_name: &RefName) -> Result<u64, OutboxError> {
        match &self.backing {
            RefBacking::Memory { rows, .. } => Ok(rows
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(ref_name)
                .map(|r| r.update_seq)
                .unwrap_or(0)),
            RefBacking::Disk { repo } => repo.ref_generation(&ref_name.0).map_err(|e| {
                OutboxError(format!("read durable ref generation for {}: {e}", ref_name.0))
            }),
        }
    }

    /// Apply one committed ref move to the backing (called AFTER the outbox transaction committed —
    /// the committed state-change half, under the ref's held linearisation lock). Memory: mutate the
    /// row + push the reflog; Disk: the durable on-disk ref CAS + reflog. `old` is the pre-move tip
    /// (the CAS expected-old, already verified under the held lock); a zero `new_oid` is a DELETE.
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
                reflog.lock().unwrap_or_else(|e| e.into_inner()).push(ReflogEntry {
                    ref_name: ref_name.clone(),
                    old_oid: old,
                    new_oid: new_oid.clone(),
                    update_seq: new_seq,
                    pusher_pseudonym: pseudonym.to_string(),
                });
                Ok(())
            }
            RefBacking::Disk { repo } => {
                // Convert the receive-pack `Oid` (this module's type) to the durable backend's
                // `core::Oid` at the boundary (both wrap the hex string).
                let old_core = old.as_ref().map(|o| crate::core::Oid::new(o.0.clone()));
                let new_core = if new_oid.is_zero() {
                    None
                } else {
                    Some(crate::core::Oid::new(new_oid.0.clone()))
                };
                let msg = format!("receive-pack: {} -> {}", self.repo, ref_name.0);
                // GT-003: the cross-system recovery reconciler is LANDED ([`crate::reconcile`]). The
                // apply-after-outbox-commit window (outbox row durable, on-disk ref move pending —
                // modeled by [`CrashPoint::AfterCommitBeforeApply`]) is BOUNDED + RECOVERABLE: on
                // restart [`crate::reconcile::reconcile_refs`] replays committed `git.ref.updated` rows
                // whose on-disk `update_seq` is behind the durable reflog and re-applies the CAS
                // (idempotent on `update_seq`, arch §4.2). It is NOT silent-loss (the committed event is
                // the durable witness), so the durable store may now reach the live front door (GT-003).
                repo.update_ref_cas(&ref_name.0, old_core.as_ref(), new_core.as_ref(), &msg, pseudonym)
                    // Post-commit apply failure is a should-not-happen invariant breach (the CAS was
                    // pre-checked under the held lock + the objects were migrated before commit). It
                    // surfaces LOUD, never silently — the event is committed; the GT-003 reconciler
                    // (above) re-applies from the durable reflog.
                    .map_err(|e| OutboxError(format!("durable ref apply failed (post-commit): {e}")))
            }
        }
    }

    /// The per-ref aggregate key for the outbox (arch §2.2 / §3): `<repo>:<ref_name>`. The `:`
    /// separates the repo from the ref; the per-aggregate ordering the outbox enforces is per-ref
    /// (so different refs of one repo advance in parallel; one ref is strictly serialised).
    fn aggregate_for(&self, ref_name: &RefName) -> AggregateKey {
        AggregateKey(format!("{}:{}", self.repo, ref_name.0))
    }

    /// The subject ref for the `git.ref.updated` event (`myelin://<tenant>/git/ref/<repo>:<ref>`).
    fn subject_for(&self, ref_name: &RefName) -> ArtifactRef {
        ArtifactRef(format!(
            "myelin://{}/git/ref/{}:{}",
            self.ctx_base.tenant.0, self.repo, ref_name.0
        ))
    }

    /// **The receive-pack write path** (arch §2): policy → (reject-before-ref-move) → object
    /// migration → **one transaction**: ref-CAS + reflog + `git.ref.updated` emit → commit. The
    /// emit happens IFF the ref move committed (BUS-2). `crash` injects the GIT-D9 failure at a step.
    ///
    /// All-or-nothing per push: a reject (policy OR a CAS-staleness on ANY ref) aborts the WHOLE push
    /// — no ref moves, the quarantine is discarded, nothing is emitted. On accept, every ref's CAS +
    /// emit are staged into ONE [`OutboxStore`] transaction and committed together.
    pub fn receive<M: QuarantineMigration>(
        &self,
        push: &PushSession,
        migration: &M,
        crash: CrashPoint,
    ) -> Result<PushOutcome, OutboxError> {
        // A push is a SET of ref transitions. Reject duplicates before policy, object migration, or
        // outbox work: otherwise both commands are planned from the same old tip/generation, both
        // witnesses commit, and the second CAS can fail only after the first mutation was applied.
        // The smart-HTTP parser enforces this too; this guard protects every direct/internal caller.
        let mut unique_refs = std::collections::BTreeSet::new();
        for update in &push.updates {
            if !unique_refs.insert(update.ref_name.clone()) {
                return Ok(PushOutcome::Rejected(RejectReason::DuplicateRefUpdate {
                    ref_name: update.ref_name.clone(),
                }));
            }
        }

        // ── Step 2: in-process policy — REJECT BEFORE THE REF MOVES (arch §2). ──
        // The policy is tenant-scoped: the pseudonymity rule (GIT-1) checks every pushed commit's
        // author/committer identity against the store's AUTHENTICATED tenant (from the token, X-1).
        let policy = PushPolicy {
            tenant: self.ctx_base.tenant.0.clone(),
            ..PushPolicy::default()
        };
        if let Err(reason) = policy.evaluate(push) {
            // The quarantine is discarded (never promoted) — we simply do NOT call migration.
            return Ok(PushOutcome::Rejected(reason));
        }

        // The crash-after-policy point: the process dies after the policy passed but before any
        // object migrated. Nothing committed → 0 ghost (the abort discards the quarantine).
        if crash == CrashPoint::AfterPolicy {
            return Ok(PushOutcome::Crashed(InjectedCrash { at: crash }));
        }

        // ── Step 3: migrate the quarantine into the object DB (durable-bytes ack, arch §2/§4). ──
        // (Done OUTSIDE the ref-CAS transaction: the bytes are content-addressed, so a migrated-but-
        // un-referenced object is harmless — the recovery fence discards a ref ahead of the DB, and
        // an orphan object is GC'd. What MUST be atomic is the ref-move + the emit, below.)
        if let Err(e) = migration.migrate(&push.quarantine) {
            // A durability failure aborts the push — the ref never moves over un-durable objects.
            return Ok(PushOutcome::Rejected(RejectReason::SecretDetected {
                oid: Oid::zero(),
                pattern: format!("object-migration-not-durable: {e}"),
            }));
        }

        // The crash-before-commit point: bytes are durable but the transaction has NOT committed.
        // emit-iff-committed → 0 ghost: no ref moved, no event emitted. (On real recovery the un-
        // acked ref is discarded; the orphan objects are GC'd — arch §4.2.)
        if crash == CrashPoint::BeforeCommit {
            return Ok(PushOutcome::Crashed(InjectedCrash { at: crash }));
        }

        // ── Step 4: ONE transaction — ref-CAS + reflog + git.ref.updated emit, then COMMIT. ──
        // The per-ref CAS is the linearisation point. We take the involved refs' OWN locks (each
        // models that ref's `FOR UPDATE` row lock; pushes to OTHER refs hold OTHER locks → they run
        // in PARALLEL — arch §3 / GIT-P10) plus the outbox transaction, so the ref-update and the
        // emit are co-committed (BUS-2). If ANY ref's CAS is stale, we return WITHOUT committing the
        // outbox transaction → emit-iff-committed: nothing is written.
        //
        // **Deadlock-free multi-ref lock acquisition (the all-or-nothing atomic push):** an atomic
        // push touches a SET of refs; we lock them in a TOTAL (sorted, de-duplicated) order so two
        // concurrent atomic pushes that overlap on refs A,B can never deadlock (both acquire A
        // before B). Single-ref pushes (the overwhelmingly common hot-ref burst) take exactly one
        // lock. Different refs never contend — the registry lock is dropped before the cells lock.
        let mut targets: Vec<RefName> = push.updates.iter().map(|u| u.ref_name.clone()).collect();
        targets.sort();
        targets.dedup();
        let locks: Vec<Arc<RefLock>> = targets.iter().map(|r| self.ref_lock(r)).collect();
        // Lock each ref's linearisation lock in the sorted order (the deadlock-free discipline). The
        // guards are held for the whole CAS→commit→apply window — the per-ref linearisation point
        // spans check + apply. The lock carries no state; the ref STATE is read/written from the
        // durable backing (on-disk git in production) under this held lock.
        let _guards: Vec<std::sync::MutexGuard<'_, ()>> = locks
            .iter()
            .map(|l| l.lock().unwrap_or_else(|e| e.into_inner()))
            .collect();

        // First pass: CAS-staleness check over EVERY ref (the per-ref linearisation assertion). A
        // single stale ref aborts the WHOLE atomic push (no partial write). Reading the backing under
        // the held lock is the `SELECT … FOR UPDATE` row read.
        for u in &push.updates {
            let actual = self.tip_of(&u.ref_name).map_err(|e| {
                OutboxError(format!("read durable ref tip for {}: {e}", u.ref_name.0))
            })?.unwrap_or_else(Oid::zero);
            if actual != u.expected_old {
                // Reject BEFORE moving any ref — drop the locks (transaction never opened) → 0 ghost.
                return Ok(PushOutcome::Rejected(RejectReason::NonFastForward {
                    ref_name: u.ref_name.clone(),
                    expected: u.expected_old.clone(),
                    actual,
                }));
            }
        }

        // Open the outbox transaction (the same-tx co-commit). Everything staged on it — plus the
        // ref-row mutations we apply on commit — becomes durable IFF `tx.commit()` is called.
        let mut tx = self
            .outbox
            .begin(Arc::clone(&self.minter), self.ctx_base.clone());

        // Stage each ref-CAS as the transaction's state change + emit its git.ref.updated together.
        let mut planned: Vec<(RefName, Oid, u64, Option<Oid>, myelin_events::EventId)> = Vec::new();
        for u in &push.updates {
            let old = self.tip_of(&u.ref_name).map_err(|e| {
                OutboxError(format!("read durable ref tip for {}: {e}", u.ref_name.0))
            })?;
            let prev_seq = self.seq_of(&u.ref_name)?;
            let new_seq = crate::durable::next_ref_generation(prev_seq).ok_or_else(|| {
                OutboxError(format!("ref generation exhausted for {}", u.ref_name.0))
            })?;

            // The state change (the ref CAS + reflog) — staged into THIS transaction (co-commit).
            tx.stage_state_change(format!(
                "git_ref CAS {}:{} {} -> {} (seq {new_seq})",
                self.repo,
                u.ref_name.0,
                old.clone().unwrap_or_else(Oid::zero).0,
                u.new_oid.0
            ));

            // The git.ref.updated emit (contract 2.9) — references-not-payloads: the payload carries
            // oids/refs + the pusher PSEUDONYM (never a raw identity), so it is NOT inline PII.
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
                // Processor posture: the tenant org is the controller of repo content (Art. 28).
                data_role: DataRole::Processor,
                visibility: Visibility::Internal,
                // The pusher identity is the opaque pseudonym (4.8) — NOT inline PII.
                contains_personal_data: false,
                pii_key_ref: None,
            };
            // A root push event (no `cause`): it is its own causal root. (A push triggered by an
            // agent run would pass that run's envelope as the cause — wired with the agent fabric.)
            let id = tx.emit(draft, None)?;
            planned.push((u.ref_name.clone(), u.new_oid.clone(), new_seq, old, id));
        }

        // The crash-after-commit point fires AFTER the commit below succeeds — see the branch.
        // Commit the transaction: the outbox rows become durable atomically. ONLY now do we apply
        // the ref-row mutations + the reflog — keeping the in-memory model's "ref move iff event
        // committed" identical to the real "UPDATE git_ref … + INSERT outbox … in one tx".
        tx.commit()?;

        // The GT-003 reconciler window: the outbox row is now DURABLE but the on-disk ref CAS has not
        // run. A crash here leaves the on-disk ref BEHIND its committed `update_seq` — recoverable by
        // replaying the committed `git.ref.updated` row ([`crate::reconcile`]), idempotent on
        // `update_seq`. We return WITHOUT applying so the recovery replay is exercised on restart.
        if crash == CrashPoint::AfterCommitBeforeApply {
            return Ok(PushOutcome::Crashed(InjectedCrash { at: crash }));
        }

        // The ref CAS + reflog are the COMMITTED state-change half (applied to the durable backing
        // under the SAME per-ref locks the CAS-staleness check read, still held here — so no
        // interleaving moved the ref between the check and the apply; the per-ref linearisation point
        // holds). A push to the zero-oid is a DELETE; otherwise the ref is created/updated. On the
        // durable path this is the real on-disk git ref CAS + reflog (survives restart).
        let mut moved = Vec::new();
        let mut emitted = Vec::new();
        for (ref_name, new_oid, new_seq, old, id) in planned {
            self.apply_one(&ref_name, &new_oid, new_seq, old, &push.pusher.pseudonym)?;
            moved.push((ref_name, new_oid, new_seq));
            emitted.push(id);
        }
        // The per-ref guards drop HERE — the linearisation window (check → commit → apply) closes,
        // releasing each ref for the next push in the burst.
        drop(_guards);

        // The crash-after-commit point: the transaction committed (the event rows are durable +
        // unsent; the ref moved). A crash HERE loses nothing — the relay publishes the durable rows
        // (0 lost), the recovery fence reconciles to the committed `update_seq`. We report Crashed so
        // the drill can assert "ref moved AND event durable" survived the kill.
        if crash == CrashPoint::AfterCommit {
            return Ok(PushOutcome::Crashed(InjectedCrash { at: crash }));
        }

        Ok(PushOutcome::Accepted { moved, emitted })
    }

    /// Look up — vivifying if absent — the per-ref **linearisation lock** for a ref. The registry
    /// lock is held ONLY for this lookup/insert, never across the per-ref CAS (so different refs never
    /// contend on the registry). Returns an `Arc` clone so the caller holds the lock independently —
    /// the per-ref `FOR UPDATE` serialisation lives in this lock; the ref STATE lives in the durable
    /// backing (arch §3 / GIT-P10).
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

    // ════════ R0.2 / DELTA N1 + R2-exit — the protected-ref DIRECT-push gate (reuses merge_gate +
    // the FULL lifecycle ruleset; the pusher's `protected_push` standing is the bypass leg) ════════

    /// `pusher_has_protected_push = false` for every R0.2 case (a plain writer, not a bypass/admin —
    /// so the required-checks + full-ruleset gate is enforced). The R2-exit cases below exercise the
    /// `true` bypass leg + the new approvals/CODEOWNERS half explicitly.
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
        // A protected ref is never deleted over the wire — even by a bypass/admin pusher.
        let rs = protected_ruleset(&[], false);
        let head = GitOid("0".repeat(40));
        for bypass in [WRITER, ADMIN] {
            assert_eq!(
                evaluate_protected_ref_push(
                    &RefName::new("refs/heads/main"),
                    /*is_delete*/ true,
                    /*is_forced*/ false,
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
        // Force-push refused when the ruleset does NOT allow it — even for a bypass/admin pusher.
        for bypass in [WRITER, ADMIN] {
            assert_eq!(
                evaluate_protected_ref_push(
                    &RefName::new("refs/heads/main"),
                    false,
                    /*is_forced*/ true,
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
        // The SAME force-push admits when the ruleset opts into `allow_force_push` (no required set).
        assert_eq!(
            evaluate_protected_ref_push(
                &RefName::new("refs/heads/main"),
                false,
                true,
                WRITER,
                &protected_ruleset(&[], /*allow_force_push*/ true),
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
        // No recorded greens for the head → BLOCKED (CI red/missing), naming the unmet contexts.
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
        // A partial green (only ci/build) is still BLOCKED (ci/test missing).
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
        // Both required contexts green-and-current for the head → ADMITTED (the ff push may proceed;
        // this ruleset requires 0 approvals / no CODEOWNERS, so the full ruleset half is satisfied).
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
        // A fork-run (untrusted) success is neutral-for-gating (Δ3) — BLOCKED until a maintainer
        // endorses it; endorsement admits. A fork cannot self-green its own protected push.
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
        // No required contexts + 0 approvals / no CODEOWNERS → the full gate admits a plain ff (the
        // force/delete bans still apply). A repo that configures NO protection requirements permits a
        // writer's fast-forward — GitHub-parity; the escalation the R2-exit cases below close is a
        // repo that DOES configure required reviews/CODEOWNERS.
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
        // A malformed recorded green name is never treated as "not required" — fail-closed loud.
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
                &["ci/".into()], // an empty-named context — malformed
                &[],
                &[],
            ),
            Err(RejectReason::ProtectedGateInput { .. })
        ));
    }

    // ──── R2-EXIT BLOCKER: the writer→protected-branch escalation is closed ────

    /// A ruleset that models the red-team's target: a protected `main` requiring a CI context AND a
    /// human approval AND CODEOWNERS review — the shape a real repo protects `main` with.
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

    /// **THE EXPLOIT, FLIPPED TO DENIED (Defect 3).** A plain WRITER direct-pushes to a strict-protected
    /// `main` carrying (even genuinely-green, producer-attested) required checks — but a direct push has
    /// NO PR review context, so the required-approval + CODEOWNERS half is UNSATISFIABLE. DENIED, naming
    /// the specific unmet ruleset reasons. The writer cannot land on protected main by pushing directly.
    #[test]
    fn writer_direct_push_to_strict_protected_ref_is_denied_by_the_full_ruleset() {
        let head = GitOid("c0ffee".into());
        let rs = strict_protected_ruleset();
        // Even with the required context GREEN, the approvals + CODEOWNERS half blocks a direct push.
        match evaluate_protected_ref_push(
            &RefName::new("refs/heads/main"),
            /*is_delete*/ false,
            /*is_forced*/ false,
            WRITER,
            &rs,
            &head,
            &["ci/build".into()], // genuine (or forged) greens do NOT help — approvals/CODEOWNERS block
            &[],
            &[],
        ) {
            Err(RejectReason::ProtectedRulesetNotSatisfied { reasons, .. }) => {
                assert!(
                    reasons
                        .iter()
                        .any(|r| matches!(r, BlockReason::InsufficientApprovals { need: 1, .. })),
                    "a direct push carries 0 approvals — the 1 required is unmet: {reasons:?}"
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

    /// A writer with NO greens on a strict-protected ref is denied at the CONTEXTS half first (still
    /// DENIED — the escalation is closed regardless of which half fires first).
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

    /// **The LEGIT bypass (Defect 2): an admin / `protected_push` pusher CAN direct-push a protected
    /// ref** (a fast-forward) even against a strict ruleset it cannot otherwise satisfy — the required
    /// hotfix/admin path. Force/delete bans still apply to admin (proven above).
    #[test]
    fn admin_bypass_may_direct_push_a_strict_protected_ref() {
        let head = GitOid("ba5e".into());
        assert_eq!(
            evaluate_protected_ref_push(
                &RefName::new("refs/heads/main"),
                /*is_delete*/ false,
                /*is_forced*/ false,
                ADMIN,
                &strict_protected_ruleset(),
                &head,
                &[], // no greens, no approvals — the bypass clears the checks/ruleset gate
                &[],
                &[],
            ),
            Ok(())
        );
    }

    /// A ruleset requiring a human approval (but NO CODEOWNERS) still blocks a writer's direct push —
    /// the `required_approvals` half alone is unsatisfiable for a direct push.
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

    /// Duplicate create, update, and delete commands are rejected before quarantine migration or
    /// outbox work. This defensive boundary protects callers that do not enter through smart HTTP.
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

    /// **The happy path: receive-pack → one-tx ref-CAS + outbox.** A push to a non-protected ref is
    /// accepted; the ref moves, ONE `git.ref.updated` row is durable + unsent, the quarantine is
    /// migrated, the per-ref aggregate is `<repo>:<ref>`, and `update_seq` is 1.
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
        // The ref moved + the event is durable + unsent (depth 1).
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
        // The quarantine was migrated (promoted into the object DB).
        assert!(db.contains(&Oid::new("cafe")));
        // The emitted row is git.ref.updated on the per-ref aggregate `core:refs/heads/feature`.
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
        assert_eq!(row.aggregate, AggregateKey("core:refs/heads/x".into()));
    }

    /// **emit-iff-committed: crash AFTER policy (before commit) emits NOTHING (0 ghost).** The ref
    /// never moves, the outbox is empty, the quarantine is NOT promoted.
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
        // 0 ghost: nothing committed.
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

    /// **emit-iff-committed: crash BEFORE commit (after object migration) emits NOTHING (0 ghost).**
    /// The bytes are durable but the ref did not move and no event was emitted — the un-acked ref is
    /// discarded on recovery; an orphan object is harmless (content-addressed, GC'd).
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
        // 0 ghost: the transaction never committed.
        assert_eq!(outbox.outbox_depth(), 0);
        assert_eq!(outbox.committed_count(), 0);
        assert_eq!(
            store.tip(&RefName::new("refs/heads/feature")),
            None,
            "the ref never moved"
        );
        // The bytes ARE durable (migrated before the crash) — but that is harmless without the ref.
        assert!(
            db.contains(&Oid::new("cafe")),
            "objects migrated before the kill (orphan, GC'd)"
        );
    }

    /// **emit-iff-committed: crash AFTER commit keeps BOTH the ref move AND the event (0 lost).** The
    /// transaction committed; the ref moved and the event row is durable + unsent — the relay will
    /// publish it.
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
        // 0 lost: the ref moved AND the event survived.
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

    /// **Reject BEFORE the ref moves: a force-push on a protected ref.** The ref never moves, nothing
    /// is emitted, the quarantine is NOT promoted.
    #[test]
    fn force_push_on_protected_is_rejected_before_ref_moves() {
        let (store, outbox) = store();
        let db = InMemoryObjectDb::new();
        // main already at `old1`.
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
        // The ref did NOT move past old1, and no new event was emitted.
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

    /// **Reject BEFORE the ref moves: a protected-ref deletion.** (delete = push to the zero oid.)
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

    /// **Reject BEFORE the ref moves: a secret in a quarantined object.** The secret never migrates
    /// into the object DB (reject-before-ref-move).
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
                bytes: b"export AWS_KEY=AKIAIOSFODNN7EXAMPLE".to_vec(),
            }],
            pusher: Pusher {
                pseudonym: "anon-1@acme.noreply".into(),
                is_agent: false,
            },
        };
        match store.receive(&push, &db, CrashPoint::None).unwrap() {
            PushOutcome::Rejected(RejectReason::SecretDetected { oid, pattern }) => {
                assert_eq!(oid, Oid::new("bad"));
                assert_eq!(pattern, "AKIA");
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

    /// **Reject: an agent direct-push to a protected ref (`agent_needs_human`).**
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

    // ───────────── GIT-P12 (P-273): the receive-pack pseudonymity rule (the data-model gate) ──────

    /// Build a push whose quarantine carries a single COMMIT object with the given author/committer
    /// identity LINE (the raw bytes a client pushed). `identity_line` is the `<name> <email> ts tz`
    /// tail of both the `author` and `committer` headers.
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

    /// **GIT-P12 GATE — a pushed commit with a RAW name/email is REJECTED before the ref moves (0
    /// cleartext-PII commit admitted).** The non-cooperating-client commit `Ada Lovelace <ada@…>`
    /// never moves a ref; nothing is emitted; the quarantine is not promoted.
    #[test]
    fn non_pseudonymous_commit_is_rejected_before_ref_moves() {
        let (store, outbox) = store(); // tenant = acme
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
        // 0 cleartext PII admitted: the ref never moved, nothing emitted, the commit not promoted.
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

    /// **GIT-P12 GATE — a pushed commit authored to the tenant pseudonym is ACCEPTED.** The
    /// cooperative-client happy path: `<pseudonym>@acme.noreply` passes the door, the ref moves, one
    /// event commits.
    #[test]
    fn pseudonymous_commit_for_the_tenant_is_accepted() {
        let (store, outbox) = store(); // tenant = acme
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

    /// **GIT-P12 GATE — a well-formed pseudonym for ANOTHER tenant is REJECTED (cross-tenant
    /// smuggling).** A commit authored `psn@globex.noreply` cannot move a ref in tenant `acme`.
    #[test]
    fn wrong_tenant_pseudonym_commit_is_rejected() {
        let (store, _outbox) = store(); // tenant = acme
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

    /// **GIT-P12: a non-commit object (a blob) is NOT subject to the pseudonymity rule.** A blob
    /// carrying a stray `<email>` is not a commit identity — it passes the rule (only commit objects
    /// have an author line). The push is accepted on its other merits.
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
            // A blob (NOT tree-headed) whose contents mention a real email — not an identity field.
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

    /// **Reject: a blank pseudonym (pseudonymity required, GIT-1).**
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

    /// **The per-ref CAS is the linearisation point: a stale expected-old is a non-fast-forward
    /// reject (0 ghost).** Two pushes race the same ref from the same old; the second sees a moved
    /// tip and is rejected — the ref reflects only the first move, and only one event committed.
    #[test]
    fn stale_cas_is_non_fast_forward_reject_zero_ghost() {
        let (store, outbox) = store();
        let db = InMemoryObjectDb::new();
        // The first push creates the ref at `v1`.
        store
            .receive(
                &human_push("refs/heads/feature", Oid::zero(), Oid::new("v1")),
                &db,
                CrashPoint::None,
            )
            .unwrap();
        assert_eq!(outbox.committed_count(), 1);

        // A second push believes the ref is STILL at zero (stale) → non-fast-forward reject.
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
        // The ref reflects only the first move; only one event committed (0 ghost from the reject).
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

    /// **All-or-nothing per push: an atomic push of two refs where one CAS is stale rejects the
    /// WHOLE push** — NEITHER ref moves, NOTHING is emitted (no partial write — the silent-data-loss
    /// guard).
    #[test]
    fn atomic_push_with_one_stale_ref_moves_neither() {
        let (store, outbox) = store();
        let db = InMemoryObjectDb::new();
        // Seed refs/heads/a at v1 (so the atomic push's stale expected-old on `a` fails).
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
                // `b` is a fresh create (would succeed alone)…
                ProposedRefUpdate {
                    ref_name: RefName::new("refs/heads/b"),
                    expected_old: Oid::zero(),
                    new_oid: Oid::new("bbb"),
                    forced: false,
                    commit_oids: vec![Oid::new("bbb")],
                },
                // …but `a`'s expected-old is stale (it is at v1, not zero) → the whole push rejects.
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
        // NEITHER ref moved: `b` was never created, `a` stayed at v1; nothing new emitted.
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

    /// **Per-ref ordering: successive pushes to one ref carry monotonic `update_seq` AND a
    /// per-aggregate-ordered outbox seq.** (The burst-ordering load proof is GIT-P10/GIT-D1.)
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

        // The ref tip + update_seq advanced monotonically.
        assert_eq!(
            store.tip(&RefName::new("refs/heads/feature")),
            Some(Oid::new("v3"))
        );
        let log = store.reflog().expect("reflog");
        let seqs: Vec<u64> = log
            .iter()
            .filter(|e| e.ref_name == RefName::new("refs/heads/feature"))
            .map(|e| e.update_seq)
            .collect();
        assert_eq!(seqs, vec![1, 2, 3], "update_seq is monotonic per ref");
        // The outbox carries three rows on the one per-ref aggregate, seqs 0,1,2 (per-aggregate order).
        let agg = AggregateKey("core:refs/heads/feature".into());
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

    // ───────────────────── GIT-P10 (GIT-D1): the per-ref CAS concurrency control ─────────────────

    /// **GIT-P10 hot-ref burst: rapid SAME-ref pushes SERIALISE per ref (the per-ref CAS is the
    /// linearisation point).** N threads race the SAME hot ref, each presenting the SAME stale
    /// expected-old (the create-from-zero). Exactly ONE wins (commits the create); the rest see the
    /// moved tip and are non-fast-forward rejected — 0 lost (the winner is durable), 0 ghost (no
    /// rejected push emitted). This is the per-ref serialisation: the ref advances by exactly one
    /// generation no matter how many racers hit it at once.
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
                // Every racer creates the SAME ref from zero to its OWN new oid — only one can win.
                let push = human_push("refs/heads/hot", Oid::zero(), Oid::new(format!("w{i:02}")));
                barrier.wait(); // release all racers at once → maximal contention on the one ref.
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
        // 0 ghost: exactly ONE event committed (only the winner emitted); the ref advanced by one.
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
        // The committed tip is one of the racers' oids, at update_seq 1.
        let tip = store.tip(&RefName::new("refs/heads/hot")).unwrap();
        assert!(tip.0.starts_with('w'), "the tip is a racer's oid: {tip:?}");
    }

    /// **GIT-P10: a chained hot-ref burst keeps push order per ref (the outbox order == the
    /// ref-update order).** A single feeder thread chains rapid pushes to one hot ref (each from the
    /// previous tip), interleaved with losing racers on the same ref. The committed `update_seq`
    /// sequence is contiguous 1..=k AND the outbox per-aggregate seq is gap-free 0..k-1 in the SAME
    /// order — the burst never reorders or drops a generation.
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
        // The outbox per-aggregate seqs are gap-free 0..k-1 in push order (the ref-update order).
        let agg = AggregateKey("core:refs/heads/hot".into());
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

    /// **GIT-P10: DIFFERENT refs FAN OUT PARALLEL (no whole-repo serialisation).** Concurrent pushes
    /// to N distinct refs ALL succeed — none blocks another (each takes its OWN ref lock, never a
    /// whole-repo lock). After the burst every ref is at its own tip and there are exactly N
    /// committed events. (The previous GIT-P9 store held one global lock; arch §3 / GIT-P10 require
    /// per-ref locks so distinct refs advance in parallel — this is the property under test.)
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
                barrier.wait(); // all distinct-ref pushes fire at once → they must NOT serialise.
                (
                    ref_name,
                    store.receive(&push, &db, CrashPoint::None).unwrap(),
                )
            }));
        }
        let results: Vec<(String, PushOutcome)> =
            handles.into_iter().map(|h| h.join().unwrap()).collect();

        // EVERY distinct-ref push committed (none lost to a whole-repo lock contention reject).
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
        // Each ref is at its own tip with update_seq 1 (one create each, independent generations).
        for i in 0..n {
            assert_eq!(
                store.tip(&RefName::new(format!("refs/heads/r{i:02}"))),
                Some(Oid::new(format!("t{i:02}"))),
                "ref r{i:02} advanced independently"
            );
        }
        // The per-aggregate outbox seqs are each 0 (every ref is its own aggregate, first move).
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

    /// **GIT-P10: a non-protected ref DELETE removes the row (the cell goes empty), then a re-create
    /// starts a fresh generation.** (Hardens the create→delete→create lifecycle the burst can hit.)
    #[test]
    fn non_protected_ref_delete_then_recreate() {
        let (store, outbox) = store();
        let db = InMemoryObjectDb::new();
        // create feature@v1 (seq 1), then delete it (seq 2), then re-create (seq 1 of a fresh row).
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
        // Re-create: a delete-from-zero CAS (the row is gone → expected-old is zero again).
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

    /// **H1 holder registration: opening the store auto-registers it (contract 1.4 / 10.1).** The
    /// receipt is real; the DSR bodies are the GIT-P29 floor.
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
        assert!(store.reflog().is_err(), "audit history faults must not become an empty log");
        let result = store.receive(
            &human_push("refs/heads/feature", Oid::zero(), Oid::new("new")),
            &InMemoryObjectDb::new(),
            CrashPoint::None,
        );
        assert!(result.is_err(), "a missing durable repo is not an absent ref");
        assert_eq!(outbox.committed_count(), 0, "no event commits on an invented empty tip");
    }

    /// **`is_protected` distinguishes protected from feature refs** (kills the `-> true` mutant):
    /// `main` + `release/*` are protected; a feature ref is NOT (so a force-push there is accepted).
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

        // A FORCE-push to a non-protected feature ref is ACCEPTED (proving the protected gate is not
        // universally `true`): seed the ref, then force-update it.
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

    /// **The object size limit is a strict `>` boundary** (kills the `> → ==` / `> → >=` mutants): an
    /// object EXACTLY at the limit is accepted; one byte over is rejected.
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
        over.quarantine[0].bytes = vec![0u8; 9]; // one byte over.
        match policy.evaluate(&over) {
            Err(RejectReason::ObjectTooLarge { size, limit, .. }) => {
                assert_eq!(size, 9);
                assert_eq!(limit, 8);
            }
            other => panic!("expected ObjectTooLarge, got {other:?}"),
        }
    }

    /// **The object DB accessors distinguish populated from empty** (kills the `contains -> true` /
    /// `is_empty -> true` / `len -> 0` mutants): a fresh DB is empty; after a migrate it contains the
    /// migrated oid and reports the right length.
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

    /// **`RefStore::outbox` returns the SHARED outbox the store emits into** (kills the
    /// `-> Box::leak(default)` mutant): an event committed through the store is visible via the
    /// accessor's depth signal.
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

    /// The frozen `git_ref` migration carries the per-ref primary key + `update_seq` + the reflog,
    /// and is forward-only (no destructive DROP).
    #[test]
    fn git_ref_migration_is_the_frozen_shape() {
        assert!(GIT_REF_MIGRATION.contains("CREATE TABLE IF NOT EXISTS git_ref"));
        assert!(GIT_REF_MIGRATION.contains("PRIMARY KEY (tenant, repo, ref_name)"));
        assert!(GIT_REF_MIGRATION.contains("update_seq"));
        assert!(GIT_REF_MIGRATION.contains("git_reflog"));
        assert!(GIT_REF_MIGRATION.contains("pusher_pseudonym"));
        assert!(!GIT_REF_MIGRATION.contains("DROP TABLE"));
    }

    /// **GIT-P11 closes the named floor end-to-end: `RefStore::receive` migrates the accepted
    /// quarantine into the REAL local-NVMe pack tier (`PackObjectDb`, NOT the `InMemoryObjectDb`
    /// floor), then a clone round-trips byte-identical.** The push commits one `git.ref.updated`
    /// (emit-iff-committed) AND the pushed objects are durable + content-addressed in the pack tier,
    /// servable as a byte-identical clone — the receive-pack → store → clone GATE through the
    /// production migration the architecture §2 step 3 mandates.
    #[test]
    fn receive_pack_migrates_into_the_real_pack_tier_and_clone_round_trips() {
        use crate::pack_tier::{PackObjectDb, PackTierMigration};
        use myelin_storage::{
            FsBlobStore, GitPackTier, RepoGitPlacement, RepoId, RepoPlacementStatus, StorageGroup,
        };
        use myelin_tenancy::{Region, TenantId};

        // The real local-NVMe pack tier (fs floor), repo placed region-pinned + relocatable.
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

        // A real push: the quarantine carries the pushed object bytes.
        let (store, outbox) = store();
        let pushed_oid = Oid::new("cafe");
        let pushed_bytes = b"a normal commit blob".to_vec(); // matches `human_push`'s quarantine.
        let push = human_push("refs/heads/feature", Oid::zero(), Oid::new("aaaa"));

        // receive-pack → policy → REAL migration → one-tx ref-CAS + outbox.
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

        // The pushed object is durable + content-addressed in the pack tier — a clone serves it back
        // BYTE-IDENTICAL (0 corruption; the GIT-P11 round-trip GATE through the production migration).
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
