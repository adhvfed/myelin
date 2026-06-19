//! `PgRelay` — the OLTP-co-located outbox table + the RELAY, backed by REAL Postgres.
//!
//! **Stage 2 / infra.** This is the storage-tier RELAY: it owns the co-located `outbox` table
//! (the frozen [`myelin_events::OUTBOX_MIGRATION`], RUN not re-defined) and drains it to the
//! [`BusTransport`](myelin_events::relay::BusTransport) with the real
//! `SELECT … FOR UPDATE SKIP LOCKED` claim. It is the ONE legitimate broker-publish site for
//! the OLTP service — exactly the role `myelin-events/src/relay.rs` plays for the in-process
//! floor (BUS-2: the relay is the only component on the broker-publish side). Its `bus.put(...)`
//! forwards an ALREADY-committed outbox row (emit-iff-committed), it is NOT a fire-and-forget
//! bypass of `OutboxTx::emit`.
//!
//! Like `relay.rs`, this file is a NAMED, LOUD exclusion in the `no-raw-publish` scanner
//! (`lint-gate.rs` + `tests/workspace_clean.rs`) — the relay is the sanctioned publisher,
//! documented here, never a silent skip. The outbox queries here are relay-INTERNAL (the outbox
//! is keyed by `(aggregate, seq)` and drained across aggregates by the relay), not tenant-store
//! queries — so they correctly carry no per-row tenant predicate, the same as `relay.rs`.
//!
//! ## `residency-pin` lint — region pinned PER SESSION (`@residency-cell-pinned:file`)
//! The relay opens its bounded sqlx pool region-agnostic (the same NAMED floor `oltp.rs` /
//! `pg.rs` record); the per-POOL runtime region-pin is the STOR-D5 gate (P-ST-15 / P-102). The
//! file-level waiver marker `@residency-cell-pinned:file` records this floor LOUDLY (EI-01 §4).

use myelin_events::relay::{BusTransport, Delivery};
use myelin_events::EventEnvelope;
use sqlx::postgres::PgPool;
use sqlx::Row;

use crate::pg::PgError;

/// The OLTP-co-located outbox + relay over a bounded sqlx `PgPool`. Cloneable (the pool is an
/// `Arc`-backed handle). Shares the SAME pool as the [`crate::pg::PgStore`] in a real service
/// (the outbox co-commits in the service DB); here it is constructed from a pool handle.
#[derive(Clone)]
pub struct PgRelay {
    pool: PgPool,
}

impl PgRelay {
    /// Wrap a pool as the OLTP relay (the outbox lives in the same OLTP DB the service writes).
    pub fn new(pool: PgPool) -> PgRelay {
        PgRelay { pool }
    }

    /// Insert an outbox row in the frozen `outbox` table shape: the envelope as JSONB,
    /// `published_at` NULL (so the relay's unsent index claims it). `aggregate`/`seq` carry the
    /// per-aggregate ordering key (`UNIQUE(aggregate, seq)`).
    pub async fn enqueue(
        &self,
        aggregate: &str,
        seq: i64,
        envelope: &EventEnvelope,
    ) -> Result<(), PgError> {
        let payload = serde_json::to_value(envelope)
            .map_err(|e| PgError::Query(format!("serialize envelope: {e}")))?;
        sqlx::query(
            "INSERT INTO outbox (event_id, aggregate, seq, subject, envelope) \
             VALUES ($1, $2, $3, $4, $5) ON CONFLICT (event_id) DO NOTHING",
        )
        .bind(&envelope.event_id.0)
        .bind(aggregate)
        .bind(seq)
        .bind(&envelope.subject.0)
        .bind(payload)
        .execute(&self.pool)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(())
    }

