//! # Durable PG backing for the transactional `outbox` + relay (SI-007, MR-009b W3b.2)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/00-platform-substrate.md` §3.3 (the outbox
//! relay) + the W3b execution contract
//! `planning/system-reviews/2026-06-26/16-w3b-outbox-design.md`. Closes census SI-007
//! ("`OutboxStore` is an in-memory `Arc<Mutex<Inner>>`, rebuilt empty on every process start")
//! at the *production* level: this module is the REAL durable backing the events
//! [`myelin_events::OutboxStore`] binds to via the [`myelin_events::DurableOutboxBacking`] seam
//! (added in W3b.1), so the transactional co-commit + the unsent-row ledger + the relay drain
//! **survive a process restart** (a committed-but-unsent row after a crash is still relayed
//! because it lives in Postgres, not a per-process `Arc<Mutex<Inner>>`).
//!
//! ## What this is (the reuse story — EXTEND the frozen table, never fork; delegate to PgRelay)
//! - The TABLE is the frozen [`myelin_events::OUTBOX_MIGRATION`] (`event_id UNIQUE`,
//!   `UNIQUE(aggregate, seq)`, `attempts`, `published_at`) — the SAME table
//!   [`crate::pgrelay::PgRelay`] already owns; the provider's
//!   [`crate::provider::foundation_migrations`] applies it at boot. This module does NOT re-define
//!   or add a parallel outbox table (EI-01 §7). There is exactly ONE outbox table.
//! - The SEAM is the existing [`myelin_events::DurableOutboxBacking`] trait: [`PgOutboxBacking`]
//!   is the production impl a later composition root (W3b.4) injects via
//!   [`myelin_events::OutboxStore::durable`]. The in-memory `OutboxStore::new` stays the
//!   test/dev double (gated in W3b.6). There is NO second outbox store.
//! - **ALL outbox SQL lives in [`crate::pgrelay::PgRelay`]** — the ONE sanctioned, lint-excluded,
//!   relay-INTERNAL outbox-query site. This module holds NO raw queries: every verb is a thin
//!   sync→async delegation to a `PgRelay` method (`commit_staged_atomic`, `unsent_depth`,
//!   `dead_count`, the read snapshots, and the composite `drain_once_dead_letter`). So the durable
//!   backing reuses the proven `co_commit_in_tx` seq discipline + `FOR UPDATE SKIP LOCKED` claim,
//!   never rebuilds them.
//!
//! ## Dead-letter encoding (no parallel table, no schema change): `published_at IS NULL AND
//! attempts >= MAX_PUBLISH_ATTEMPTS`
//! The frozen table already carries `attempts`. A row is DEAD-LETTERED exactly when it exhausted
//! the retry bound WITHOUT being published. This mirrors the in-memory arm (where a dead-lettered
//! row is moved out of the live set into `dead_letters` with `attempts == MAX_PUBLISH_ATTEMPTS`)
//! with ZERO DDL — the `PgRelay` reads simply partition the table on this predicate.
//!
//! ## Duplicate-`event_id` semantics — REJECT the whole commit (parity with the Memory arm)
//! The W3b.1 verifier flagged the open question. `commit_staged` → `PgRelay::commit_staged_atomic`
//! inserts with a PLAIN `INSERT` (no `ON CONFLICT`), so the `outbox_event_id_unique` constraint
//! aborts the transaction on a duplicate → nothing is staged → `Err` (parity with the in-memory
//! [`myelin_events::OutboxStore`] `commit`, which ERRORS the whole commit; proven in the CDC suite
//! `tests/integration_mr009b_outbox_durable.rs`). A racing (`aggregate`, `seq`) collision is
//! DISTINCT — benign contention, retried until the loser gets the next contiguous seq (gap-free,
//! true commit order under concurrency; EB-03, durably).
//!
//! ## How a sync trait drives the async client
//! [`myelin_events::DurableOutboxBacking`] is sync (it matches the emit/relay call sites);
//! `sqlx` is async, so [`PgOutboxBacking`] holds a `tokio::runtime::Handle` and drives each op
//! with `block_in_place` + `block_on` — the SAME bridge [`crate::events_durable::DurableDedupBacking`]
//! and `myelin_events::nats` use.
//!
//! The durable code compiles in the default build (sqlx is a non-optional dep); the LIVE proof
//! (`tests/integration_mr009b_outbox_durable.rs`) is `integration`-gated like the rest of the
//! live-PG suite.
//!
//! **Scanner posture (`no-in-memory-durable-store`):** [`PgOutboxBacking`] holds a `PgPool` (a
//! pool token) and NO in-memory collection, and its name does not end in a durable role suffix
//! (`Store`/`Registry`/`Outbox`/`Ledger`) — so it is ADMITTED and adds no baseline entry. The
//! in-memory `OutboxStore` holder stays the single known baseline entry (flipped in W3b.6).

use sqlx::postgres::PgPool;

use myelin_events::relay::{BusTransport, DrainReport, MAX_PUBLISH_ATTEMPTS};
use myelin_events::{EventId, OutboxError, OutboxRow, Result, Timestamp};

use crate::pg::PgError;
use crate::pgrelay::PgRelay;

/// The REAL durable `outbox` backing over the OLTP `PgPool` (SI-007). Cloneable (the pool is an
/// `Arc`-backed handle). Constructed by a later composition root (W3b.4) over the SAME pool the
/// stores/relay use, and injected into [`myelin_events::OutboxStore::durable`].
///
/// Holds a `PgPool` (durable) + a `tokio::runtime::Handle` (the sync→async bridge). Deliberately
/// NOT named `*Store`/`*Outbox`/`*Ledger` and carrying NO in-memory collection, so the
/// `no-in-memory-durable-store` scanner ADMITS it (a pool token is the durability proof). Every
/// verb delegates to a [`PgRelay`] method — this struct owns no SQL of its own.
#[derive(Clone)]
pub struct PgOutboxBacking {
    pool: PgPool,
    rt: tokio::runtime::Handle,
}

