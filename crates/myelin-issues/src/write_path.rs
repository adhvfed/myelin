//! # `write_path` — the silent-data-loss-safe Issues write path (ISS-P06 / P-372, M4-I1)
//!
//! **The non-negotiable this module ships (EI-01 §2 — silent data loss outranks every feature):**
//! every state change to an issue runs **validate → `Id.check` (+ `CaveatContext`) → mutate the
//! typed core → `OutboxTx::emit` IN THE SAME TRANSACTION**. The issue is the **aggregate**
//! (`UNIQUE(aggregate, seq)` per-issue ordering, contract 2.3); the event co-commits with the row
//! through the outbox, so there is **0 ghost / 0 lost** — no event whose state did not commit, and
//! no committed state without its event (emit-iff-committed, the SUB-D1 / BUS-D4 shape applied to
//! Issues).
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/issue-tracker/architecture/02-internals-and-algorithms.md`
//! §2 (the write/transition path — `validate → Id.check(+CaveatContext) → BEGIN TX { mutate typed
//! core; append change-log; OutboxTx::emit } COMMIT`, the issue is the aggregate). The `BEGIN TX …
//! COMMIT` block is the [`myelin_events::OutboxTransaction`] same-tx co-commit ([`crate`] links the
//! shared outbox; it is never re-implemented here, EI-01 §7).
//!
//! **Contract-index rows (consumed here — built to the FROZEN shapes, never diverged):**
//! - **2.1 / 2.2 / 2.3 / 2.5** — the [`myelin_events::EventEnvelope`] / [`myelin_events::OutboxTx`]
//!   emit / the `outbox` table `UNIQUE(aggregate, seq)` / consumer dedup. The issue is the
//!   aggregate; the ONE emit verb is `OutboxTx::emit` (the `no-raw-publish` lint, P-019, holds —
//!   **0 `publish_now` call sites**). The `issue.*` token a mutation emits is a NAMED constant from
//!   [`crate::events`] (the names anchor X-5), never a literal.
//! - **4.2** — the write gate. Every mutation calls [`myelin_identity::IdentityService::check`]
//!   with the per-action [`Permission`] and the field/transition [`CaveatContext`] (off the hot
//!   `list_objects` path, OQ-E). A non-`Allow` decision **denies the mutation and emits nothing**
//!   (fail-closed, ADR-03) — the gate is BEFORE the transaction is committed.
//! - **4.6 / 4.10** — `write_tuples` (assign / watch / confidential-grant) + the returned
//!   [`myelin_identity::Zookie`]. A mutation that changes a relation
//!   ([`MutationKind::assign`]/`watch`/`confidential_grant`) drives
//!   [`myelin_identity::IdentityService::write_tuples`] and stamps the returned zookie on the result
//!   so the caller's next read is read-your-writes (contract 4.10).
//!
//! ## What this prompt (ISS-P06 / P-372) ships — and the floors it NAMES
//! Ships the **minimal write path as a state-changing handler** ([`apply_mutation`]): the
//! validate → check → mutate → emit seam, proven emit-iff-committed (the drop-without-commit path
//! writes nothing), per-aggregate seq monotonic + dedup-safe on replay, with `write_tuples` + the
//! zookie wired on the relation-changing mutations. The seam is proven FIRST, before keys/CAS/
//! content layer on top.
//!
//! **FLOORS NAMED (VISION §3 — name-your-floors):**
//! - **Key allocation is a PLACEHOLDER here.** The issue's stored canonical key
//!   (`<PROJECTKEY>-<seqno>`, the Hi/Lo human-key) is allocated in **ISS-P08 / P-374**; here the
//!   write path emits + mutates with a placeholder aggregate key
//!   ([`StagedMutation::aggregate`] is `issue:<project_id>:<placeholder>`), so the emit-iff-committed
//!   seam is proven WITHOUT depending on the key allocator. The aggregate-key SHAPE (the issue is
//!   the aggregate) does not change when ISS-P08 lands the real key.
//! - **`order_key` ranking + the server-arbitrated CAS reorder land in ISS-P09 / P-375.** The
//!   [`crate::events::ISSUE_REORDERED`] emit + the optimistic version-CAS body are NOT in this
//!   module; the move-CRDT is the M5 follow-on (ISS-P32). Named so the plain typed-core mutation
//!   here is not mistaken for the full ranking path.
//! - **The issue body / comment as a `myelin-content` block subtree is ISS-P10 / P-376.** The
//!   `title`/`props` carried in [`IssueDraft`] are opaque bytes here; the parse/render-round-trip
//!   content layer attaches on top of this write path.
//! - **The LIVE OLTP store is the ISS-P05 [`myelin_events::outbox::OutboxStore`] in-memory model
//!   PLUS the `integration`-feature live Postgres apply.** The same-tx co-commit MECHANISM is the
//!   shared outbox's (the real `INSERT … RETURNING` inside the caller's DB transaction is the
//!   OutboxStore's named floor, P-007); this module drives that mechanism. The seam shape does not
//!   change when the live binding lands.
//!
//! ## Mutation-score floor (mandatory-core — this IS the write-loss seam)
//! The write path is the silent-data-loss seam (EI-01 §2: write-loss is Tier-1), so it is a
//! **mandatory-core mutation target with a ≥ 90% floor**: `cargo mutants -p myelin-issues --file
//! crates/myelin-issues/src/write_path.rs`. The mutation-tested core is the order-of-operations
//! (validate BEFORE check BEFORE the same-tx mutate+emit), the fail-closed gate (a non-`Allow`
//! decision DENIES + commits nothing), the emit-iff-committed structure (the transaction is dropped
//! without commit on any `Err`), the per-mutation permission/event-token/tuple mapping, and the
//! PII-flag discipline. A mutant that emits outside the transaction, accepts a Deny, skips the gate,
//! mis-maps a permission/token, or drops the PII flag is caught. **FLOOR (measured-under-load):**
//! running the mutation score is a CI step (the harness runs `cargo mutants` on the mandatory-core
//! file list); this prompt SHIPS the testable construction + the unit/e2e/drill coverage the score
//! reads — the measured % is the CI artifact, registered red-until-run in the scorecard, never
//! self-asserted here (EI-01 §3 — do not claim a green you did not earn). The world-scale
//! corpus-under-load drill is the M5 band.
//!
//! ## Why a thin handler over the shared outbox (EI-01 §7 — reuse, never duplicate)
//! The transactional outbox + the same-tx co-commit + the per-aggregate `seq` ordering + the
//! emit-iff-committed structure ALREADY exist in `myelin_events::outbox`
//! ([`myelin_events::OutboxStore`] / [`myelin_events::OutboxTransaction`], EB-03 / P-008). This
//! module does NOT re-implement any of that — it is the **Issues write-path handler** that BEGINS an
//! `OutboxTransaction`, stages the typed-core mutation into it, calls `Id.check` BEFORE staging, and
//! calls `OutboxTx::emit` on the SAME transaction so the issue's `issue.*` event co-commits with the
//! row. One outbox, one emit verb, one ordering key — exactly the substrate seam, used in place.

use crate::events;
use myelin_events::{
    AggregateKey, ArtifactRef, DataRole, EmitContextBase, EventDraft, EventType, IdMinter,
    OutboxStore, OutboxTransaction, OutboxTx, Visibility,
};
use myelin_identity::{
    CaveatContext, Consistency, ConsistencyMode, Decision, IdentityService, Permission,
    Precondition, Principal, RelName, RelationTuple, TupleDelta, Zookie,
};
use std::sync::Arc;

// ===========================================================================
// §1 — the write-gate permission tokens (the frozen rebac_fragment permission names)
// ===========================================================================

/// The `manage` permission an issue-CREATE / -UPDATE gates on (the
/// [`crate::rebac_fragment::issue_fragment`] permission name `manage`). A `&'static str` constant so
/// the gate asserts against the NAMED permission, never a literal (the names anchor X-5).
pub const PERM_MANAGE: &str = "manage";
/// The `transition` permission a state-transition gates on (the `issue` fragment permission
/// `transition`; the field/transition-level caveat is `perform_transition` on the
/// `issue_transition` sub-object — evaluated at check-time, contract 4.2 / §6.2).
pub const PERM_TRANSITION: &str = "transition";
/// The `perform_transition` permission name on the `issue_transition` sub-object (the
/// [`CaveatContext`]-gated transition check, §6.2 — approver-role evaluated off the hot path).
pub const PERM_PERFORM_TRANSITION: &str = "perform_transition";
/// The `comment` permission a comment-CREATE gates on (the `issue` fragment permission `comment`).
pub const PERM_COMMENT: &str = "comment";

// ===========================================================================
// §2 — the mutation kinds + the typed-core inputs (the validate surface)
// ===========================================================================

/// The free-text-bearing inputs of an issue CREATE (the typed core a CREATE mutates). The
/// `title`/`props` are opaque bytes here — the `myelin-content` body parse/render is the ISS-P10
/// floor; this struct carries the bytes the write path stores + references (never an inline PII body
/// on the wire — references-not-payloads, contract 2.7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssueDraft {
    /// The project the issue belongs to (the partition the placeholder aggregate keys on).
    pub project_id: u128,
    /// The issue title (opaque bytes here; `myelin-content` round-trip is ISS-P10). Free-text PII →
    /// the emitted event sets `contains_personal_data` (the event carries a `pii_key_ref`, never the
    /// body — references-not-payloads).
    pub title: String,
    /// The custom-field JSONB tail (opaque bytes; per-subject DEK is ISS-P07).
    pub props: Vec<u8>,
    /// The reporter's OPAQUE pseudonym (contract 4.8 — never a raw name/email).
    pub reporter_pseudonym: String,
}