    /// **Same-tx co-commit (BUS-D4 emit-iff-committed):** insert a domain STATE row into
    /// `state_table` AND the outbox row in ONE transaction — both commit, or both roll back. This
    /// is the structural emit-iff-committed property the silent-data-loss floor rests on: a
    /// relay can only ever publish an event whose state change durably committed (no ghost
    /// without its committed state change), and a committed state change always has its outbox row
    /// (no lost emit). `state_table` is expected to carry `(id text primary key, event_id text)`.
    /// Used by the stage-3 OUTBOX-NO-LOSS-UNDER-CRASH drill to write N events co-committed with a
    /// state change before crashing the relay.
    pub async fn enqueue_with_state(
        &self,
        state_table: &str,
        state_id: &str,
        aggregate: &str,
        seq: i64,
        envelope: &EventEnvelope,
    ) -> Result<(), PgError> {
        let payload = serde_json::to_value(envelope)
            .map_err(|e| PgError::Query(format!("serialize envelope: {e}")))?;
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| PgError::Query(e.to_string()))?;
        // The domain state change. The table name is a trusted, drill-internal identifier (never
        // user input) — it is interpolated because Postgres does not bind identifiers; the VALUES
        // are bound parameters.
        sqlx::query(&format!(
            "INSERT INTO {state_table} (id, event_id) VALUES ($1, $2) ON CONFLICT (id) DO NOTHING"
        ))
        .bind(state_id)
        .bind(&envelope.event_id.0)
        .execute(&mut *tx)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        // The outbox row, in the SAME transaction. If the commit below fails, neither the state
        // row nor the outbox row exists — emit-iff-committed by construction.
        sqlx::query(
            "INSERT INTO outbox (event_id, aggregate, seq, subject, envelope) \
             VALUES ($1, $2, $3, $4, $5) ON CONFLICT (event_id) DO NOTHING",
        )
        .bind(&envelope.event_id.0)
        .bind(aggregate)
        .bind(seq)
        .bind(&envelope.subject.0)
        .bind(payload)
        .execute(&mut *tx)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        tx.commit().await.map_err(|e| PgError::Query(e.to_string()))?;
        Ok(())
    }

    /// One relay drain pass against REAL Postgres: in ONE transaction, claim up to `batch`
    /// unsent rows with `SELECT … FOR UPDATE SKIP LOCKED` (so a second relay worker SKIPs a
    /// claimed row — no double-claim across replicas), publish each to `bus` (carrying the
    /// stable `event_id` as the `Nats-Msg-Id` dedup id → 0 ghost), and mark the published rows
    /// `published_at = now()`. Returns how many rows were published.
    ///
    /// Emit-iff-published: a row is marked sent only if its publish was Accepted/Deduplicated; a
    /// publish failure aborts the transaction (the claim releases, the row stays unsent to
    /// retry) — the 0-lost property the relay floor models.
    pub async fn relay_once<B: BusTransport>(&self, bus: &B, batch: i64) -> Result<usize, PgError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| PgError::Query(e.to_string()))?;

        let rows = sqlx::query(
            "SELECT event_id, subject, envelope FROM outbox \
             WHERE published_at IS NULL \
             ORDER BY aggregate, seq \
             FOR UPDATE SKIP LOCKED LIMIT $1",
        )
        .bind(batch)
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;

        let mut published = 0usize;
        for row in &rows {
            let event_id: String = row.get("event_id");
            let payload: serde_json::Value = row.get("envelope");
            let envelope: EventEnvelope = serde_json::from_value(payload)
                .map_err(|e| PgError::Query(format!("deserialize envelope: {e}")))?;

            // The relay's sanctioned broker publish (BUS-2): the row was already durably
            // committed to the outbox; the relay forwards it with the stable event_id as the
            // broker-side dedup id (0 ghost). A transport error aborts the whole tx → the claim
            // releases, rows stay unsent.
            match bus.put(&envelope.subject, &envelope, &envelope.event_id) {
                Ok(Delivery::Accepted) | Ok(Delivery::Deduplicated) => {}
                Err(e) => return Err(PgError::Publish(e.0)),
            }

            sqlx::query("UPDATE outbox SET published_at = now() WHERE event_id = $1")
                .bind(&event_id)
                .execute(&mut *tx)
                .await
                .map_err(|e| PgError::Query(e.to_string()))?;
            published += 1;
        }

        tx.commit().await.map_err(|e| PgError::Query(e.to_string()))?;
        Ok(published)
    }

    /// **Crash-injection drain (stage-3 OUTBOX-NO-LOSS-UNDER-CRASH drill).** Claims the unsent
    /// rows and PUBLISHES each to `bus` — exactly as [`relay_once`](Self::relay_once) — but
    /// SIMULATES A CRASH after publishing `crash_after` rows: the per-row `published_at` UPDATEs
    /// are NEVER committed (the transaction is dropped/rolled back). This reproduces the worst-case
    /// silent-data-loss window: the relay forwarded the event to the broker but died before
    /// recording that it did. On restart, [`relay_once`](Self::relay_once) re-claims those rows
    /// (they are still `published_at IS NULL`) and re-publishes them — the broker's
    /// `Nats-Msg-Id = event_id` dedup suppresses the re-delivery (0 ghost), and because no
    /// committed row was ever dropped (0 lost), every committed event is delivered exactly once.
    ///
    /// Returns how many rows were published to the broker before the simulated crash (these are
    /// the rows that will be re-published-and-deduplicated by the post-restart relay).
    pub async fn relay_once_crash_after<B: BusTransport>(
        &self,
        bus: &B,
        batch: i64,
        crash_after: usize,
    ) -> Result<usize, PgError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| PgError::Query(e.to_string()))?;

        let rows = sqlx::query(
            "SELECT event_id, subject, envelope FROM outbox \
             WHERE published_at IS NULL \
             ORDER BY aggregate, seq \
             FOR UPDATE SKIP LOCKED LIMIT $1",
        )
        .bind(batch)
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;

        let mut published = 0usize;
        for row in &rows {
            let event_id: String = row.get("event_id");
            let payload: serde_json::Value = row.get("envelope");
            let envelope: EventEnvelope = serde_json::from_value(payload)
                .map_err(|e| PgError::Query(format!("deserialize envelope: {e}")))?;

            match bus.put(&envelope.subject, &envelope, &envelope.event_id) {
                Ok(Delivery::Accepted) | Ok(Delivery::Deduplicated) => {}
                Err(e) => return Err(PgError::Publish(e.0)),
            }
            published += 1;

            // CRASH SIMULATION: drop the transaction WITHOUT committing the published_at UPDATEs.
            // The rows we published stay `published_at IS NULL`, so the restarted relay re-claims
            // and re-publishes them — exactly the crash-mid-drain window the drill exercises.
            if published >= crash_after {
                drop(tx); // rolls back: no published_at is recorded for ANY row this pass.
                return Ok(published);
            }

            sqlx::query("UPDATE outbox SET published_at = now() WHERE event_id = $1")
                .bind(&event_id)
                .execute(&mut *tx)
                .await
                .map_err(|e| PgError::Query(e.to_string()))?;
        }

        // crash_after was never reached (fewer rows than crash_after) — roll back anyway so the
        // whole pass is a "crash" (no marks committed).
        drop(tx);
        Ok(published)
    }

    /// The count of unsent outbox rows (`published_at IS NULL`) — the `outbox_depth` signal, read
    /// straight from the DB. 0 after a full drain.
    pub async fn outbox_depth(&self) -> Result<i64, PgError> {
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM outbox WHERE published_at IS NULL")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(n)
    }
}
