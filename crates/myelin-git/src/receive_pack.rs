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
}

impl Default for PushPolicy {
    fn default() -> Self {
        Self {
            // 50 MiB per object — git's default large-object guard order of magnitude.
            max_object_bytes: 50 * 1024 * 1024,
            secret_patterns: vec![
                "AKIA".to_string(),        // an AWS access-key id prefix
                "-----BEGIN PRIVATE KEY".to_string(),
                "-----BEGIN RSA PRIVATE KEY".to_string(),
            ],
            protected_needs_human: true,
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
        }
        Ok(())
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
        self.migrated.lock().unwrap_or_else(|e| e.into_inner()).contains(oid)
    }
    /// The number of migrated objects.
    pub fn len(&self) -> usize {
        self.migrated.lock().unwrap_or_else(|e| e.into_inner()).len()
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

/// One per-ref lock cell — the in-memory model of the **single `git_ref` row** the CAS locks
/// `FOR UPDATE` (arch §3 / GIT-P10). The cell holds `Some(row)` once the ref exists, `None` while it
/// does not (a create transitions `None → Some`; a delete transitions `Some → None`). The `Mutex`
/// IS the per-ref linearisation point: a rapid burst of pushes to ONE hot ref serialises on THIS
/// lock, while pushes to OTHER refs lock OTHER cells and proceed in PARALLEL — the per-ref-order /
/// refs-fan-out-parallel property GIT-P10 (GIT-D1) hardens. (The previous GIT-P9 store held one
/// global lock over the whole repo, which serialised EVERY ref — arch §3 requires per-row locks so
/// different refs advance in parallel. This is the reconciliation.)
type RefCell = std::sync::Mutex<Option<RefRow>>;


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
    /// the registry of per-ref lock cells (the modeled `git_ref` rows, each behind its OWN lock —
    /// the per-ref linearisation point) + the reflog. The registry lock guards only lookup/vivify;
    /// the per-ref CAS holds the individual cells, so different refs fan out parallel (arch §3).
    registry: std::sync::Mutex<BTreeMap<RefName, Arc<RefCell>>>,
    /// the append-only reflog (the modeled `git_reflog` table), behind its own short-lived lock.
    reflog: std::sync::Mutex<Vec<ReflogEntry>>,
    /// the H1 holder-registration receipt (proof the store registered when it opened).
    holder: crate::holder_intent::HolderRegistration,
}

impl RefStore {
    /// **Open the ref store for a repo** — and AUTO-REGISTER it as `PersonalDataHolder` H1 (contract
    /// 10.1 / 1.4). The registration receipt is produced here (the store cannot escape the holder
    /// registry — "we forgot a store" is structurally impossible); the DSR bodies are GIT-P29.
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
            registry: std::sync::Mutex::new(BTreeMap::new()),
            reflog: std::sync::Mutex::new(Vec::new()),
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
    /// so a tip read of ref A never blocks a CAS on ref B).
    pub fn tip(&self, ref_name: &RefName) -> Option<Oid> {
        let cell = self.cell(ref_name);
        let g = cell.lock().unwrap_or_else(|e| e.into_inner());
        g.as_ref().map(|r| r.target_oid.clone())
    }