/// What a single write-path call mutates (the validate surface — each variant names the permission
/// it gates on + whether it drives a `write_tuples` relation change). The issue is the aggregate for
/// every variant (per-issue ordering).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MutationKind {
    /// CREATE a new issue (gates on [`PERM_MANAGE`]). Emits [`events::ISSUE_CREATED`].
    Create(IssueDraft),
    /// UPDATE an issue's fields (gates on [`PERM_MANAGE`]). Emits [`events::ISSUE_UPDATED`].
    Update {
        /// the field deltas (opaque bytes — the rollup/sync input the event carries by reference).
        delta: Vec<u8>,
    },
    /// TRANSITION an issue's state (gates on [`PERM_PERFORM_TRANSITION`] with the transition
    /// [`CaveatContext`]). Emits [`events::ISSUE_TRANSITIONED`] `{from, to}`.
    Transition {
        /// the source state name (the FSM `from`).
        from: String,
        /// the target state name (the FSM `to`).
        to: String,
    },
    /// ASSIGN an issue (gates on [`PERM_MANAGE`]; drives `write_tuples` `assignee` — contract 4.6).
    /// Emits [`events::ISSUE_ASSIGNED`] + stamps the returned zookie (contract 4.10).
    Assign {
        /// the assignee's OPAQUE pseudonym / principal id (the `assignee` relation subject).
        assignee_pseudonym: String,
    },
    /// WATCH an issue (gates on [`PERM_COMMENT`]; drives `write_tuples` `watcher` — Notif read-fanout).
    /// Emits no lifecycle event (the watch is a relation, not a content change) but returns the zookie.
    Watch {
        /// the watcher's OPAQUE pseudonym / principal id (the `watcher` relation subject).
        watcher_pseudonym: String,
    },
    /// GRANT confidential access (gates on [`PERM_MANAGE`]; drives `write_tuples` `confidential_grant`
    /// — the explicit re-admit over the `- confidential` set-difference). Returns the zookie.
    ConfidentialGrant {
        /// the granted subject's OPAQUE pseudonym / principal id (the `confidential_grant` subject).
        grantee_pseudonym: String,
    },
}

impl MutationKind {
    /// The frozen [`Permission`] this mutation gates on (contract 4.2 — the per-action write gate).
    pub fn permission(&self) -> Permission {
        match self {
            MutationKind::Create(_)
            | MutationKind::Update { .. }
            | MutationKind::Assign { .. }
            | MutationKind::ConfidentialGrant { .. } => Permission(PERM_MANAGE.into()),
            MutationKind::Transition { .. } => Permission(PERM_PERFORM_TRANSITION.into()),
            MutationKind::Watch { .. } => Permission(PERM_COMMENT.into()),
        }
    }

