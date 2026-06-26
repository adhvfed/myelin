//! # Durable PG backing for the `consumer_dedup` ledger (SI-023, MR-023, the P-522/523 floor)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/event-bus.md` §3.3 (the `consumer_dedup`
//! ledger) + §4.2 (at-least-once + idempotent consumers ≈ effectively-once). Closes census SI-023
//! ("consumer-dedup ledger is an in-memory `HashSet`, not a DB table") at the *production* level:
//! the events [`myelin_events::DedupLedger`] was an in-memory `HashSet` rebuilt empty on every
//! process start (so a redelivery AFTER A RESTART re-ran the handler — a 0-ghost regression). This
//! module is the REAL durable backing it binds to via the [`myelin_events::DurableDedup`] seam, so
//! the dedup mark **survives a process restart**.
//!
//! ## What this is (the reuse story — EXTEND, never fork)
//! - The TABLE is the frozen [`myelin_events::CONSUMER_DEDUP_MIGRATION`] (`(consumer, event_id)` PK,
//!   `ON CONFLICT DO NOTHING`). This module RUNS it; it does NOT re-define the dedup table shape
//!   (EI-01 §7). The provider's [`crate::provider::foundation_migrations`] already applies it at boot.
//! - The SEAM is the existing [`myelin_events::DurableDedup`] trait (MR-023, added in `dedup.rs`):
//!   [`DurableDedupBacking`] is the production impl the events `serve()` composition root
//!   ([`crate::events_serve`]) injects via [`myelin_events::DedupLedger::durable`]. The in-memory
//!   `DedupLedger::new` stays the test-double. There is NO second dedup ledger or table.
//!
//! ## Relay/consumer-internal table (the tenant-predicate posture — same as `pgrelay.rs`)
//! The `consumer_dedup` table is keyed `(consumer, event_id)` and carries **no tenant column** (the
//! `event_id` is a globally-unique ULID; dedup is per-consumer, cross-tenant by design — the frozen
//! 2.5 shape). Its queries are therefore consumer-INTERNAL, not tenant-store queries, so they carry
//! no per-row tenant predicate — exactly the relay-internal posture [`crate::pgrelay`]'s outbox
//! queries take. Like `pgrelay.rs`, this file is a NAMED, LOUD exclusion in the `tenant-predicate`
//! workspace scanner (`tests/workspace_clean.rs`), documented here, never a silent skip.
//!
//! ## Fail-direction (0-lost preserved): a DB error reports FRESH, never a silent "already handled"
//! [`DurableDedup::mark_handled`] returns `true` (FRESH → run the handler) when it cannot reach the
//! DB. A durable dedup must NEVER report a false "already handled" on an error, because the consumer
//! would then SKIP + ack the event → silent data loss. Reporting FRESH degrades effectively-once to
//! at-least-once under a DB outage (the idempotent handler tolerates the re-run), never to loss.
//!
//! ## How a sync trait drives the async client
//! [`myelin_events::DurableDedup`] is sync (it matches the consumer's sync `mark_handled` call
//! site); `sqlx` is async, so [`DurableDedupBacking`] holds a `tokio::runtime::Handle` and drives
//! each op with `block_in_place` + `block_on` — the same bridge `nats::NatsJetStreamBus` and the
//! storage S3/Valkey backings use.
//!
//! Feature-gated `integration` (it pulls the real sqlx client), like the rest of the live-PG code.

use sqlx::postgres::PgPool;

use myelin_events::{ConsumerName, DurableDedup, EventId};

/// The REAL durable `consumer_dedup` backing over the OLTP `PgPool`. Cloneable (the pool is an
/// `Arc`-backed handle). Constructed by the events `serve()` composition root over the SAME pool
/// the relay/stores use, and injected into [`myelin_events::DedupLedger::durable`].
#[derive(Clone)]
pub struct DurableDedupBacking {
    pool: PgPool,
    rt: tokio::runtime::Handle,
}

impl DurableDedupBacking {
    /// Wrap a pool as the durable dedup backing. `rt` is the runtime handle the sync trait methods
    /// drive the async sqlx client on. The caller must have applied
    /// [`myelin_events::CONSUMER_DEDUP_MIGRATION`] (the provider's
    /// [`crate::provider::foundation_migrations`] does this at boot).
    pub fn new(pool: PgPool, rt: tokio::runtime::Handle) -> DurableDedupBacking {
        DurableDedupBacking { pool, rt }
    }

    fn block<F: std::future::Future>(&self, fut: F) -> F::Output {
        tokio::task::block_in_place(|| self.rt.block_on(fut))
    }
}

impl DurableDedup for DurableDedupBacking {
    fn mark_handled(&self, consumer: &ConsumerName, event_id: &EventId) -> bool {
        self.block(async {
            // `INSERT … ON CONFLICT DO NOTHING`: rows_affected == 1 ⇒ the pair was FRESH (run the
            // handler); == 0 ⇒ the pair already existed (a redelivery — SKIP + ack, 0 dup). The row
            // SURVIVES a process restart, so a redelivery after a restart still reads 0 → deduped.
            match sqlx::query(
                "INSERT INTO consumer_dedup (consumer, event_id) VALUES ($1, $2) \
                 ON CONFLICT (consumer, event_id) DO NOTHING",
            )
            .bind(&consumer.0)
            .bind(&event_id.0)
            .execute(&self.pool)
            .await
            {
                Ok(res) => res.rows_affected() == 1,
                // Fail-direction (0-lost): a DB error reports FRESH so the handler RUNS — never a
                // silent "already handled" (which would skip + ack → lost event).
                Err(_) => true,
            }
        })
    }

    fn is_handled(&self, consumer: &ConsumerName, event_id: &EventId) -> bool {
        self.block(async {
            // `EXISTS(..)` yields a bool (a `SELECT 1` would decode as int4, not i64). On a DB error
            // report NOT handled (fail-closed for a read — never falsely claim "already handled").
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM consumer_dedup WHERE consumer = $1 AND event_id = $2)",
            )
            .bind(&consumer.0)
            .bind(&event_id.0)
            .fetch_one(&self.pool)
            .await
            .unwrap_or(false);
            exists
        })
    }

    fn revert(&self, consumer: &ConsumerName, event_id: &EventId) {
        // A `Retry` reverts its speculative mark so a redelivery re-runs the handler. Best-effort
        // (an error leaves the mark; the same-tx-as-handler atomicity that makes revert truly atomic
        // with the handler rollback is the MR-023b floor named in `dedup.rs`).
        self.block(async {
            let _ = sqlx::query("DELETE FROM consumer_dedup WHERE consumer = $1 AND event_id = $2")
                .bind(&consumer.0)
                .bind(&event_id.0)
                .execute(&self.pool)
                .await;
        });
    }

    fn forget(&self, consumer: &ConsumerName, event_id: &EventId) -> bool {
        // The reindex-after-wipe path: `true` iff a mark was removed (so the cold rebuild re-applies
        // the snapshot). A scoped `DELETE` (forward-only; the snapshot id re-applies idempotently).
        self.block(async {
            match sqlx::query("DELETE FROM consumer_dedup WHERE consumer = $1 AND event_id = $2")
                .bind(&consumer.0)
                .bind(&event_id.0)
                .execute(&self.pool)
                .await
            {
                Ok(res) => res.rows_affected() > 0,
                Err(_) => false,
            }
        })
    }
}