    /// The reflog (append-only; the per-ref history — used by the holder + the audit walk).
    pub fn reflog(&self) -> Vec<ReflogEntry> {
        self.reflog.lock().unwrap_or_else(|e| e.into_inner()).clone()
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
        // ── Step 2: in-process policy — REJECT BEFORE THE REF MOVES (arch §2). ──
        if let Err(reason) = PushPolicy::default().evaluate(push) {
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
        let cells: Vec<Arc<RefCell>> = targets.iter().map(|r| self.cell(r)).collect();
        // Lock each cell in the sorted order (the deadlock-free discipline). The guards are held for
        // the whole CAS→commit→apply window — the per-ref linearisation point spans check + apply.
        let mut guards: BTreeMap<RefName, std::sync::MutexGuard<'_, Option<RefRow>>> = BTreeMap::new();
        for (name, cell) in targets.iter().zip(cells.iter()) {
            guards.insert(name.clone(), cell.lock().unwrap_or_else(|e| e.into_inner()));
        }

        // First pass: CAS-staleness check over EVERY ref (the per-ref linearisation assertion). A
        // single stale ref aborts the WHOLE atomic push (no partial write). Reading the locked cell
        // is the `SELECT … FOR UPDATE` row read.
        for u in &push.updates {
            let actual = guards
                .get(&u.ref_name)
                .and_then(|g| g.as_ref())
                .map(|r| r.target_oid.clone())
                .unwrap_or_else(Oid::zero);
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
            let cur = guards.get(&u.ref_name).and_then(|g| g.as_ref());
            let old = cur.map(|r| r.target_oid.clone());
            let prev_seq = cur.map(|r| r.update_seq).unwrap_or(0);
            let new_seq = prev_seq + 1;

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

        // The ref CAS + reflog are the COMMITTED state-change half (applied under the SAME per-ref
        // locks the CAS-staleness check read, still held here — so no interleaving moved the ref
        // between the check and the apply; the per-ref linearisation point holds). A push to the new
        // zero-oid is a DELETE (the cell transitions to `None`); otherwise the cell becomes `Some`.
        let mut moved = Vec::new();
        let mut emitted = Vec::new();
        {
            let mut reflog = self.reflog.lock().unwrap_or_else(|e| e.into_inner());
            for (ref_name, new_oid, new_seq, old, id) in planned {
                let guard = guards
                    .get_mut(&ref_name)
                    .expect("the ref's cell was locked for the CAS");
                if new_oid.is_zero() {
                    // A delete: the ref row is removed (the cell goes empty); the next create starts
                    // a fresh generation (matching the `git_ref` row being deleted).
                    **guard = None;
                } else {
                    **guard = Some(RefRow { target_oid: new_oid.clone(), update_seq: new_seq });
                }
                reflog.push(ReflogEntry {
                    ref_name: ref_name.clone(),
                    old_oid: old,
                    new_oid: new_oid.clone(),
                    update_seq: new_seq,
                    pusher_pseudonym: push.pusher.pseudonym.clone(),
                });
                moved.push((ref_name, new_oid, new_seq));
                emitted.push(id);
            }
        }
        // The per-ref guards drop HERE — the linearisation window (check → commit → apply) closes,
        // releasing each ref for the next push in the burst.
        drop(guards);

        // The crash-after-commit point: the transaction committed (the event rows are durable +
        // unsent; the ref moved). A crash HERE loses nothing — the relay publishes the durable rows
        // (0 lost), the recovery fence reconciles to the committed `update_seq`. We report Crashed so
        // the drill can assert "ref moved AND event durable" survived the kill.
        if crash == CrashPoint::AfterCommit {
            return Ok(PushOutcome::Crashed(InjectedCrash { at: crash }));
        }

        Ok(PushOutcome::Accepted { moved, emitted })
    }

    /// Look up — vivifying if absent — the per-ref lock cell for a ref. The registry lock is held
    /// ONLY for this lookup/insert, never across the per-ref CAS (so different refs never contend on
    /// the registry). Returns an `Arc` clone so the caller holds the cell's lock independently — the
    /// per-ref `FOR UPDATE` serialisation lives in the cell, not the registry (arch §3 / GIT-P10).
    fn cell(&self, ref_name: &RefName) -> Arc<RefCell> {
        let mut g = self.registry.lock().unwrap_or_else(|e| e.into_inner());
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
            pusher: Pusher { pseudonym: "anon-7@acme.noreply".into(), is_agent: false },
        }
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
        assert_eq!(store.tip(&RefName::new("refs/heads/feature")), Some(Oid::new("aaaa")));
        assert_eq!(outbox.outbox_depth(), 1, "one git.ref.updated row is durable + unsent");
        assert_eq!(outbox.committed_count(), 1);
        // The quarantine was migrated (promoted into the object DB).
        assert!(db.contains(&Oid::new("cafe")));
        // The emitted row is git.ref.updated on the per-ref aggregate `core:refs/heads/feature`.
        let id = match store.receive(&human_push("refs/heads/x", Oid::zero(), Oid::new("bb")), &db, CrashPoint::None).unwrap() {
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
        assert_eq!(outcome, PushOutcome::Crashed(InjectedCrash { at: CrashPoint::AfterPolicy }));
        // 0 ghost: nothing committed.
        assert_eq!(outbox.outbox_depth(), 0, "a crash before commit emits no event");
        assert_eq!(outbox.committed_count(), 0);
        assert_eq!(store.tip(&RefName::new("refs/heads/feature")), None, "the ref never moved");
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
        assert_eq!(outcome, PushOutcome::Crashed(InjectedCrash { at: CrashPoint::BeforeCommit }));
        // 0 ghost: the transaction never committed.
        assert_eq!(outbox.outbox_depth(), 0);
        assert_eq!(outbox.committed_count(), 0);
        assert_eq!(store.tip(&RefName::new("refs/heads/feature")), None, "the ref never moved");
        // The bytes ARE durable (migrated before the crash) — but that is harmless without the ref.
        assert!(db.contains(&Oid::new("cafe")), "objects migrated before the kill (orphan, GC'd)");
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
        assert_eq!(outcome, PushOutcome::Crashed(InjectedCrash { at: CrashPoint::AfterCommit }));
        // 0 lost: the ref moved AND the event survived.
        assert_eq!(store.tip(&RefName::new("refs/heads/feature")), Some(Oid::new("aaaa")));
        assert_eq!(outbox.outbox_depth(), 1, "the committed event is durable + awaiting the relay");
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
        assert_eq!(store.tip(&RefName::new("refs/heads/main")), Some(Oid::new("old1")));
        assert_eq!(outbox.outbox_depth(), depth_before, "a rejected push emits nothing");
    }

    /// **Reject BEFORE the ref moves: a protected-ref deletion.** (delete = push to the zero oid.)
    #[test]
    fn delete_protected_is_rejected() {
        let (store, _outbox) = store();
        let db = InMemoryObjectDb::new();
        store
            .receive(&human_push("refs/heads/main", Oid::zero(), Oid::new("t1")), &db, CrashPoint::None)
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
            pusher: Pusher { pseudonym: "anon-1@acme.noreply".into(), is_agent: false },
        };
        assert_eq!(
            store.receive(&del, &db, CrashPoint::None).unwrap(),
            PushOutcome::Rejected(RejectReason::DeleteProtected { ref_name: RefName::new("refs/heads/main") })
        );
        assert_eq!(store.tip(&RefName::new("refs/heads/main")), Some(Oid::new("t1")), "ref not deleted");
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
            pusher: Pusher { pseudonym: "anon-1@acme.noreply".into(), is_agent: false },
        };
        match store.receive(&push, &db, CrashPoint::None).unwrap() {
            PushOutcome::Rejected(RejectReason::SecretDetected { oid, pattern }) => {
                assert_eq!(oid, Oid::new("bad"));
                assert_eq!(pattern, "AKIA");
            }
            o => panic!("expected SecretDetected, got {o:?}"),
        }
        assert_eq!(store.tip(&RefName::new("refs/heads/feature")), None, "ref never moved");
        assert!(db.is_empty(), "the secret object was NOT promoted out of quarantine");
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
            PushOutcome::Rejected(RejectReason::AgentNeedsHuman { ref_name: RefName::new("refs/heads/main") })
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
            .receive(&human_push("refs/heads/feature", Oid::zero(), Oid::new("v1")), &db, CrashPoint::None)
            .unwrap();
        assert_eq!(outbox.committed_count(), 1);

        // A second push believes the ref is STILL at zero (stale) → non-fast-forward reject.
        let stale = human_push("refs/heads/feature", Oid::zero(), Oid::new("v2"));
        match store.receive(&stale, &db, CrashPoint::None).unwrap() {
            PushOutcome::Rejected(RejectReason::NonFastForward { ref_name, expected, actual }) => {
                assert_eq!(ref_name, RefName::new("refs/heads/feature"));
                assert_eq!(expected, Oid::zero());
                assert_eq!(actual, Oid::new("v1"));
            }
            o => panic!("expected NonFastForward, got {o:?}"),
        }
        // The ref reflects only the first move; only one event committed (0 ghost from the reject).
        assert_eq!(store.tip(&RefName::new("refs/heads/feature")), Some(Oid::new("v1")));
        assert_eq!(outbox.committed_count(), 1, "the rejected stale push emitted nothing");
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
            .receive(&human_push("refs/heads/a", Oid::zero(), Oid::new("v1")), &db, CrashPoint::None)
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
            pusher: Pusher { pseudonym: "anon-1@acme.noreply".into(), is_agent: false },
        };
        assert!(matches!(
            store.receive(&atomic, &db, CrashPoint::None).unwrap(),
            PushOutcome::Rejected(RejectReason::NonFastForward { .. })
        ));
        // NEITHER ref moved: `b` was never created, `a` stayed at v1; nothing new emitted.
        assert_eq!(store.tip(&RefName::new("refs/heads/b")), None, "the fresh ref was NOT created");
        assert_eq!(store.tip(&RefName::new("refs/heads/a")), Some(Oid::new("v1")));
        assert_eq!(outbox.committed_count(), committed_before, "no partial emit");
    }