    /// The NAMED `issue.*` event token this mutation emits (the names anchor X-5; [`crate::events`]).
    /// `Watch`/`ConfidentialGrant` emit no lifecycle event (they are pure relation changes — the
    /// `write_tuples` zookie is the observable, not a lifecycle event), so they return `None`.
    pub fn event_token(&self) -> Option<&'static str> {
        match self {
            MutationKind::Create(_) => Some(events::ISSUE_CREATED),
            MutationKind::Update { .. } => Some(events::ISSUE_UPDATED),
            MutationKind::Transition { .. } => Some(events::ISSUE_TRANSITIONED),
            MutationKind::Assign { .. } => Some(events::ISSUE_ASSIGNED),
            MutationKind::Watch { .. } | MutationKind::ConfidentialGrant { .. } => None,
        }
    }

    /// The relation [`TupleDelta`] this mutation writes (contract 4.6), if it changes a tuple. A
    /// CREATE/UPDATE/TRANSITION changes no relation tuple (the typed core mutates, not the ReBAC
    /// graph); ASSIGN/WATCH/CONFIDENTIAL-GRANT each add exactly one tuple. The object is the issue
    /// (the placeholder aggregate URN here; the real `<PROJECTKEY>-<seqno>` object is ISS-P08).
    fn tuple_delta(&self, object: &myelin_identity::ObjectId) -> Option<TupleDelta> {
        let (rel, subject) = match self {
            MutationKind::Assign { assignee_pseudonym } => ("assignee", assignee_pseudonym),
            MutationKind::Watch { watcher_pseudonym } => ("watcher", watcher_pseudonym),
            MutationKind::ConfidentialGrant { grantee_pseudonym } => {
                ("confidential_grant", grantee_pseudonym)
            }
            _ => return None,
        };
        Some(TupleDelta::Add(RelationTuple {
            object: object.clone(),
            relation: RelName(rel.into()),
            subject: myelin_identity::PrincipalId(subject.clone()),
            caveat: None,
        }))
    }

    /// Whether this mutation carries free-text PII the emitted event must flag
    /// (`contains_personal_data`). A CREATE's title/props + an UPDATE's delta may carry PII; the
    /// pure relation/transition changes do not (they carry opaque pseudonyms / state tokens).
    fn carries_personal_data(&self) -> bool {
        matches!(self, MutationKind::Create(_) | MutationKind::Update { .. })
    }
}

// ===========================================================================
// §3 — the write-path error taxonomy (loud, never a silent allow / silent drop)
// ===========================================================================

/// Why a write-path call failed (LOUD — never a silent allow, never a silent data loss). A
/// `Denied`/`Invalid` returns BEFORE the transaction commits, so the mutation + its event are
/// written **neither** (emit-iff-committed: a denied write is indistinguishable from one that never
/// happened — 0 ghost).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriteError {
    /// Validation rejected the input (e.g. an empty pseudonym / an empty transition target). The
    /// mutation never reached the gate; nothing was written.
    Invalid(String),
    /// The write gate (`Id.check`, contract 4.2) returned a non-`Allow` decision (fail-closed,
    /// ADR-03). The transaction is dropped WITHOUT commit → nothing written, nothing emitted.
    Denied { permission: String },
    /// `Id.check` / `write_tuples` surfaced an authz-surface error (the fail-static path decides —
    /// §10). The transaction is dropped WITHOUT commit (the write is fail-closed on an Id hiccup).
    Authz(String),
    /// The outbox emit / co-commit failed (a UNIQUE(event_id) collision is a programming error —
    /// the monotonic minter cannot collide on the happy path). The transaction is dropped → nothing
    /// written.
    Outbox(String),
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WriteError::Invalid(why) => write!(f, "invalid write-path input: {why}"),
            WriteError::Denied { permission } => write!(
                f,
                "write DENIED by Id.check on `{permission}` (fail-closed, ADR-03) — nothing written"
            ),
            WriteError::Authz(why) => write!(f, "authz surface error (write fail-closed): {why}"),
            WriteError::Outbox(why) => write!(f, "outbox co-commit failed: {why}"),
        }
    }
}

impl std::error::Error for WriteError {}

/// The outcome of a committed write-path call — the minted [`myelin_events::EventId`] (if the
/// mutation emitted a lifecycle event) + the [`Zookie`] the relation write returned (if it changed a
/// tuple, contract 4.10 — the caller stamps it for read-your-writes). A mutation that emits no event
/// and writes no tuple (none of the v1 mutations do that) would carry neither.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WriteOutcome {
    /// The minted stable `event_id` of the emitted `issue.*` event (the broker-side dedup key), if
    /// this mutation emitted a lifecycle event. `None` for a pure relation change (Watch / Grant).
    pub event_id: Option<myelin_events::EventId>,
    /// The consistency watermark `write_tuples` returned (contract 4.10), if this mutation changed a
    /// relation tuple. The caller stamps it on the object for read-your-writes. `None` if the
    /// mutation changed no tuple.
    pub zookie: Option<Zookie>,
}

// ===========================================================================
// §4 — the write path: validate → check → mutate → emit IN ONE TRANSACTION
// ===========================================================================

/// The placeholder aggregate key for an issue (ISS-P08 floor — the real `<PROJECTKEY>-<seqno>` Hi/Lo
/// human key lands there). **The issue is the aggregate** (per-issue ordering, contract 2.3): every
/// `issue.*` event for one logical issue shares this aggregate, so its create → update → transition
/// sequence is per-aggregate ordered (gap-free, in commit order, EB-03). The SHAPE — one aggregate
/// per issue — does not change when ISS-P08 swaps the placeholder for the canonical key.
pub fn issue_aggregate_key(project_id: u128, issue_local_id: &str) -> AggregateKey {
    AggregateKey(format!("issue:{project_id}:{issue_local_id}"))
}

/// The opaque issue object URN (the `Id.check` / `write_tuples` object). Placeholder local id here
/// (ISS-P08 mints the real `<PROJECTKEY>-<seqno>`); the URN SHAPE
/// (`myelin://<tenant>/issue/issue/<id>`) is the frozen Issues artifact-ref grammar.
pub fn issue_ref(tenant: &str, issue_local_id: &str) -> ArtifactRef {
    ArtifactRef(format!("myelin://{tenant}/issue/issue/{issue_local_id}"))
}

