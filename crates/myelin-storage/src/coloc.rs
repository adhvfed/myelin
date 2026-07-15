//! # Outbox co-location in the OLTP database + the in-same-transaction co-commit
//!
//! **Prompt:** P-ST-02 → global **P-016** (M0). **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/storage.md` §3.1 (Tier 1 OLTP —
//! "**the outbox lives here** (same transaction as the state change — the anchor of the
//! cross-seam consistency point, §7.3)"), §7.3 (the cross-seam consistency point = the
//! per-aggregate outbox `seq` / event-log offset: "the outbox row is written in the same
//! OLTP tx as the state change, **so OLTP commit order == event order**").
//! **Contract-index:** row 11.1 (the outbox-co-location half — completing P-ST-01's
//! pool/RLS half); consumed/wired 2.2 (`OutboxTx::emit`) + 2.3 (the `outbox` table).
//!
//! ## What this module ADDS (and what it deliberately reuses — coherence, EI-01 §7)
//! The `outbox` **table** (the frozen 2.3 DDL [`myelin_events::OUTBOX_MIGRATION`]), the
//! `OutboxTx::emit` surface, the same-transaction buffer→commit machinery, the per-aggregate
//! commit-order `seq`, and the relay all already exist in `myelin-events` (shipped by
//! P-008/P-012/P-013 from the substrate + event-bus roadmaps). The global run order
//! interleaves three roadmaps, so the outbox surface is reached from all of them. Per the
//! coherence rule (EI-01 §7: never define a type twice, never build a parallel second
//! implementation) this prompt does **NOT** re-define the outbox table, the emit trait, or
//! the seq allocator — it **reuses** them in place.
//!
//! ## `residency-pin` lint — NAMED M0 FLOOR (`@residency-cell-pinned:file`)
//! This module opens the co-located OLTP pool ([`ColocatedOltp::open`] → [`crate::oltp::OltpPool`]).
//! The `residency-pin` lint (P-S11 → P-018, SHARPENED to the real OLTP constructors in
//! P-ST-04 → P-020) requires a `Region` on every store construction. Like [`crate::oltp`] this is
//! the **M0 region-less pool MODEL** — the cell's region pins data via the per-query
//! `(tenant, region)` `TenantScope`; the per-POOL runtime region-pin lands end-to-end in
//! **P-ST-15 / P-102** (STOR-D5). The file-level waiver marker `@residency-cell-pinned:file` records
//! this floor LOUDLY (EI-01 §4 — named, never a silent skip).
//!
//! What P-ST-02 is genuinely about — and what is **new here** — is the **co-location**: the
//! outbox table living in the **same OLTP service database** as the state change, so a single
//! OLTP transaction carries *both* the domain state write *and* the outbox insert. That is the
//! Storage-side binding (P-S07/EB-03 built the outbox *mechanism*; this prompt *co-locates* it
//! in the OLTP tier this crate owns):
//! - [`ColocatedOltp`] — the OLTP store that **owns** the outbox: opening it declares the
//!   outbox table as part of *this service's* migration set ([`ColocatedOltp::migrations`]
//!   contains [`OUTBOX_MIGRATION`]), proving the table is co-located in the service DB and not
//!   in some separate "events database" (a separate DB would make the cross-seam cursor a
//!   distributed-transaction problem — the thing co-location exists to avoid, §7.3).
//! - [`ColocatedTx`] — the **one** transaction that holds an acquired OLTP pool permit AND the
//!   open [`OutboxTransaction`] together. The caller stages domain-state writes and emits
//!   events on the *same* handle; [`ColocatedTx::commit`] makes **both** durable atomically,
//!   and dropping it without commit (an abort / a crash between the state write and the
//!   publish) makes **neither** durable. This is the same-transaction co-commit the prompt
//!   names: *the state write and the outbox insert share one transaction and either both
//!   commit or both roll back.*
//!
//! ## The load-bearing invariant this module establishes (§7.3, the cross-seam cursor)
//! Because the outbox row commits in the **same** OLTP transaction as the state change, **OLTP
//! commit order == event order**, and the per-aggregate outbox `seq` *is* the §7.3 cross-seam
//! linearisation cursor. That is what makes restore-to-a-consistent-point possible: PITR the
//! OLTP DB to the WAL position whose outbox rows have `seq ≤ T`, and every restored state row
//! has its event and vice-versa (no torn cross-seam state). Co-location is the precondition;
//! without it OLTP commit order and event order could diverge and the cursor would not exist.
//!
//! ## FLOOR named (the forward dependency the prompt requires recorded in writing)
//! The per-aggregate `seq` established here is the **restore-to-consistent-point cursor used by
//! P-ST-14** (`restore(to_offset T)` / post-restore re-erasure, global P-100): restore replays
//! to the WAL position whose outbox `seq ≤ T`, relying on the "OLTP commit order == event
//! order" property co-location gives. Named here per the DEFINITION OF DONE; P-ST-14 consumes
//! it.
//!
//! ## DEVIATION / FLOOR — the modeled OLTP transaction (EI-01 §1, write it down)
//! There is **no live Postgres in M0** (the concrete `serve(AppSpec)` pool body + the migration
//! *runner* are the deferred floors P-S12/P-S15; see [`crate::oltp`]). So the *mechanism* this
//! prompt owns — *one transaction co-commits state + outbox, or rolls back both* — is modeled
//! exactly:
//! - the OLTP transaction is modeled by holding a bounded-pool [`PermitGuard`](crate::oltp) for
//!   the tenant (the real `BEGIN … COMMIT` runs under one connection from this pool) plus the
//!   in-memory [`OutboxTransaction`] (whose semantics ARE the SQL `INSERT … RETURNING` inside
//!   the caller's tx);
//! - the staged domain-state writes are recorded so a test can assert state + event commit
//!   together and that an abort writes **neither** (the same modeled-state discipline
//!   `myelin-events` uses);
//! - **commit is all-or-nothing**: if the staged outbox commit fails (e.g. a `UNIQUE` violation),
//!   the staged state is **discarded too** — both roll back. An *injected mid-tx failure*
//!   ([`ColocatedTx::commit_with_state_fault`]) models the state write failing after events were
//!   buffered: commit returns an error and **nothing** is written (the outbox stays empty),
//!   proving the co-commit really is atomic in both directions.
//!
//! When the real driver lands (P-S12), [`ColocatedTx::commit`] becomes the real `COMMIT` of the
//! one connection that did the state `INSERT/UPDATE` and the outbox `INSERT`; the seam shape (one
//! handle stages both, commit is atomic, abort writes neither) does **not** change.