impl PgOutboxBacking {
    /// Wrap a pool as the durable outbox backing. `rt` is the runtime handle the sync trait methods
    /// drive the async sqlx client on. The caller must have applied
    /// [`myelin_events::OUTBOX_MIGRATION`] (the provider's
    /// [`crate::provider::foundation_migrations`] does this at boot).
    pub fn new(pool: PgPool, rt: tokio::runtime::Handle) -> PgOutboxBacking {
        PgOutboxBacking { pool, rt }
    }

    fn relay(&self) -> PgRelay {
        PgRelay::new(self.pool.clone())
    }

    fn block<F: std::future::Future>(&self, fut: F) -> F::Output {
        tokio::task::block_in_place(|| self.rt.block_on(fut))
    }
}

/// The legacy read half of `DurableOutboxBacking` is infallible, so a PostgreSQL read failure cannot
/// be returned to its caller. It must nevertheless never masquerade as zero depth, no dead letters,
/// or a missing row: those values can falsely certify a clean drain. Route every such failure through
/// one redacted fail-static boundary until the trait's read surface becomes fallible.
fn require_outbox_read<T>(operation: &str, result: std::result::Result<T, PgError>) -> T {
    result.unwrap_or_else(|_| {
        panic!("FAIL-STATIC: durable outbox {operation} read failed; state is unknown")
    })
}

impl myelin_events::DurableOutboxBacking for PgOutboxBacking {
    fn commit_staged(&self, rows: Vec<OutboxRow>) -> Result<()> {
        self.block(async {
            self.relay()
                .commit_staged_atomic(&rows)
                .await
                .map_err(|e| OutboxError(e.to_string()))
        })
    }

    /// **H1 (peer-review #7 re-prosecution) — the real absorb arm.** Delegates to
    /// [`PgRelay::commit_staged_absorb`]: `ON CONFLICT (event_id) DO NOTHING` + payload-equality
    /// verification, so a deterministic crash-window re-emit is absorbed (no `Err` → no `Retry`
    /// livelock) while a divergent-payload collision still rejects.
    fn commit_staged_absorb(&self, rows: Vec<OutboxRow>) -> Result<()> {
        self.block(async {
            self.relay()
                .commit_staged_absorb(&rows)
                .await
                .map_err(|e| OutboxError(e.to_string()))
        })
    }

    fn outbox_depth(&self) -> usize {
        let depth = self.block(async { self.relay().unsent_depth().await });
        require_outbox_read("depth", depth).max(0) as usize
    }

    fn dead_letter_count(&self) -> usize {
        let count = self.block(async { self.relay().dead_count().await });
        require_outbox_read("dead-letter count", count).max(0) as usize
    }

    fn oldest_unsent_recorded_at(&self) -> Option<Timestamp> {
        let timestamp = self.block(async { self.relay().oldest_unsent_recorded_at().await });
        require_outbox_read("oldest-unsent timestamp", timestamp).map(Timestamp)
    }

    fn committed_count(&self) -> usize {
        let count = self.block(async { self.relay().committed_live_count().await });
        require_outbox_read("committed count", count).max(0) as usize
    }

    fn row(&self, id: &EventId) -> Option<OutboxRow> {
        let row = self.block(async { self.relay().committed_row(id).await });
        require_outbox_read("row", row)
    }

    fn committed_rows(&self) -> Vec<OutboxRow> {
        let rows = self.block(async { self.relay().committed_live_rows().await });
        require_outbox_read("committed rows", rows)
    }

    fn try_committed_rows(&self) -> Result<Vec<OutboxRow>> {
        self.block(async {
            self.relay()
                .committed_live_rows()
                .await
                .map_err(|e| OutboxError(e.to_string()))
        })
    }

    fn try_retained_rows(&self) -> Result<Vec<OutboxRow>> {
        self.block(async {
            self.relay()
                .retained_rows()
                .await
                .map_err(|e| OutboxError(e.to_string()))
        })
    }

    fn dead_letters(&self) -> Vec<OutboxRow> {
        let rows = self.block(async { self.relay().dead_rows().await });
        require_outbox_read("dead-letter rows", rows)
    }

    fn drain_once(&self, transport: &dyn BusTransport, batch: usize) -> Result<DrainReport> {
        // The SINGLE composite verb: delegate to the PgRelay bounded-retry + dead-letter discipline
        // (claim FOR UPDATE SKIP LOCKED → publish in (aggregate, seq) order → per-row mark-sent /
        // attempts-bump / dead-letter at MAX_PUBLISH_ATTEMPTS, all in ONE tx; a failed publish does
        // NOT abort the pass). The relay owns the `outbox` table — reuse it, don't rebuild.
        self.block(async {
            self.relay()
                .drain_once_dead_letter(transport, batch as i64, MAX_PUBLISH_ATTEMPTS)
                .await
                .map_err(|e| OutboxError(e.to_string()))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::require_outbox_read;
    use crate::pg::PgError;

    #[test]
    fn infallible_read_boundary_fails_loud_without_logging_database_detail() {
        let panic = std::panic::catch_unwind(|| {
            require_outbox_read::<usize>(
                "depth",
                Err(PgError::Query("sentinel database detail".to_string())),
            )
        })
        .expect_err("a durable read failure must not become a zero value");
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .expect("panic payload is a string");
        assert!(message.contains("durable outbox depth read failed"));
        assert!(!message.contains("sentinel database detail"));
    }
}