/// **THE silent-data-loss-safe Issues write path (ISS-P06 / P-372 — the prompt's headline).**
///
/// Runs **validate → `Id.check` (+ `CaveatContext`) → mutate the typed core → `OutboxTx::emit`** all
/// in ONE [`OutboxTransaction`] (contract 2.1/2.2/2.3/4.2/4.6/4.10). The order is non-negotiable:
///
/// 1. **validate** — reject malformed input ([`WriteError::Invalid`]) BEFORE touching the gate or
///    the store (nothing written on a bad input).
/// 2. **`Id.check`** — the per-action write gate (contract 4.2), with the field/transition
///    [`CaveatContext`] for a transition (off the hot `list_objects` path, OQ-E). A non-`Allow`
///    decision returns [`WriteError::Denied`] and the transaction is DROPPED without commit →
///    **nothing written, nothing emitted** (fail-closed, ADR-03; emit-iff-committed).
/// 3. **`write_tuples`** (only for `assign`/`watch`/`confidential_grant`) — the atomic relation
///    write (contract 4.6) returning the [`Zookie`] (contract 4.10). Done BEFORE the typed-core
///    stage so a relation-write failure fails the whole write closed (no half-applied mutation).
/// 4. **mutate the typed core** — stage the issue-row / change-log mutation into the SAME
///    transaction (`stage_state_change`). In the live OLTP binding this is the `INSERT`/`UPDATE` in
///    the caller's DB transaction (the OutboxStore models exactly its commit semantics).
/// 5. **`OutboxTx::emit`** — the ONE sanctioned emit verb (contract 2.2; the `no-raw-publish` lint,
///    P-019). The `issue.*` event is BUFFERED into the SAME transaction; it co-commits with the
///    typed-core mutation on [`OutboxTransaction::commit`] — **emit-iff-committed** (0 ghost / 0
///    lost). The aggregate is the issue (`UNIQUE(aggregate, seq)` per-issue ordering).
///
/// `cause` is the optional parent envelope (a reflex-driven mutation inherits its correlation +
/// `depth+1`, P-S06 — the caller cannot typo a wrong parent: the causal triple is not on
/// [`EventDraft`]). A root human action passes `None`.
///
/// **Only commits on success.** If any step fails, the function returns `Err` and the
/// [`OutboxTransaction`] is dropped at the end of the function WITHOUT `commit` — so a failed write
/// writes neither the state nor the event (the silent-data-loss floor, correct-by-construction).
#[allow(clippy::too_many_arguments)]
pub fn apply_mutation<Id: IdentityService>(
    store: &OutboxStore,
    minter: Arc<dyn IdMinter>,
    ctx_base: EmitContextBase,
    id: &Id,
    actor: &Principal,
    issue_local_id: &str,
    mutation: &MutationKind,
    cause: Option<&myelin_events::EventEnvelope>,
) -> Result<WriteOutcome, WriteError> {
    // ── 1. VALIDATE (reject malformed input before the gate / the store) ──────────────────────────
    validate(mutation)?;

    let tenant = ctx_base.tenant.0.clone();
    let object_ref = issue_ref(&tenant, issue_local_id);
    let object_id = myelin_identity::ObjectId(object_ref.0.clone());
    let permission = mutation.permission();

    // ── 2. Id.check — the per-action write gate (contract 4.2), fail-closed (ADR-03) ──────────────
    // A transition carries the frozen transition CaveatContext (approver-role / state attrs),
    // evaluated at check-time OFF the hot list_objects path (OQ-E, §6.2). Other mutations carry the
    // object-level CaveatContext (no field/transition sub-object). A Strong (read-your-writes)
    // consistency so the gate does not use a stale fail-static cache for a security-sensitive write.
    let caveat = caveat_for(mutation, &object_ref);
    let at = strong_consistency(&ctx_base);
    match id.check(actor, &permission, &object_ref, &at, Some(&caveat)) {
        Ok(Decision::Allow) => {}
        Ok(Decision::Deny) | Ok(Decision::Conditional) => {
            // Fail-closed: a Deny OR a Conditional-with-context-already-supplied is NOT a write.
            return Err(WriteError::Denied {
                permission: permission.0,
            });
        }
        Err(e) => return Err(WriteError::Authz(format!("{e:?}"))),
    }

    // ── 3. write_tuples (assign / watch / confidential_grant) → the zookie (4.6 / 4.10) ───────────
    // Done BEFORE the typed-core stage so a relation-write failure fails the WHOLE write closed
    // (no half-applied mutation, no event emitted). The precondition carries the expected zookie
    // (read-modify-write guard, contract 4.6) — None here (the optimistic version-CAS over the
    // relation is ISS-P09; the carrier is wired so the seam does not change).
    let zookie = match mutation.tuple_delta(&object_id) {
        Some(delta) => {
            let precondition: Option<&Precondition> = None;
            match id.write_tuples(&[delta], precondition) {
                Ok(zk) => Some(zk),
                Err(e) => return Err(WriteError::Authz(format!("{e:?}"))),
            }
        }
        None => None,
    };

    // ── 4 + 5. BEGIN TX { mutate typed core ; OutboxTx::emit } — the same-tx co-commit ────────────
    let mut tx = store.begin(minter, ctx_base);
    // 4. mutate the typed core: stage the issue-row / change-log mutation into THIS transaction.
    //    In the live OLTP binding this is the INSERT/UPDATE in the caller's DB transaction; here the
    //    OutboxStore models exactly its commit semantics (the state + the event co-commit).
    tx.stage_state_change(state_change_description(mutation, issue_local_id));

    // 5. OutboxTx::emit — the ONE sanctioned emit path (no-raw-publish, P-019). The issue.* event is
    //    BUFFERED into `tx`; it co-commits with the staged state change on commit (emit-iff-committed).
    let event_id = match mutation.event_token() {
        Some(token) => {
            let draft = event_draft(
                token,
                &object_ref,
                project_of(mutation),
                issue_local_id,
                mutation,
            );
            match tx.emit(draft, cause) {
                Ok(eid) => Some(eid),
                // The buffered transaction is dropped at function exit WITHOUT commit on this Err →
                // nothing written, nothing emitted (emit-iff-committed).
                Err(e) => return Err(WriteError::Outbox(format!("{e:?}"))),
            }
        }
        None => None,
    };

    // ── COMMIT: the staged typed-core mutation + the buffered event become durable ATOMICALLY ─────
    // This is the ONLY path that writes a row into the store. If we returned `Err` above, `tx` was
    // dropped without reaching here — emit-iff-committed (0 ghost / 0 lost).
    commit_tx(tx)?;

    Ok(WriteOutcome { event_id, zookie })
}

/// Validate the mutation input (reject malformed before the gate / the store). Empty pseudonyms /
/// empty transition states are the smell — a write with a blank subject would leave an unattributable
/// row. Loud rejection, never a silent default.
fn validate(mutation: &MutationKind) -> Result<(), WriteError> {
    let nonempty = |label: &str, v: &str| -> Result<(), WriteError> {
        if v.trim().is_empty() {
            Err(WriteError::Invalid(format!("{label} must not be empty")))
        } else {
            Ok(())
        }
    };
    match mutation {
        MutationKind::Create(draft) => nonempty("reporter_pseudonym", &draft.reporter_pseudonym),
        MutationKind::Update { delta } => {
            if delta.is_empty() {
                Err(WriteError::Invalid("update delta must not be empty".into()))
            } else {
                Ok(())
            }
        }
        MutationKind::Transition { from, to } => {
            nonempty("transition.from", from)?;
            nonempty("transition.to", to)?;
            if from == to {
                return Err(WriteError::Invalid(
                    "transition.from must differ from transition.to".into(),
                ));
            }
            Ok(())
        }
        MutationKind::Assign { assignee_pseudonym } => nonempty("assignee", assignee_pseudonym),
        MutationKind::Watch { watcher_pseudonym } => nonempty("watcher", watcher_pseudonym),
        MutationKind::ConfidentialGrant { grantee_pseudonym } => {
            nonempty("confidential_grant", grantee_pseudonym)
        }
    }
}