use std::sync::Arc;

use myelin_events::{
    EmitContextBase, EventDraft, EventEnvelope, EventId, IdMinter, OutboxStore, OutboxTransaction,
    OutboxTx, OUTBOX_MIGRATION,
};

use crate::oltp::{OltpConfig, OltpError, OltpPool, PermitGuard};

/// An error from the co-located OLTP store / its transactions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ColocError {
    /// The underlying OLTP pool rejected (bad config / saturation / per-tenant cap). The
    /// co-commit inherits the bounded-pool fast-fail — a transaction cannot even begin if no
    /// connection is available (it does not block unboundedly).
    Pool(OltpError),
    /// The atomic commit failed (e.g. the staged state write failed mid-transaction, or an
    /// outbox `UNIQUE` constraint was violated). **Nothing was written** — both the state and
    /// the events rolled back. Carries the precise reason.
    CommitRolledBack(String),
}

impl core::fmt::Display for ColocError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ColocError::Pool(e) => write!(f, "co-located OLTP pool error: {e}"),
            ColocError::CommitRolledBack(why) => write!(
                f,
                "co-located transaction rolled back — neither state nor outbox committed: {why}"
            ),
        }
    }
}

impl std::error::Error for ColocError {}

impl From<OltpError> for ColocError {
    fn from(e: OltpError) -> Self {
        ColocError::Pool(e)
    }
}

/// The OLTP store that **co-locates its outbox** (contract 11.1 outbox half + 2.3 table). It
/// owns a bounded OLTP [`OltpPool`] (P-ST-01) AND the service's [`OutboxStore`] (the outbox
/// table living in this same DB). A subsystem opens ONE of these for its service database; the
/// outbox table is part of *this* DB's migration set, so an outbox row commits in the same OLTP
/// transaction as the domain state change (§3.1 / §7.3 — OLTP commit order == event order).
///
/// `Clone` shares the same underlying pool counters and the same outbox rows (both are
/// `Arc`-backed), so the relay, the depth reader, and the emitting handlers all observe one
/// truth — exactly as they would against one Postgres database.
#[derive(Clone)]
pub struct ColocatedOltp {
    pool: OltpPool,
    outbox: OutboxStore,
    minter: Arc<dyn IdMinter>,
}