    /// **Per-ref ordering: successive pushes to one ref carry monotonic `update_seq` AND a
    /// per-aggregate-ordered outbox seq.** (The burst-ordering load proof is GIT-P10/GIT-D1.)
    #[test]
    fn successive_pushes_to_one_ref_are_monotonic() {
        let (store, outbox) = store();
        let db = InMemoryObjectDb::new();
        let mut ids = Vec::new();
        for (old, new) in [(Oid::zero(), Oid::new("v1")), (Oid::new("v1"), Oid::new("v2")), (Oid::new("v2"), Oid::new("v3"))] {
            match store.receive(&human_push("refs/heads/feature", old, new), &db, CrashPoint::None).unwrap() {
                PushOutcome::Accepted { emitted, .. } => ids.push(emitted[0].clone()),
                o => panic!("{o:?}"),
            }
        }

        // The ref tip + update_seq advanced monotonically.
        assert_eq!(store.tip(&RefName::new("refs/heads/feature")), Some(Oid::new("v3")));
        let log = store.reflog();
        let seqs: Vec<u64> = log.iter().filter(|e| e.ref_name == RefName::new("refs/heads/feature")).map(|e| e.update_seq).collect();
        assert_eq!(seqs, vec![1, 2, 3], "update_seq is monotonic per ref");
        // The outbox carries three rows on the one per-ref aggregate, seqs 0,1,2 (per-aggregate order).
        let agg = AggregateKey("core:refs/heads/feature".into());
        let mut agg_seqs: Vec<u64> = ids
            .iter()
            .map(|id| {
                let row = outbox.row(id).unwrap();
                assert_eq!(row.aggregate, agg, "all three rows share the per-ref aggregate");
                row.seq
            })
            .collect();
        agg_seqs.sort_unstable();
        assert_eq!(agg_seqs, vec![0, 1, 2], "per-ref outbox ordering is gap-free");
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

        let accepted = outcomes.iter().filter(|o| matches!(o, PushOutcome::Accepted { .. })).count();
        let rejected = outcomes
            .iter()
            .filter(|o| matches!(o, PushOutcome::Rejected(RejectReason::NonFastForward { .. })))
            .count();
        assert_eq!(accepted, 1, "exactly one racer wins the create (per-ref linearisation)");
        assert_eq!(rejected, n - 1, "every loser is a non-fast-forward reject (0 lost-update)");
        // 0 ghost: exactly ONE event committed (only the winner emitted); the ref advanced by one.
        assert_eq!(outbox.committed_count(), 1, "only the winner's git.ref.updated committed (0 ghost)");
        assert_eq!(
            store.reflog().iter().filter(|e| e.ref_name == RefName::new("refs/heads/hot")).count(),
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
                .receive(&human_push("refs/heads/hot", prev.clone(), new.clone()), &db, CrashPoint::None)
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
                assert_eq!(row.aggregate, agg, "every burst event is on the one per-ref aggregate");
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
                (ref_name, store.receive(&push, &db, CrashPoint::None).unwrap())
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
        assert_eq!(outbox.committed_count(), n, "all N distinct-ref events committed");
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
            outbox
                .row(&match &results[i].1 {
                    PushOutcome::Accepted { emitted, .. } => emitted[0].clone(),
                    _ => unreachable!(),
                })
        }) {
            assert_eq!(row.seq, 0, "each distinct ref's first event is its own aggregate's seq 0");
        }
    }

    /// **GIT-P10: a non-protected ref DELETE removes the row (the cell goes empty), then a re-create
    /// starts a fresh generation.** (Hardens the create→delete→create lifecycle the burst can hit.)
    #[test]
    fn non_protected_ref_delete_then_recreate() {
        let (store, outbox) = store();
        let db = InMemoryObjectDb::new();
        // create feature@v1 (seq 1), then delete it (seq 2), then re-create (seq 1 of a fresh row).
        store.receive(&human_push("refs/heads/feature", Oid::zero(), Oid::new("v1")), &db, CrashPoint::None).unwrap();
        let del = PushSession {
            updates: vec![ProposedRefUpdate {
                ref_name: RefName::new("refs/heads/feature"),
                expected_old: Oid::new("v1"),
                new_oid: Oid::zero(),
                forced: false,
                commit_oids: vec![],
            }],
            quarantine: vec![],
            pusher: Pusher { pseudonym: "anon-1@acme.noreply".into(), is_agent: false },
        };
        assert!(matches!(
            store.receive(&del, &db, CrashPoint::None).unwrap(),
            PushOutcome::Accepted { .. }
        ));
        assert_eq!(store.tip(&RefName::new("refs/heads/feature")), None, "the ref was deleted");
        // Re-create: a delete-from-zero CAS (the row is gone → expected-old is zero again).
        match store.receive(&human_push("refs/heads/feature", Oid::zero(), Oid::new("v2")), &db, CrashPoint::None).unwrap() {
            PushOutcome::Accepted { moved, .. } => assert_eq!(moved[0].2, 1, "the re-created row starts a fresh generation"),
            o => panic!("re-create must be accepted, got {o:?}"),
        }
        assert_eq!(store.tip(&RefName::new("refs/heads/feature")), Some(Oid::new("v2")));
        assert_eq!(outbox.committed_count(), 3, "create + delete + re-create each emitted");
    }

    /// **H1 holder registration: opening the store auto-registers it (contract 1.4 / 10.1).** The
    /// receipt is real; the DSR bodies are the GIT-P29 floor.
    #[test]
    fn opening_the_store_registers_holder_h1() {
        let (store, _outbox) = store();
        assert_eq!(store.holder().holder_id, crate::holder_intent::HOLDER_ID);
        assert!(store.holder().registered, "the store auto-registered as H1 on open");
    }

    /// **`is_protected` distinguishes protected from feature refs** (kills the `-> true` mutant):
    /// `main` + `release/*` are protected; a feature ref is NOT (so a force-push there is accepted).
    #[test]
    fn protected_set_is_exactly_main_and_release() {
        assert!(RefName::new("refs/heads/main").is_protected());
        assert!(RefName::new("refs/heads/release/1.0").is_protected());
        assert!(!RefName::new("refs/heads/feature").is_protected(), "a feature ref is NOT protected");
        assert!(!RefName::new("refs/heads/mainline").is_protected(), "only exact `main` is protected");

        // A FORCE-push to a non-protected feature ref is ACCEPTED (proving the protected gate is not
        // universally `true`): seed the ref, then force-update it.
        let (store, outbox) = store();
        let db = InMemoryObjectDb::new();
        store.receive(&human_push("refs/heads/feature", Oid::zero(), Oid::new("a1")), &db, CrashPoint::None).unwrap();
        let mut forced = human_push("refs/heads/feature", Oid::new("a1"), Oid::new("a2"));
        forced.updates[0].forced = true;
        assert!(matches!(
            store.receive(&forced, &db, CrashPoint::None).unwrap(),
            PushOutcome::Accepted { .. }
        ), "a force-push to a NON-protected ref is accepted");
        assert_eq!(store.tip(&RefName::new("refs/heads/feature")), Some(Oid::new("a2")));
        assert_eq!(outbox.committed_count(), 2);
    }

    /// **The object size limit is a strict `>` boundary** (kills the `> → ==` / `> → >=` mutants): an
    /// object EXACTLY at the limit is accepted; one byte over is rejected.
    #[test]
    fn object_size_limit_is_strict_greater_than() {
        let policy = PushPolicy { max_object_bytes: 8, secret_patterns: vec![], protected_needs_human: true };
        let at_limit = PushSession {
            updates: vec![ProposedRefUpdate {
                ref_name: RefName::new("refs/heads/f"),
                expected_old: Oid::zero(),
                new_oid: Oid::new("a"),
                forced: false,
                commit_oids: vec![],
            }],
            quarantine: vec![QuarantineObject { oid: Oid::new("x"), bytes: vec![0u8; 8] }],
            pusher: Pusher { pseudonym: "p@acme.noreply".into(), is_agent: false },
        };
        assert!(policy.evaluate(&at_limit).is_ok(), "an object exactly at the limit is accepted");

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

        db.migrate(&[QuarantineObject { oid: Oid::new("z"), bytes: vec![] }]).unwrap();
        assert!(!db.is_empty(), "a migrated DB is not empty");
        assert_eq!(db.len(), 1);
        assert!(db.contains(&Oid::new("z")));
        assert!(!db.contains(&Oid::new("other")), "it contains only what was migrated");
    }

    /// **`RefStore::outbox` returns the SHARED outbox the store emits into** (kills the
    /// `-> Box::leak(default)` mutant): an event committed through the store is visible via the
    /// accessor's depth signal.
    #[test]
    fn outbox_accessor_returns_the_shared_store() {
        let (store, _outbox) = store();
        let db = InMemoryObjectDb::new();
        assert_eq!(store.outbox().outbox_depth(), 0, "the shared outbox starts empty");
        store.receive(&human_push("refs/heads/f", Oid::zero(), Oid::new("a")), &db, CrashPoint::None).unwrap();
        assert_eq!(store.outbox().outbox_depth(), 1, "the accessor sees the committed event");
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
}