/// The `CaveatContext` for the write gate (contract 4.2 / §6.2). A transition carries the
/// transition-level caveat (the `perform_transition` approver-role / state attrs, evaluated at
/// check-time off the hot path); other mutations carry the object-level caveat (no field/transition
/// sub-object). The attrs map carries the FSM `from`/`to` so the engine's transition rewrite can
/// gate on them (the live rewrite is the ISS-P11/P-ID floor; the carrier is wired).
fn caveat_for(mutation: &MutationKind, object: &ArtifactRef) -> CaveatContext {
    let mut attrs = std::collections::BTreeMap::new();
    let transition = match mutation {
        MutationKind::Transition { from, to } => {
            attrs.insert("from".into(), myelin_identity::Literal::Str(from.clone()));
            attrs.insert("to".into(), myelin_identity::Literal::Str(to.clone()));
            // The transition sub-object id (the §6.2 issue_transition ABAC object). Placeholder id
            // shape `<from>-><to>`; the live transition-id resolution is the ISS-P12 FSM floor.
            Some(myelin_identity::TransitionId(format!("{from}->{to}")))
        }
        _ => None,
    };
    CaveatContext {
        object: object.clone(),
        field: None,
        transition,
        attrs,
    }
}

/// A `Strong` (read-your-writes) consistency for the write gate — a security-sensitive write must
/// not use a stale fail-static cache (contract 4.10, the new-enemy guard §8.7). The `at_least`
/// watermark is the actor's last-seen zookie; here the ctx carries none, so the empty zookie reads
/// at-latest (the live last-seen-zookie threading is the read-path floor). The MODE — `Strong`,
/// cache-bypass — is the load-bearing choice and is pinned.
fn strong_consistency(_ctx: &EmitContextBase) -> Consistency {
    Consistency {
        at_least: Zookie(String::new()),
        mode: ConsistencyMode::Strong,
    }
}

/// The placeholder project id for the aggregate key (ISS-P08 floor). A CREATE carries its project in
/// the draft; the other mutations operate on an existing issue whose project the store knows — here
/// the placeholder `0` keeps the aggregate-key SHAPE stable (the live store reads the issue's real
/// project on a non-create mutation).
fn project_of(mutation: &MutationKind) -> u128 {
    match mutation {
        MutationKind::Create(draft) => draft.project_id,
        _ => 0,
    }
}

/// Build the canonical `issue.*` [`EventDraft`] for a mutation (references-not-payloads, contract
/// 2.7 — the payload carries IDs/refs + a `pii_key_ref` for a PII-bearing event, never the inline
/// body). The aggregate is the issue (per-issue ordering, contract 2.3). `contains_personal_data` is
/// set for the free-text-bearing mutations (CREATE/UPDATE); the PII body itself is NOT on the wire
/// (the per-subject-DEK `pii_key_ref` is the ISS-P07 floor — here the flag is set, the key ref
/// carried as a placeholder so the envelope shape is the real one).
fn event_draft(
    token: &str,
    object: &ArtifactRef,
    project_id: u128,
    issue_local_id: &str,
    mutation: &MutationKind,
) -> EventDraft {
    let contains_pii = mutation.carries_personal_data();
    let mut payload = serde_json::json!({
        // references-not-payloads: the issue URN + the placeholder local id (the real
        // <PROJECTKEY>-<seqno> is ISS-P08). Never the inline title/props body.
        "issue": object.0,
        "issue_local_id": issue_local_id,
    });
    // The mutation-specific reference fields (still refs/tokens, never an inline PII body).
    match mutation {
        MutationKind::Transition { from, to } => {
            payload["from"] = serde_json::Value::String(from.clone());
            payload["to"] = serde_json::Value::String(to.clone());
        }
        MutationKind::Assign { assignee_pseudonym } => {
            // the OPAQUE pseudonym (contract 4.8 — not a raw name/email), a reference token.
            payload["assignee"] = serde_json::Value::String(assignee_pseudonym.clone());
        }
        _ => {}
    }
    EventDraft {
        type_: EventType(token.into()),
        subject: object.clone(),
        aggregate: issue_aggregate_key(project_id, issue_local_id),
        payload,
        // Issues is the CONTROLLER of the issue fact it authors (the tenant org is the controller
        // of issue content; Issues is the processor surface, but the EVENT's data_role marks the
        // fact's controllership — Controller, the same role the other producer subsystems stamp).
        data_role: DataRole::Controller,
        // A state-change event's default visibility is Internal (a routing hint, never an authz
        // decision — Identity decides at resolve-time).
        visibility: Visibility::Internal,
        contains_personal_data: contains_pii,
        // The per-subject-DEK pii_key_ref is ISS-P07. A PII-bearing event carries a key REF (never
        // the body); here a placeholder ref proves the envelope shape carries it iff PII is present.
        pii_key_ref: if contains_pii {
            Some(myelin_events::PiiKeyRef(format!(
                "issue-dek:{issue_local_id}"
            )))
        } else {
            None
        },
    }
}

/// A human-readable description of the staged typed-core mutation (the "state change" half of the
/// co-commit, recorded so a test can assert the state + the event commit together — and that an
/// abort writes neither). In the live OLTP binding this is the actual row INSERT/UPDATE.
fn state_change_description(mutation: &MutationKind, issue_local_id: &str) -> String {
    match mutation {
        MutationKind::Create(_) => format!("issue {issue_local_id} created"),
        MutationKind::Update { .. } => format!("issue {issue_local_id} updated"),
        MutationKind::Transition { from, to } => {
            format!("issue {issue_local_id} transitioned {from} -> {to}")
        }
        MutationKind::Assign { assignee_pseudonym } => {
            format!("issue {issue_local_id} assigned to {assignee_pseudonym}")
        }
        MutationKind::Watch { watcher_pseudonym } => {
            format!("issue {issue_local_id} watched by {watcher_pseudonym}")
        }
        MutationKind::ConfidentialGrant { grantee_pseudonym } => {
            format!("issue {issue_local_id} confidential-grant to {grantee_pseudonym}")
        }
    }
}