impl ColocatedOltp {
    /// Open the co-located OLTP store: a bounded pool (validated config) + the INJECTED outbox
    /// that lives in the same DB (MR-009b W3b.4 — the composition root owns durability: production
    /// passes `OutboxStore::durable(PgOutboxBacking)`; a test passes the in-memory
    /// `OutboxStore::new()` double). `minter` supplies the stable ULID for emitted events — a
    /// production root wires the UNIQUE `myelin_events::UlidMinter` (the P-S12 stand-in), NEVER
    /// the per-instance-resetting default `MonotonicMinter` (the W3b.3 named condition: colliding
    /// `event_id`s are silently dropped by the durable `ON CONFLICT (event_id) DO NOTHING`).
    ///
    /// Fast-fails on a bad pool config (never starts with an unbounded pool — the §3.1 bound).
    pub fn open(
        config: OltpConfig,
        outbox: OutboxStore,
        minter: Arc<dyn IdMinter>,
    ) -> Result<ColocatedOltp, ColocError> {
        let pool = OltpPool::open(config)?;
        Ok(ColocatedOltp {
            pool,
            outbox,
            minter,
        })
    }

    /// The forward-only migration set for THIS service database — and it **contains the outbox
    /// table DDL** ([`OUTBOX_MIGRATION`]). This is the structural proof of co-location: the
    /// outbox table is migrated into the *same* service DB as the domain tables (not a separate
    /// "events DB"), so a single transaction can carry a state write and an outbox insert. The
    /// migration runner (P-S15) applies these; here the set is the declared shape.
    ///
    /// `service_tables` are the subsystem's own domain-table DDL statements; the outbox DDL is
    /// appended so it always co-locates. A test asserts the outbox migration is present.
    pub fn migrations(service_tables: &[&'static str]) -> Vec<&'static str> {
        let mut set: Vec<&'static str> = service_tables.to_vec();
        set.push(OUTBOX_MIGRATION);
        set
    }

    /// Begin a **co-located transaction** for `ctx_base`'s tenant: acquire one OLTP pool permit
    /// (the bounded connection the real `BEGIN … COMMIT` runs under) and open the outbox
    /// transaction on the same logical transaction. The returned [`ColocatedTx`] stages domain
    /// state AND emits events on the same handle; commit is atomic across both.
    ///
    /// Returns [`ColocError::Pool`] immediately (fast-fail, never blocks) if no connection is
    /// available — a saturated pool cannot even begin a transaction.
    pub fn begin(&self, ctx_base: EmitContextBase) -> Result<ColocatedTx, ColocError> {
        // Acquire the bounded OLTP permit for this tenant FIRST (the connection the whole
        // transaction runs on). The permit is held for the life of the transaction and released
        // on drop — committed or aborted — so a connection is never leaked.
        let permit = self.pool.acquire(&ctx_base.tenant)?;
        let outbox_tx = self.outbox.begin(Arc::clone(&self.minter), ctx_base);
        Ok(ColocatedTx {
            _permit: permit,
            outbox_tx,
            staged_state: Vec::new(),
        })
    }

    /// The shared outbox handle (for the relay to drain + the depth signal to read). The relay
    /// (`myelin_events::Relay`) drains THIS store — same outbox, one truth.
    pub fn outbox(&self) -> &OutboxStore {
        &self.outbox
    }

    /// The number of **unsent** outbox rows (`published_at IS NULL`) — the `outbox_depth`
    /// survival signal (contract 1.8). The SUB-D1 drill asserts this `→ 0` after the relay
    /// drains; co-location does not change the signal, it makes the rows co-commit.
    pub fn outbox_depth(&self) -> usize {
        self.outbox.outbox_depth()
    }

    /// The bounded pool this store opened with (for the cap/saturation signals).
    pub fn pool(&self) -> &OltpPool {
        &self.pool
    }
}

/// One **co-located transaction** — the same-transaction co-commit handle. It holds:
/// - an OLTP pool [`PermitGuard`] (the bounded connection the real `BEGIN … COMMIT` runs on),
///   released on drop whether the transaction commits or aborts (no leaked connection);
/// - the open [`OutboxTransaction`] (the same-tx outbox buffer).
///
/// The caller [`stage_state`](Self::stage_state)s its domain writes and [`emit`](OutboxTx::emit)s
/// events on the **same** handle. [`commit`](Self::commit) makes BOTH durable atomically;
/// dropping without commit makes NEITHER durable (emit-iff-committed in both directions — there
/// is no committed state without its event, and no event without its committed state).
pub struct ColocatedTx {
    /// The OLTP connection permit, held for the whole transaction (released on drop). The
    /// underscore marks it as held-for-RAII, not read.
    _permit: PermitGuard,
    /// The same-tx outbox buffer (events emitted here become durable iff this tx commits).
    outbox_tx: OutboxTransaction,
    /// The caller's staged domain-state writes (modeled; in a real service these are the rows
    /// the handler `INSERT/UPDATE`s into its own tables on the same connection). Recorded so a
    /// test can assert state + events commit together — and that an abort writes neither.
    staged_state: Vec<String>,
}

impl ColocatedTx {
    /// Stage a domain-state write into THIS transaction (the "state change" half of the
    /// co-commit). In a real service this is the row the handler writes to its own table on the
    /// same OLTP connection the outbox insert uses; here it is recorded so the co-commit is
    /// assertable. Returns `&mut self` for chaining alongside `emit`.
    pub fn stage_state(&mut self, change: impl Into<String>) -> &mut Self {
        self.staged_state.push(change.into());
        // Mirror it onto the outbox transaction's modeled state slot too, so the events crate's
        // own co-commit invariant sees the staged state (one transaction, one truth).
        self.outbox_tx
            .stage_state_change(change_label(&self.staged_state));
        self
    }

    /// Emit an event on THIS transaction (delegates to the frozen [`OutboxTx::emit`], 2.2). The
    /// derived [`EventEnvelope`] is buffered into the same transaction as the staged state; it
    /// becomes durable iff [`commit`](Self::commit) succeeds.
    pub fn emit(
        &mut self,
        draft: EventDraft,
        cause: Option<&EventEnvelope>,
    ) -> Result<EventId, ColocError> {
        self.outbox_tx
            .emit(draft, cause)
            .map_err(|e| ColocError::CommitRolledBack(e.0))
    }

    /// The number of events buffered (emitted-but-not-yet-committed) on this transaction.
    pub fn staged_event_count(&self) -> usize {
        self.outbox_tx.staged_len()
    }

    /// The staged domain-state writes (for the co-commit assertion in tests).
    pub fn staged_state(&self) -> &[String] {
        &self.staged_state
    }

    /// **Commit the co-located transaction: the staged domain state + every buffered outbox row
    /// become durable atomically.** This is the same-transaction co-commit the prompt requires.
    ///
    /// In the modeled floor the only thing that can fail at commit is the outbox commit itself
    /// (a `UNIQUE(event_id)` violation — a programming error, never on the happy path). If it
    /// fails, the staged STATE is discarded too (it never reaches durable storage) — both roll
    /// back. The OLTP permit is released on drop either way.
    ///
    /// In the real OLTP binding (P-S12) this is the single `COMMIT` of the one connection that
    /// did the state `INSERT/UPDATE` and the outbox `INSERT … RETURNING` — Postgres makes it
    /// atomic; the seam does not change.
    pub fn commit(self) -> Result<(), ColocError> {
        // Hand the buffered events to the outbox's atomic commit. If it errors, NOTHING (state
        // or events) is written — the staged state is dropped with `self`, the outbox untouched.
        self.outbox_tx
            .commit()
            .map_err(|e| ColocError::CommitRolledBack(e.0))
        // `self` (incl. the staged state + the permit) drops here on success too. On the happy
        // path the state was "written" together with the outbox rows (modeled); in the real
        // binding the state INSERT/UPDATE is part of the same COMMIT.
    }

    /// **Inject a mid-transaction failure on the STATE write** (the drill lever): model the
    /// caller's domain-state `INSERT/UPDATE` failing *after* events were buffered. The whole
    /// co-located transaction must roll back — **neither** the state **nor** the buffered events
    /// become durable. Returns [`ColocError::CommitRolledBack`] and writes nothing to the outbox.
    ///
    /// This proves the co-commit is atomic in BOTH directions: not just "no event without
    /// committed state" (the structural drop-the-tx case) but also "no committed state without
    /// its event" — here the state write fails so neither side commits. The unit test
    /// `both_roll_back_under_injected_mid_tx_failure` asserts the outbox stays empty.
    pub fn commit_with_state_fault(self, reason: &str) -> Result<(), ColocError> {
        // The state write failed mid-tx → abort. The buffered outbox events are dropped with
        // `self` WITHOUT reaching `outbox_tx.commit()`, so the outbox store is untouched. The
        // permit is released on drop. Both sides rolled back.
        Err(ColocError::CommitRolledBack(format!(
            "injected state-write failure: {reason}"
        )))
    }
}

/// The label the outbox's modeled `state_committed` slot carries — the joined staged-state
/// changes, so the events crate's own co-commit invariant observes the same staged state.
fn change_label(staged: &[String]) -> String {
    staged.join("; ")
}

/// Re-export the frozen outbox table DDL so a reader of this module sees the co-located shape
/// without chasing it into `myelin-events` (it is the SAME constant, not a copy — co-location
/// reuses the one table definition, never re-declares it).
pub use myelin_events::OUTBOX_MIGRATION as COLOCATED_OUTBOX_MIGRATION;

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{Actor, CausedBy, Region, TenantId, Timestamp};
    use myelin_events::{
        AggregateKey, ArtifactRef, DataRole, EventDraft, EventType, MonotonicMinter, Visibility,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn cfg() -> OltpConfig {
        OltpConfig {
            max_pool_size: 8,
            statement_timeout_ms: 3_000,
            per_tenant_in_flight_cap: 4,
        }
    }

    fn store() -> ColocatedOltp {
        ColocatedOltp::open(
            cfg(),
            OutboxStore::new(),
            Arc::new(MonotonicMinter::new()) as Arc<dyn IdMinter>,
        )
        .expect("a valid config opens the co-located OLTP store")
    }

    fn ctx_base(tenant: &str) -> EmitContextBase {
        EmitContextBase {
            tenant: TenantId(tenant.into()),
            region: Region("eu-west".into()),
            actor: Actor(Principal::stub(
                PrincipalId("p".into()),
                PrincipalKind::Human,
                TenantId(tenant.into()),
            )),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-19T00:00:01Z".into()),
            caused_by: Some(CausedBy("session:abc".into())),
        }
    }

    fn draft(type_: &str, aggregate: &str) -> EventDraft {
        EventDraft {
            type_: EventType(type_.into()),
            subject: ArtifactRef(format!("myelin://acme/issues/issue/{aggregate}")),
            aggregate: AggregateKey(aggregate.into()),
            payload: serde_json::json!({ "ref": aggregate }),
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        }
    }

    /// **Co-location is structural: the outbox table DDL is part of the service DB's migration
    /// set.** A subsystem's migration set (its own domain tables + the outbox) carries the
    /// frozen 2.3 outbox DDL — proving the table lives in the SAME DB as the state, the
    /// precondition for the same-tx co-commit (§3.1).
    #[test]
    fn outbox_migration_is_co_located_in_the_service_db() {
        let set = ColocatedOltp::migrations(&["CREATE TABLE issue (id TEXT PRIMARY KEY);"]);
        assert!(
            set.contains(&OUTBOX_MIGRATION),
            "the outbox table DDL must be in the service DB migration set (co-located)"
        );
        assert!(
            set.iter().any(|m| m.contains("issue")),
            "the service's own domain table is in the same set"
        );
        // The outbox DDL is the frozen 2.3 shape (reused, not re-declared).
        assert!(OUTBOX_MIGRATION.contains("CREATE TABLE IF NOT EXISTS outbox"));
        assert!(OUTBOX_MIGRATION.contains("UNIQUE (aggregate, seq)"));
    }

    /// The staged domain-state is mirrored onto the outbox transaction's modeled state slot, so
    /// the events crate's own co-commit invariant observes the SAME staged state (one
    /// transaction, one truth). Kills the `change_label` mutants: the label must be the joined
    /// staged writes, not an empty/constant string.
    #[test]
    fn staged_state_is_mirrored_onto_the_outbox_transaction() {
        let db = store();
        let mut tx = db.begin(ctx_base("acme")).unwrap();
        tx.stage_state("write A");
        tx.stage_state("write B");
        // The co-located handle records both writes...
        assert_eq!(
            tx.staged_state(),
            &["write A".to_string(), "write B".to_string()]
        );
        // ...and the outbox tx's modeled state slot carries the joined label (the exact value,
        // so an empty/constant `change_label` is caught).
        assert_eq!(
            tx.outbox_tx.staged_state().as_deref(),
            Some("write A; write B"),
            "the joined staged-state label must be mirrored onto the outbox transaction"
        );
    }

    /// **The happy-path co-commit: state + events become durable together.** Before commit the
    /// outbox is empty (emit-iff-committed); after commit both the staged state and the emitted
    /// rows are durable, carrying the per-aggregate commit-order seq (§7.3 cursor).
    #[test]
    fn commit_makes_state_and_events_durable_together() {
        let db = store();
        let mut tx = db.begin(ctx_base("acme")).unwrap();
        tx.stage_state("issue PROJ-1 created");
        let id = tx
            .emit(draft("issues.issue.created", "issue:PROJ-1"), None)
            .unwrap();
        tx.emit(draft("issues.issue.updated", "issue:PROJ-1"), None)
            .unwrap();
        assert_eq!(tx.staged_event_count(), 2, "two events buffered");
        assert_eq!(tx.staged_state(), &["issue PROJ-1 created".to_string()]);
        // Before commit: NOTHING durable — the outbox is empty (the row co-commits with state).
        assert_eq!(
            db.outbox_depth(),
            0,
            "an open co-located tx has written nothing"
        );

        tx.commit().unwrap();
        // After commit: both event rows are durable + unsent (depth 2) and carry the cursor seq.
        assert_eq!(db.outbox_depth(), 2);
        let row = db
            .outbox()
            .row(&id)
            .expect("the committed event row is present");
        assert_eq!(
            row.seq, 0,
            "first event for the aggregate is the seq-0 cursor anchor"
        );
        assert!(
            row.published_at.is_none(),
            "freshly co-committed rows are unsent"
        );
    }

    /// **BUS-D4 direction 1 (no event without committed state): a DROPPED co-located tx writes
    /// nothing.** An abort / a crash between buffering and commit leaves the outbox empty — no
    /// ghost event whose state did not commit.
    #[test]
    fn dropped_tx_writes_neither_state_nor_event() {
        let db = store();
        {
            let mut tx = db.begin(ctx_base("acme")).unwrap();
            tx.stage_state("issue PROJ-9 created");
            tx.emit(draft("issues.issue.created", "issue:PROJ-9"), None)
                .unwrap();
            assert_eq!(tx.staged_event_count(), 1, "buffered, not committed");
            // tx dropped here WITHOUT commit (the crash point between state-write and publish).
        }
        assert_eq!(
            db.outbox_depth(),
            0,
            "an aborted co-located tx writes no event"
        );
        assert_eq!(
            db.outbox().committed_count(),
            0,
            "no ghost row from an abort"
        );
    }

    /// **BUS-D4 direction 2 (no committed state without its event): an injected MID-TX STATE
    /// failure rolls BOTH back.** The state write fails after events were buffered; commit
    /// returns an error and the outbox stays EMPTY — neither side became durable. This is the
    /// "both commit / both roll back" property the prompt's unit test requires.
    #[test]
    fn both_roll_back_under_injected_mid_tx_failure() {
        let db = store();
        let mut tx = db.begin(ctx_base("acme")).unwrap();
        tx.stage_state("issue PROJ-7 created");
        tx.emit(draft("issues.issue.created", "issue:PROJ-7"), None)
            .unwrap();
        assert_eq!(tx.staged_event_count(), 1);

        // Inject the state-write failure mid-transaction.
        let result = tx.commit_with_state_fault("disk full");
        assert!(
            matches!(result, Err(ColocError::CommitRolledBack(_))),
            "an injected mid-tx state failure must roll the whole tx back: {result:?}"
        );
        // NEITHER the state NOR the events committed — the outbox is empty.
        assert_eq!(
            db.outbox_depth(),
            0,
            "a rolled-back co-commit writes no event"
        );
        assert_eq!(
            db.outbox().committed_count(),
            0,
            "no committed row from a rolled-back state write"
        );
    }

    /// **The per-aggregate seq is the §7.3 cross-seam cursor: monotonic, gap-free, and ==
    /// commit order** (the property P-ST-14 restore relies on). Three co-located transactions to
    /// one aggregate commit in order; their seqs are 0, 1, 2 — OLTP commit order == event order.
    #[test]
    fn seq_is_the_monotonic_cross_seam_cursor() {
        let db = store();
        let agg = "issue:CURSOR";
        let mut ids = Vec::new();
        for i in 0..3 {
            let mut tx = db.begin(ctx_base("acme")).unwrap();
            tx.stage_state(format!("state write {i}"));
            let id = tx.emit(draft("issues.issue.updated", agg), None).unwrap();
            tx.commit().unwrap();
            ids.push(id);
        }
        let seqs: Vec<u64> = ids
            .iter()
            .map(|id| db.outbox().row(id).unwrap().seq)
            .collect();
        assert_eq!(
            seqs,
            vec![0, 1, 2],
            "OLTP commit order == event order (the §7.3 cursor)"
        );
    }

    /// Co-location inherits the bounded-pool fast-fail: a transaction cannot even BEGIN if no
    /// OLTP connection is available (it does not block unboundedly). The permit is the
    /// connection the whole co-commit runs on.
    #[test]
    fn begin_fast_fails_when_the_pool_is_saturated() {
        // A 1-connection pool: one open transaction holds the only permit.
        let cfg = OltpConfig {
            max_pool_size: 1,
            statement_timeout_ms: 1_000,
            per_tenant_in_flight_cap: 1,
        };
        let db = ColocatedOltp::open(
            cfg,
            OutboxStore::new(),
            Arc::new(MonotonicMinter::new()) as Arc<dyn IdMinter>,
        )
        .unwrap();
        let _held = db.begin(ctx_base("acme")).unwrap(); // holds the only permit
        let rejected = db.begin(ctx_base("acme"));
        assert!(
            matches!(rejected, Err(ColocError::Pool(_))),
            "a saturated pool must fast-fail the BEGIN, never block (got Ok={})",
            rejected.is_ok()
        );
    }

    /// Releasing a committed transaction frees its OLTP connection for the next one (no leak):
    /// after the first co-commit completes, a second transaction begins on the freed connection.
    #[test]
    fn committing_frees_the_connection_for_the_next_tx() {
        let cfg = OltpConfig {
            max_pool_size: 1,
            statement_timeout_ms: 1_000,
            per_tenant_in_flight_cap: 1,
        };
        let db = ColocatedOltp::open(
            cfg,
            OutboxStore::new(),
            Arc::new(MonotonicMinter::new()) as Arc<dyn IdMinter>,
        )
        .unwrap();
        {
            let mut tx = db.begin(ctx_base("acme")).unwrap();
            tx.emit(draft("issues.issue.created", "issue:A"), None)
                .unwrap();
            tx.commit().unwrap();
        }
        // The connection is free again — a second tx begins (no leaked permit).
        let mut tx2 = db.begin(ctx_base("acme")).unwrap();
        tx2.emit(draft("issues.issue.created", "issue:B"), None)
            .unwrap();
        tx2.commit().unwrap();
        assert_eq!(db.outbox_depth(), 2, "both co-committed events are durable");
    }

    /// The `ColocError` Display is loud and specific (never an empty string) — a rolled-back
    /// co-commit is observable (EI-01 §3, observability is part of the pass).
    #[test]
    fn coloc_error_display_is_loud() {
        let e = ColocError::CommitRolledBack("disk full".into());
        let msg = e.to_string();
        assert!(!msg.is_empty());
        assert!(msg.contains("rolled back"), "must name the rollback: {msg}");
        assert!(
            msg.contains("neither"),
            "must say neither side committed: {msg}"
        );
    }
}