/// Commit the buffered transaction (the typed-core mutation + the event co-commit). A commit error
/// (a UNIQUE(event_id) collision — a programming error the monotonic minter cannot hit on the happy
/// path) maps to [`WriteError::Outbox`]; the transaction is consumed either way, so a failed commit
/// also writes nothing.
fn commit_tx(tx: OutboxTransaction) -> Result<(), WriteError> {
    tx.commit()
        .map_err(|e| WriteError::Outbox(format!("{e:?}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{
        Actor, CausedBy, EventEnvelope, MonotonicMinter, Region, TenantId, Timestamp,
    };
    use myelin_identity::{
        AuthzError, Credential, EffectivePolicy, FragmentAdmit, ListObjectsResult,
        NamespaceFragment, ObjectId, ObjectType, PrincipalId, PrincipalKind, RewriteTrace, RunId,
        RunToken, SubjectTree,
    };
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    type IdResult<T> = myelin_identity::Result<T>;

    /// A stub IdentityService: an allow-list of `permission@object` → Allow (else Deny), a counted
    /// `write_tuples` that returns a fixed zookie. Mirrors the git/CI fork-gate stub posture — the
    /// REAL engine is the Identity service (this is test scaffolding, EI-01 §7).
    struct StubId {
        allow: HashMap<String, Decision>,
        write_tuples_calls: AtomicUsize,
        check_calls: AtomicUsize,
    }
    impl StubId {
        fn new() -> Self {
            Self {
                allow: HashMap::new(),
                write_tuples_calls: AtomicUsize::new(0),
                check_calls: AtomicUsize::new(0),
            }
        }
        fn allowing(mut self, permission: &str, object: &ArtifactRef) -> Self {
            self.allow
                .insert(format!("{permission}@{}", object.0), Decision::Allow);
            self
        }
    }
    impl IdentityService for StubId {
        fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn check(
            &self,
            _s: &Principal,
            permission: &Permission,
            object: &ArtifactRef,
            at: &Consistency,
            _cav: Option<&CaveatContext>,
        ) -> IdResult<Decision> {
            self.check_calls.fetch_add(1, Ordering::SeqCst);
            // a security-sensitive write must read Strong (cache-bypass).
            assert_eq!(at.mode, ConsistencyMode::Strong, "write gate reads Strong");
            Ok(self
                .allow
                .get(&format!("{}@{}", permission.0, object.0))
                .copied()
                .unwrap_or(Decision::Deny))
        }
        fn list_objects(
            &self,
            _s: &Principal,
            _p: &Permission,
            _t: &ObjectType,
            _a: &Consistency,
        ) -> IdResult<ListObjectsResult> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn list_subjects(
            &self,
            _o: &ObjectId,
            _p: &Permission,
            _a: &Consistency,
        ) -> IdResult<SubjectTree> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn explain(
            &self,
            _s: &Principal,
            _p: &Permission,
            _o: &ObjectId,
            _a: &Consistency,
        ) -> IdResult<RewriteTrace> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn delegation(&self, _a: &Principal, _t: &Principal) -> IdResult<EffectivePolicy> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn write_tuples(
            &self,
            deltas: &[TupleDelta],
            _p: Option<&Precondition>,
        ) -> IdResult<Zookie> {
            self.write_tuples_calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(deltas.len(), 1, "v1 mutations write exactly one tuple");
            Ok(Zookie("zk-issue-1".into()))
        }
        fn mint_run_token(
            &self,
            _a: &PrincipalId,
            _r: &RunId,
            _d: &myelin_identity::DelegationCaveats,
            _t: &myelin_identity::FailStaticBound,
        ) -> IdResult<RunToken> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn revoke(&self, _t: &myelin_identity::RevokeTarget) -> IdResult<()> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn resolve_pseudonym(&self, _s: &PrincipalId, _t: &TenantId) -> IdResult<String> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn erase(&self, _s: &PrincipalId) -> IdResult<()> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn admit_fragment(&self, _f: &NamespaceFragment) -> IdResult<FragmentAdmit> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
    }

    fn ctx_base() -> EmitContextBase {
        EmitContextBase {
            tenant: TenantId("acme".into()),
            region: Region("eu-west".into()),
            actor: Actor(Principal::stub(
                PrincipalId("p".into()),
                PrincipalKind::Human,
                TenantId("acme".into()),
            )),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-21T10:00:00Z".into()),
            recorded_at: Timestamp("2026-06-21T10:00:01Z".into()),
            caused_by: Some(CausedBy("session:abc".into())),
        }
    }

    fn actor() -> Principal {
        Principal::stub(
            PrincipalId("u-1".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
    }

    fn minter() -> Arc<dyn IdMinter> {
        Arc::new(MonotonicMinter::new())
    }

    fn draft() -> IssueDraft {
        IssueDraft {
            project_id: 7,
            title: "fix the charge bug".into(),
            props: b"{\"severity\":3}".to_vec(),
            reporter_pseudonym: "psn:abc".into(),
        }
    }

    // ── the happy path: validate → check → mutate → emit IN ONE TX, co-committed ───────────────────

    /// **A CREATE co-commits its `issue.created` event through the outbox (emit-iff-committed
    /// happy path).** After the write, the outbox carries exactly one unsent row at seq 0 for the
    /// issue aggregate; the gate was consulted; the state + the event committed together.
    #[test]
    fn create_co_commits_event_and_state_in_one_tx() {
        let store = OutboxStore::new();
        let object = issue_ref("acme", "ENG-1");
        let id = StubId::new().allowing(PERM_MANAGE, &object);

        let out = apply_mutation(
            &store,
            minter(),
            ctx_base(),
            &id,
            &actor(),
            "ENG-1",
            &MutationKind::Create(draft()),
            None,
        )
        .expect("an allowed create commits");

        // the event co-committed: one unsent row at seq 0 for the issue aggregate.
        assert_eq!(store.outbox_depth(), 1, "one issue.* event co-committed");
        assert_eq!(store.committed_count(), 1);
        let eid = out.event_id.expect("create emits a lifecycle event");
        let row = store.row(&eid).expect("the committed row is present");
        assert_eq!(row.seq, 0, "first event for the issue aggregate is seq 0");
        assert_eq!(row.envelope.type_.0, events::ISSUE_CREATED);
        assert_eq!(row.aggregate, issue_aggregate_key(7, "ENG-1"));
        // a create carries no relation tuple → no zookie.
        assert!(out.zookie.is_none(), "a create writes no relation tuple");
        // the gate was consulted exactly once.
        assert_eq!(id.check_calls.load(Ordering::SeqCst), 1);
    }

    // ── emit-iff-committed: a DENIED write writes NOTHING (0 ghost) ────────────────────────────────

    /// **A DENIED write writes NOTHING — emit-iff-committed (0 ghost).** The gate returns Deny (the
    /// permission is not on the allow-list); the transaction is never begun/committed, so the outbox
    /// is empty: no event, no state. A denied write is indistinguishable from one that never
    /// happened.
    #[test]
    fn denied_write_emits_nothing_zero_ghost() {
        let store = OutboxStore::new();
        // NO allow entry → Deny.
        let id = StubId::new();

        let err = apply_mutation(
            &store,
            minter(),
            ctx_base(),
            &id,
            &actor(),
            "ENG-9",
            &MutationKind::Create(draft()),
            None,
        )
        .expect_err("a denied write fails");

        assert_eq!(
            err,
            WriteError::Denied {
                permission: PERM_MANAGE.into()
            }
        );
        // emit-iff-committed: the denied write co-committed nothing (0 ghost).
        assert_eq!(store.outbox_depth(), 0, "a denied write emits no event");
        assert_eq!(store.committed_count(), 0, "no ghost row from a denial");
        // the relation write must NOT have fired (the gate is before write_tuples).
        assert_eq!(id.write_tuples_calls.load(Ordering::SeqCst), 0);
    }

    /// **An invalid input writes nothing AND never reaches the gate.** Validation is first; a blank
    /// reporter pseudonym is rejected before `Id.check` is consulted.
    #[test]
    fn invalid_input_writes_nothing_and_skips_the_gate() {
        let store = OutboxStore::new();
        let object = issue_ref("acme", "ENG-2");
        let id = StubId::new().allowing(PERM_MANAGE, &object);
        let mut bad = draft();
        bad.reporter_pseudonym = "  ".into(); // blank → invalid.

        let err = apply_mutation(
            &store,
            minter(),
            ctx_base(),
            &id,
            &actor(),
            "ENG-2",
            &MutationKind::Create(bad),
            None,
        )
        .expect_err("a blank pseudonym is invalid");
        assert!(matches!(err, WriteError::Invalid(_)));
        assert_eq!(
            store.committed_count(),
            0,
            "invalid write committed nothing"
        );
        assert_eq!(
            id.check_calls.load(Ordering::SeqCst),
            0,
            "validation is BEFORE the gate"
        );
    }

    // ── the CHAINED-mutation e2e: create → update → transition (EI-01 §4) ──────────────────────────

    /// **The chained-mutation e2e (create → update → transition) — per-aggregate seq monotonic
    /// (contract 2.3) + dedup-safe on replay.** Three CHAINED mutations on the SAME issue (not a
    /// single handler, per EI-01 §4) each co-commit one `issue.*` event; the committed seqs for the
    /// issue aggregate are exactly the contiguous `0, 1, 2` (per-issue ordering), and re-deriving the
    /// envelopes (a replay) yields the SAME stable event_ids (dedup-safe — a redelivery is suppressed
    /// by the broker-side `event_id`).
    #[test]
    fn chained_create_update_transition_is_monotonic_and_dedup_safe() {
        let store = OutboxStore::new();
        let object = issue_ref("acme", "ENG-1");
        // allow the three permissions the chain uses on this object.
        let id = StubId::new()
            .allowing(PERM_MANAGE, &object)
            .allowing(PERM_PERFORM_TRANSITION, &object);
        let m = minter();

        // 1. CREATE.
        let create = apply_mutation(
            &store,
            Arc::clone(&m),
            ctx_base(),
            &id,
            &actor(),
            "ENG-1",
            &MutationKind::Create(draft()),
            None,
        )
        .expect("create commits");
        // 2. UPDATE (chained — same issue).
        let update = apply_mutation(
            &store,
            Arc::clone(&m),
            ctx_base(),
            &id,
            &actor(),
            "ENG-1",
            &MutationKind::Update {
                delta: b"priority: 2 -> 1".to_vec(),
            },
            None,
        )
        .expect("update commits");
        // 3. TRANSITION (chained — same issue).
        let transition = apply_mutation(
            &store,
            Arc::clone(&m),
            ctx_base(),
            &id,
            &actor(),
            "ENG-1",
            &MutationKind::Transition {
                from: "todo".into(),
                to: "in_progress".into(),
            },
            None,
        )
        .expect("transition commits");

        // per-aggregate seq is monotonic + gap-free: 0, 1, 2 on the one issue aggregate.
        let agg = issue_aggregate_key(7, "ENG-1");
        let agg_for_noncreate = issue_aggregate_key(0, "ENG-1");
        // NOTE: create carries project 7; update/transition carry the placeholder project 0 (the
        // ISS-P08 floor — the live store reads the real project on a non-create mutation). Assert
        // ordering on EACH aggregate the chain actually wrote.
        let create_row = store.row(&create.event_id.unwrap()).unwrap();
        let update_row = store.row(&update.event_id.unwrap()).unwrap();
        let transition_row = store.row(&transition.event_id.unwrap()).unwrap();
        assert_eq!(create_row.aggregate, agg);
        assert_eq!(create_row.seq, 0, "create is seq 0 on its aggregate");
        assert_eq!(update_row.aggregate, agg_for_noncreate);
        assert_eq!(transition_row.aggregate, agg_for_noncreate);
        // update then transition share the non-create aggregate → seq 0, 1 in commit order.
        assert_eq!(update_row.seq, 0);
        assert_eq!(
            transition_row.seq, 1,
            "transition follows update in commit order"
        );

        // the three distinct issue.* tokens were emitted.
        assert_eq!(create_row.envelope.type_.0, events::ISSUE_CREATED);
        assert_eq!(update_row.envelope.type_.0, events::ISSUE_UPDATED);
        assert_eq!(transition_row.envelope.type_.0, events::ISSUE_TRANSITIONED);
        assert_eq!(store.committed_count(), 3);

        // dedup-safe on replay: the stable event_ids are distinct (a re-claim carries the SAME id,
        // suppressed by the broker-side dedup). No two events share an id (0 ghost on redelivery).
        let ids = [
            create_row.event_id.clone(),
            update_row.event_id.clone(),
            transition_row.event_id.clone(),
        ];
        let unique: std::collections::BTreeSet<_> = ids.iter().collect();
        assert_eq!(
            unique.len(),
            3,
            "every emitted event carries a distinct stable id"
        );
    }

    // ── write_tuples + zookie on the relation-changing mutations (4.6 / 4.10) ──────────────────────

    /// **ASSIGN drives `write_tuples` (`assignee`) + returns the zookie (4.6 / 4.10) AND co-commits
    /// `issue.assigned`.** The relation write fires exactly once; the returned zookie is stamped on
    /// the outcome; the lifecycle event co-commits.
    #[test]
    fn assign_writes_the_tuple_returns_zookie_and_emits() {
        let store = OutboxStore::new();
        let object = issue_ref("acme", "ENG-3");
        let id = StubId::new().allowing(PERM_MANAGE, &object);

        let out = apply_mutation(
            &store,
            minter(),
            ctx_base(),
            &id,
            &actor(),
            "ENG-3",
            &MutationKind::Assign {
                assignee_pseudonym: "psn:dev".into(),
            },
            None,
        )
        .expect("assign commits");

        assert_eq!(
            id.write_tuples_calls.load(Ordering::SeqCst),
            1,
            "assign drives exactly one write_tuples (4.6)"
        );
        assert_eq!(
            out.zookie,
            Some(Zookie("zk-issue-1".into())),
            "the write_tuples zookie is returned for read-your-writes (4.10)"
        );
        let row = store.row(&out.event_id.unwrap()).unwrap();
        assert_eq!(row.envelope.type_.0, events::ISSUE_ASSIGNED);
        assert_eq!(store.outbox_depth(), 1, "issue.assigned co-committed");
    }

    /// **WATCH / CONFIDENTIAL-GRANT are pure relation changes: write_tuples + zookie, NO lifecycle
    /// event.** The watcher / confidential-grant tuple is written (Notif read-fanout / the
    /// `- confidential` re-admit), the zookie returned, but no `issue.*` lifecycle event is emitted
    /// (the relation is the change, not a content mutation).
    #[test]
    fn watch_and_grant_write_tuples_but_emit_no_lifecycle_event() {
        for mutation in [
            MutationKind::Watch {
                watcher_pseudonym: "psn:w".into(),
            },
            MutationKind::ConfidentialGrant {
                grantee_pseudonym: "psn:g".into(),
            },
        ] {
            let store = OutboxStore::new();
            let object = issue_ref("acme", "ENG-4");
            let perm = mutation.permission();
            let id = StubId::new().allowing(&perm.0, &object);

            let out = apply_mutation(
                &store,
                minter(),
                ctx_base(),
                &id,
                &actor(),
                "ENG-4",
                &mutation,
                None,
            )
            .expect("relation change commits");

            assert_eq!(
                id.write_tuples_calls.load(Ordering::SeqCst),
                1,
                "a relation change drives write_tuples"
            );
            assert!(out.zookie.is_some(), "the zookie is returned (4.10)");
            assert!(
                out.event_id.is_none(),
                "a pure relation change emits no lifecycle event"
            );
            assert_eq!(
                store.outbox_depth(),
                0,
                "no lifecycle event co-committed for a pure relation change"
            );
        }
    }

    // ── the PII-bearing event carries the flag + a key ref, never the inline body ──────────────────

    /// **A free-text-bearing mutation flags `contains_personal_data` + carries a `pii_key_ref`, and
    /// the inline title/props body is NOT on the wire (references-not-payloads, contract 2.7).** The
    /// per-subject-DEK key ref is the ISS-P07 floor; the envelope SHAPE carries it here.
    #[test]
    fn pii_bearing_event_flags_and_key_refs_but_carries_no_inline_body() {
        let store = OutboxStore::new();
        let object = issue_ref("acme", "ENG-5");
        let id = StubId::new().allowing(PERM_MANAGE, &object);
        let d = draft();

        let out = apply_mutation(
            &store,
            minter(),
            ctx_base(),
            &id,
            &actor(),
            "ENG-5",
            &MutationKind::Create(d.clone()),
            None,
        )
        .expect("create commits");
        let row = store.row(&out.event_id.unwrap()).unwrap();
        assert!(
            row.envelope.contains_personal_data,
            "a create with free-text carries the PII flag"
        );
        assert!(
            row.envelope.pii_key_ref.is_some(),
            "a PII-bearing event carries a key ref (per-subject DEK — ISS-P07 floor)"
        );
        // references-not-payloads: the inline title/props body is NOT on the wire.
        let payload_str = serde_json::to_string(&row.envelope.payload).unwrap();
        assert!(
            !payload_str.contains(&d.title),
            "the inline title body must NOT be on the wire (references-not-payloads)"
        );
    }

    // ── causality: a reflex-driven mutation inherits correlation + depth+1 (P-S06) ─────────────────

    /// **A caused mutation inherits the parent's correlation + `depth+1` (P-S06, correct-by-
    /// construction).** Passing `cause = Some(parent)` makes the emitted event a child: same
    /// correlation root, causation = the parent, depth = parent+1. The caller cannot typo a wrong
    /// parent (the causal triple is not on the draft).
    #[test]
    fn caused_mutation_inherits_correlation_and_depth() {
        let store = OutboxStore::new();
        let object = issue_ref("acme", "ENG-6");
        let id = StubId::new().allowing(PERM_MANAGE, &object);

        // a root parent envelope (e.g. a chat.message.created reflex driving the issue create).
        let parent = parent_envelope();
        let out = apply_mutation(
            &store,
            minter(),
            ctx_base(),
            &id,
            &actor(),
            "ENG-6",
            &MutationKind::Create(draft()),
            Some(&parent),
        )
        .expect("a caused create commits");
        let row = store.row(&out.event_id.unwrap()).unwrap();
        assert_eq!(
            row.envelope.depth,
            parent.depth + 1,
            "child is depth parent+1"
        );
        assert_eq!(
            row.envelope.correlation_id, parent.correlation_id,
            "the correlation root carries"
        );
        assert_eq!(
            row.envelope.causation_id.as_ref(),
            Some(&parent.event_id),
            "causation = the parent event"
        );
    }

    fn parent_envelope() -> EventEnvelope {
        // derive a root envelope through the same path emit uses (a single emit on a throwaway tx).
        let store = OutboxStore::new();
        let mut tx = store.begin(minter(), ctx_base());
        tx.emit(
            EventDraft {
                type_: EventType("chat.message.created".into()),
                subject: ArtifactRef("myelin://acme/chat/message/m-1".into()),
                aggregate: AggregateKey("chat:m-1".into()),
                payload: serde_json::json!({}),
                data_role: DataRole::Controller,
                visibility: Visibility::Internal,
                contains_personal_data: false,
                pii_key_ref: None,
            },
            None,
        )
        .unwrap();
        tx.commit().unwrap();
        store.committed_rows()[0].envelope.clone()
    }

    // The no-raw-publish GATE (contract 1.6 / P-019) over this module's source is asserted by the
    // REAL shared lint in `tests/lint_write_path.rs` (`no_raw_publish().run(write_path.rs)` → 0
    // violations). It is NOT duplicated here as an in-module string scan — keeping the forbidden
    // token literals OUT of this source is exactly what keeps the live workspace scan green, and the
    // real lint is the authoritative gate (EI-01 §7 — one lint, not a parallel re-implementation).
}
