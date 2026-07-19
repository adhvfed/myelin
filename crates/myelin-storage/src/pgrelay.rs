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

use myelin_events::relay::{
    BusTransport, Delivery, DrainReport, EventPublisher, MAX_PUBLISH_ATTEMPTS,
};
use myelin_events::{
    validate_event_type, AggregateKey, ArtifactRef, EventEnvelope, EventId, OutboxRow, Region,
    StreamSubject, Timestamp,
};
use sqlx::postgres::PgPool;
use sqlx::Row;
use std::collections::HashSet;

use crate::kms::PiiKeyRef as KmsPiiKeyRef;
use crate::pg::PgError;

/// How many times a racing (`aggregate`, `seq`) collision retries in [`PgRelay::commit_staged_atomic`]
/// before giving up. A concurrent committer that lost the `MAX(seq)+1` race retries with the next
/// contiguous seq; the bound only needs to exceed the realistic concurrency on one hot aggregate.
const SEQ_CONTENTION_RETRIES: u32 = 128;

/// A defensive ceiling on configuration itself. The elected relay still requires callers to choose
/// their lower operational envelope limit explicitly; this prevents `usize::MAX` from becoming an
/// accidental "bounded" production setting.
pub const MAX_CONFIGURED_ENVELOPE_BYTES: usize = 16 * 1024 * 1024;

/// Cell scope and finite message bound required by the strict elected-relay path.
///
/// Legacy [`PgRelay::new`] + [`PgRelay::relay_once`] remain unscoped for existing integration and
/// migration callers. The elected production wrapper accepts this type at construction and invokes
/// only [`PgRelay::relay_once_scoped`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelayValidationConfig {
    region: Region,
    max_envelope_bytes: usize,
}

impl RelayValidationConfig {
    pub fn new(
        region: Region,
        max_envelope_bytes: usize,
    ) -> Result<Self, RelayValidationConfigError> {
        if region.0.trim().is_empty() {
            return Err(RelayValidationConfigError::EmptyRegion);
        }
        if !(1..=MAX_CONFIGURED_ENVELOPE_BYTES).contains(&max_envelope_bytes) {
            return Err(RelayValidationConfigError::InvalidEnvelopeLimit {
                max: MAX_CONFIGURED_ENVELOPE_BYTES,
            });
        }
        Ok(Self {
            region,
            max_envelope_bytes,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RelayValidationConfigError {
    EmptyRegion,
    InvalidEnvelopeLimit { max: usize },
}

impl core::fmt::Display for RelayValidationConfigError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::EmptyRegion => write!(f, "relay region must not be empty"),
            Self::InvalidEnvelopeLimit { max } => {
                write!(f, "relay envelope limit must be between 1 and {max} bytes")
            }
        }
    }
}

impl std::error::Error for RelayValidationConfigError {}

#[derive(Clone, Copy)]
struct PermanentRowError {
    code: &'static str,
    detail: &'static str,
}

impl PermanentRowError {
    const fn new(code: &'static str, detail: &'static str) -> Self {
        Self { code, detail }
    }
}

struct ClaimedRow {
    event_id: String,
    aggregate: String,
    seq: i64,
    subject: String,
    payload: serde_json::Value,
}

fn validate_claimed_row(
    row: &ClaimedRow,
    config: &RelayValidationConfig,
) -> Result<EventEnvelope, PermanentRowError> {
    let encoded = serde_json::to_vec(&row.payload).map_err(|_| {
        PermanentRowError::new(
            "invalid_envelope_json",
            "outbox envelope could not be encoded as canonical JSON",
        )
    })?;
    if encoded.len() > config.max_envelope_bytes {
        return Err(PermanentRowError::new(
            "envelope_too_large",
            "serialized event envelope exceeds the configured byte limit",
        ));
    }

    let envelope: EventEnvelope = serde_json::from_value(row.payload.clone()).map_err(|_| {
        PermanentRowError::new(
            "invalid_envelope_json",
            "outbox envelope is not a canonical EventEnvelope",
        )
    })?;
    if row.event_id != envelope.event_id.0 {
        return Err(PermanentRowError::new(
            "event_id_mismatch",
            "outbox event_id disagrees with the envelope event_id",
        ));
    }
    if row.subject != envelope.subject.0 {
        return Err(PermanentRowError::new(
            "subject_mismatch",
            "outbox subject disagrees with the envelope subject",
        ));
    }
    if row.aggregate != envelope.aggregate.0 {
        return Err(PermanentRowError::new(
            "aggregate_mismatch",
            "outbox aggregate disagrees with the envelope aggregate",
        ));
    }
    if envelope.schema_ver == 0 {
        return Err(PermanentRowError::new(
            "invalid_schema_version",
            "event schema_ver must be at least one",
        ));
    }
    if validate_event_type(&envelope.type_.0).is_err() {
        return Err(PermanentRowError::new(
            "invalid_event_taxonomy",
            "event type is not admitted by the canonical taxonomy",
        ));
    }
    if StreamSubject::of(&envelope).is_err() {
        return Err(PermanentRowError::new(
            "invalid_stream_subject",
            "event cannot form a safe canonical stream subject",
        ));
    }
    if envelope.actor.0.tenant != envelope.tenant {
        return Err(PermanentRowError::new(
            "actor_tenant_mismatch",
            "event actor tenant disagrees with the envelope tenant",
        ));
    }
    let parsed_subject = myelin_refs::parse_scoped(&envelope.subject.0).map_err(|_| {
        PermanentRowError::new(
            "invalid_artifact_ref",
            "event subject is not a canonical ArtifactRef",
        )
    })?;
    if parsed_subject.artifact_ref != envelope.subject {
        return Err(PermanentRowError::new(
            "invalid_artifact_ref",
            "event subject is not the canonical ArtifactRef spelling",
        ));
    }
    if parsed_subject.tenant != envelope.tenant {
        return Err(PermanentRowError::new(
            "subject_tenant_mismatch",
            "event subject tenant disagrees with the envelope tenant",
        ));
    }
    match (envelope.contains_personal_data, &envelope.pii_key_ref) {
        (false, None) => {}
        (true, Some(key_ref)) => {
            let parsed = KmsPiiKeyRef::parse(&key_ref.0).ok_or_else(|| {
                PermanentRowError::new(
                    "invalid_pii_key_ref",
                    "event pii_key_ref is not a canonical KMS key reference",
                )
            })?;
            if parsed.tenant != envelope.tenant {
                return Err(PermanentRowError::new(
                    "pii_key_tenant_mismatch",
                    "event pii_key_ref tenant disagrees with the envelope tenant",
                ));
            }
        }
        _ => {
            return Err(PermanentRowError::new(
                "pii_presence_mismatch",
                "contains_personal_data and pii_key_ref presence disagree",
            ));
        }
    }
    if envelope.region != config.region {
        return Err(PermanentRowError::new(
            "wrong_relay_region",
            "event region disagrees with the relay cell region",
        ));
    }
    Ok(envelope)
}

/// The `SELECT` projection for reconstructing an [`OutboxRow`]: every outbox column plus the
/// `published_at` cast to an RFC-3339 UTC string (`published_at_str`).
const ROW_PROJECTION: &str = "event_id, aggregate, seq, subject, envelope, attempts, \
     to_char(published_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS published_at_str";

/// A typed outcome of one `commit_staged_atomic` transaction attempt, so the retry loop can
/// distinguish a benign seq-contention collision (retry) from a genuine duplicate `event_id`
/// (reject — parity with the in-memory arm) or any other DB error (propagate).
enum CommitAttempt {
    Committed,
    SeqContention,
    DuplicateEventId(String),
    Db(PgError),
}

/// Reconstruct an [`OutboxRow`] from a selected outbox table row (the `ROW_PROJECTION` columns).
fn row_from_pg(row: &sqlx::postgres::PgRow) -> Result<OutboxRow, PgError> {
    let event_id: String = row.get("event_id");
    let aggregate: String = row.get("aggregate");
    let seq: i64 = row.get("seq");
    let subject: String = row.get("subject");
    let payload: serde_json::Value = row.get("envelope");
    let published_at: Option<String> = row.get("published_at_str");
    let attempts: i32 = row.get("attempts");
    let envelope: EventEnvelope = serde_json::from_value(payload)
        .map_err(|e| PgError::Query(format!("deserialize envelope: {e}")))?;
    Ok(OutboxRow {
        event_id: EventId(event_id),
        aggregate: AggregateKey(aggregate),
        seq: seq.max(0) as u64,
        subject: ArtifactRef(subject),
        envelope,
        published_at: published_at.map(Timestamp),
        attempts: attempts.max(0) as u32,
    })
}

/// Classify a sqlx error from an outbox INSERT/commit: `outbox_aggregate_seq_unique` → benign
/// seq-contention (retry); `outbox_event_id_unique` → a genuine duplicate (reject); else propagate.
fn classify_insert_error(e: sqlx::Error, event_id: &str) -> CommitAttempt {
    if let Some(db) = e.as_database_error() {
        match db.constraint() {
            Some("outbox_aggregate_seq_unique") => return CommitAttempt::SeqContention,
            Some("outbox_event_id_unique") => {
                return CommitAttempt::DuplicateEventId(event_id.to_string())
            }
            _ => {}
        }
    }
    CommitAttempt::Db(PgError::Query(e.to_string()))
}

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

    /// Validate an unpublished staged row with the same strict authority used by the elected relay
    /// before a domain transaction accepts it. This keeps malformed taxonomy, cross-tenant actor or
    /// subject authority, PII-key mismatch, wrong-region events, and oversized envelopes out of the
    /// durable outbox instead of relying on later quarantine as the first line of defence.
    pub fn validate_staged_row(
        row: &OutboxRow,
        region: &Region,
        max_envelope_bytes: usize,
    ) -> Result<(), PgError> {
        Self::validate_staged_shape(row)?;
        let config = RelayValidationConfig::new(region.clone(), max_envelope_bytes)
            .map_err(|e| PgError::Query(format!("invalid staged outbox validation config: {e}")))?;
        let payload = serde_json::to_value(&row.envelope)
            .map_err(|e| PgError::Query(format!("serialize staged outbox envelope: {e}")))?;
        validate_claimed_row(
            &ClaimedRow {
                event_id: row.event_id.0.clone(),
                aggregate: row.aggregate.0.clone(),
                seq: 0,
                subject: row.subject.0.clone(),
                payload,
            },
            &config,
        )
        .map(|_| ())
        .map_err(|error| {
            PgError::Query(format!(
                "invalid staged outbox row ({}): {}",
                error.code, error.detail
            ))
        })
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

    /// **Co-commit the outbox row INTO an already-open transaction (BUS-D4 emit-iff-committed).**
    /// A caller that owns a domain state-write transaction (e.g. the Chat Message Service writing the
    /// `message` row in `myelin_chat::store::pg`) hands its open transaction here so the
    /// `chat.message.created` outbox row lands in the SAME transaction — both commit, or both roll
    /// back. The relay owns the `outbox` table (BUS-2: the outbox + its publish are the relay's), so
    /// the INSERT lives HERE (the one sanctioned, lint-excluded outbox-write site) rather than being
    /// hand-rolled in each tenant-store caller. The per-aggregate `seq` is allocated INSIDE the
    /// transaction as `COALESCE(MAX(seq)+1, 0)` for the aggregate, guarded by the
    /// `UNIQUE(aggregate, seq)` constraint (a racing committer collides + retries → contiguous,
    /// gap-free, true-commit-order seqs — the per-conversation total-order property, contract 2.3).
    /// The outbox is keyed by `(aggregate, seq)` and drained across aggregates by the relay, so it
    /// correctly carries no per-row tenant predicate (the same relay-internal posture as
    /// [`enqueue`](Self::enqueue) — this is a relay query, not a tenant-store query).
    pub async fn co_commit_in_tx(
        conn: &mut sqlx::PgConnection,
        aggregate: &str,
        envelope: &EventEnvelope,
    ) -> Result<(), PgError> {
        Self::lock_outbox_aggregates_in_tx(conn, vec![aggregate]).await?;
        Self::insert_envelope_in_tx(conn, aggregate, envelope).await
    }

    async fn lock_outbox_aggregates_in_tx(
        conn: &mut sqlx::PgConnection,
        mut aggregates: Vec<&str>,
    ) -> Result<(), PgError> {
        aggregates.sort_unstable();
        aggregates.dedup();
        for aggregate in aggregates {
            sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
                .bind(aggregate)
                .execute(&mut *conn)
                .await
                .map_err(|e| PgError::Query(format!("lock outbox aggregate {aggregate}: {e}")))?;
        }
        Ok(())
    }

    async fn insert_envelope_in_tx(
        conn: &mut sqlx::PgConnection,
        aggregate: &str,
        envelope: &EventEnvelope,
    ) -> Result<(), PgError> {
        let payload = serde_json::to_value(envelope)
            .map_err(|e| PgError::Query(format!("serialize envelope: {e}")))?;
        let inserted = sqlx::query(
            "INSERT INTO outbox (event_id, aggregate, seq, subject, envelope) \
             VALUES ($1, $2, COALESCE((SELECT MAX(seq) + 1 FROM outbox WHERE aggregate = $2), 0), \
             $3, $4) ON CONFLICT (event_id) DO NOTHING",
        )
        .bind(&envelope.event_id.0)
        .bind(aggregate)
        .bind(&envelope.subject.0)
        .bind(&payload)
        .execute(&mut *conn)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        if inserted.rows_affected() == 0 {
            let existing =
                sqlx::query("SELECT aggregate, subject, envelope FROM outbox WHERE event_id = $1")
                    .bind(&envelope.event_id.0)
                    .fetch_one(&mut *conn)
                    .await
                    .map_err(|e| PgError::Query(format!("verify absorbed outbox row: {e}")))?;
            let stored_aggregate: String = existing
                .try_get("aggregate")
                .map_err(|e| PgError::Query(format!("decode absorbed aggregate: {e}")))?;
            let stored_subject: String = existing
                .try_get("subject")
                .map_err(|e| PgError::Query(format!("decode absorbed subject: {e}")))?;
            let stored_envelope: serde_json::Value = existing
                .try_get("envelope")
                .map_err(|e| PgError::Query(format!("decode absorbed envelope: {e}")))?;
            if stored_aggregate != aggregate
                || stored_subject != envelope.subject.0
                || stored_envelope != payload
            {
                return Err(PgError::Query(format!(
                    "outbox event_id {} already exists with divergent aggregate, subject, or envelope",
                    envelope.event_id.0
                )));
            }
        }
        Ok(())
    }

    /// Co-commit staged [`OutboxRow`]s into a caller-owned domain transaction. This is the batch
    /// form durable workflow/state stores use: it preserves the relay's in-transaction per-aggregate
    /// sequence allocation, serialises each aggregate with a transaction advisory lock so a domain
    /// transaction cannot be left aborted by an avoidable sequence race, and absorbs only an exact
    /// deterministic `event_id` replay. A divergent replay fails closed and rolls the caller's whole
    /// transaction back.
    pub async fn co_commit_rows_in_tx(
        conn: &mut sqlx::PgConnection,
        rows: &[OutboxRow],
    ) -> Result<(), PgError> {
        if rows.is_empty() {
            return Ok(());
        }

        for row in rows {
            Self::validate_staged_shape(row)?;
        }

        let aggregates = rows.iter().map(|row| row.aggregate.0.as_str()).collect();
        Self::lock_outbox_aggregates_in_tx(conn, aggregates).await?;

        for row in rows {
            Self::insert_envelope_in_tx(conn, &row.aggregate.0, &row.envelope).await?;
        }
        Ok(())
    }

    fn validate_staged_shape(row: &OutboxRow) -> Result<(), PgError> {
        if row.event_id != row.envelope.event_id
            || row.aggregate != row.envelope.aggregate
            || row.subject != row.envelope.subject
            || row.seq != 0
            || row.published_at.is_some()
            || row.attempts != 0
        {
            return Err(PgError::Query(format!(
                "staged outbox row {} does not match its canonical envelope, has a preallocated sequence, or is already published",
                row.event_id.0
            )));
        }
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
        tx.commit()
            .await
            .map_err(|e| PgError::Query(e.to_string()))?;
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
    pub async fn relay_once<P: EventPublisher + ?Sized>(
        &self,
        publisher: &P,
        batch: i64,
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

            // The relay's sanctioned broker publish (BUS-2): the row was already durably
            // committed to the outbox; the relay forwards it with the stable event_id as the
            // broker-side dedup id (0 ghost). A transport error aborts the whole tx → the claim
            // releases, rows stay unsent.
            match publisher.publish(&envelope.subject, &envelope, &envelope.event_id) {
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

        tx.commit()
            .await
            .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(published)
    }

    /// Strict elected-relay drain: validate every durable row before publication, quarantine a
    /// permanent row defect without copying its payload, and preserve per-aggregate order by
    /// blocking every later row behind the first quarantined sequence.
    ///
    /// A broker or database failure aborts this transaction. Accepted-but-uncommitted publishes are
    /// safe to replay through the stable event id, while no retry/dead-letter attempt is consumed.
    pub async fn relay_once_scoped<P: EventPublisher + ?Sized>(
        &self,
        publisher: &P,
        batch: i64,
        config: &RelayValidationConfig,
    ) -> Result<usize, PgError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| PgError::Query(e.to_string()))?;

        let rows = sqlx::query(
            "SELECT o.event_id, o.aggregate, o.seq, o.subject, o.envelope FROM outbox o \
             WHERE o.published_at IS NULL \
               AND NOT EXISTS ( \
                   SELECT 1 FROM outbox_quarantine own WHERE own.event_id = o.event_id \
               ) \
               AND NOT EXISTS ( \
                   SELECT 1 FROM outbox_quarantine prior \
                    WHERE prior.aggregate = o.aggregate AND prior.seq < o.seq \
               ) \
             ORDER BY o.aggregate, o.seq \
             FOR UPDATE OF o SKIP LOCKED LIMIT $1",
        )
        .bind(batch)
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;

        let mut blocked_aggregates = HashSet::new();
        let mut published = 0usize;
        for row in rows {
            let claimed = ClaimedRow {
                event_id: row.get("event_id"),
                aggregate: row.get("aggregate"),
                seq: row.get("seq"),
                subject: row.get("subject"),
                payload: row.get("envelope"),
            };
            if blocked_aggregates.contains(&claimed.aggregate) {
                continue;
            }

            let envelope = match validate_claimed_row(&claimed, config) {
                Ok(envelope) => envelope,
                Err(reason) => {
                    sqlx::query(
                        "INSERT INTO outbox_quarantine \
                         (event_id, aggregate, seq, reason_code, reason_detail) \
                         VALUES ($1, $2, $3, $4, $5) ON CONFLICT (event_id) DO NOTHING",
                    )
                    .bind(&claimed.event_id)
                    .bind(&claimed.aggregate)
                    .bind(claimed.seq)
                    .bind(reason.code)
                    .bind(reason.detail)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| PgError::Query(e.to_string()))?;
                    blocked_aggregates.insert(claimed.aggregate);
                    continue;
                }
            };

            match publisher.publish(&envelope.subject, &envelope, &envelope.event_id) {
                Ok(Delivery::Accepted) | Ok(Delivery::Deduplicated) => {}
                Err(e) => return Err(PgError::Publish(e.0)),
            }
            sqlx::query("UPDATE outbox SET published_at = now() WHERE event_id = $1")
                .bind(&claimed.event_id)
                .execute(&mut *tx)
                .await
                .map_err(|e| PgError::Query(e.to_string()))?;
            published += 1;
        }

        tx.commit()
            .await
            .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(published)
    }

    /// **Bounded-retry drain pass with per-row dead-lettering (MR-009b W3b.2 — the durable-arm
    /// PARITY with `myelin_events::relay::Relay::drain_once` over an in-memory store).** Like
    /// [`relay_once`](Self::relay_once) — ONE transaction, claim up to `batch` unsent rows with
    /// `FOR UPDATE SKIP LOCKED`, publish each in `(aggregate, seq)` order — but with the memory
    /// relay's exact retry/dead-letter discipline (the one genuine gap the W3b design names):
    ///
    /// - A publish **failure does NOT abort the whole pass** (unlike `relay_once`, which returns
    ///   `Err` on the first transport error and rolls the tx back). Instead the row's `attempts`
    ///   is incremented in the SAME tx; the row stays unsent (claimable next pass) — 0 lost. The
    ///   other rows of the pass still publish + mark sent.
    /// - When a row's incremented `attempts` reaches `max_attempts` it is **dead-lettered**: the
    ///   claim predicate is `attempts < max_attempts`, so a dead row is never re-claimed — it
    ///   leaves the `outbox_depth` unsent set and enters the dead-letter set (`published_at IS NULL
    ///   AND attempts >= max_attempts`), SURFACED not silently dropped (a LOUD stderr line carries
    ///   the `dlq.<tenant>.<subsystem>` routing key + the `event_id`, mirroring the in-memory
    ///   relay's `DeadLetterAlert`). Dead rows are NOT deleted — parity with the in-memory
    ///   `dead_letters()` snapshot, which retains them.
    /// - Every per-row mark (`published_at = now()` on success, `attempts + 1` on failure) commits
    ///   together at the end of the pass; a claim released only on commit (no double-claim).
    ///
    /// Returns a [`DrainReport`] counting this pass's published / deduplicated / failed /
    /// dead-lettered rows.
    ///
    /// # SHARED-TABLE HAZARD (W3b.4 verifier finding — probe-proven; BLOCKS the first production consumer)
    /// The claim predicate has **no service/subject scoping** — a drain claims EVERY unsent row in
    /// the shared `outbox` table, including rows other services committed. With multiple W3b.4
    /// service mains on one database, service B's relay can claim service A's event, publish it to
    /// B's process-LOCAL `InProcessBus` (where A's consumers are not), and permanently stamp
    /// `published_at` — the event is then invisible to A's consumers forever. TODAY this loses
    /// nothing (no production main registers a consumer; the git reconciler reads published rows
    /// too), but this MUST be fixed — subject/ownership scoping on the claim, or a shared
    /// distributed transport (NATS/EventsRuntime) — BEFORE the first production consumer or
    /// cross-service transport is wired. Tracked in the release ledger (14, W3b.4 residuals).
    pub async fn drain_once_dead_letter<B: BusTransport + ?Sized>(
        &self,
        bus: &B,
        batch: i64,
        max_attempts: u32,
    ) -> Result<DrainReport, PgError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| PgError::Query(e.to_string()))?;

        // Claim: unsent AND not-yet-dead-lettered rows, in (aggregate, seq) order, SKIP LOCKED.
        let rows = sqlx::query(
            "SELECT event_id, subject, envelope FROM outbox \
             WHERE published_at IS NULL AND attempts < $2 \
             ORDER BY aggregate, seq \
             FOR UPDATE SKIP LOCKED LIMIT $1",
        )
        .bind(batch)
        .bind(max_attempts as i32)
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;

        let mut report = DrainReport::default();
        for row in &rows {
            let event_id: String = row.get("event_id");
            let payload: serde_json::Value = row.get("envelope");
            let envelope: EventEnvelope = serde_json::from_value(payload)
                .map_err(|e| PgError::Query(format!("deserialize envelope: {e}")))?;

            // The relay's sanctioned broker publish (BUS-2): stable event_id = broker dedup id.
            match bus.put(&envelope.subject, &envelope, &envelope.event_id) {
                Ok(Delivery::Accepted) => {
                    sqlx::query("UPDATE outbox SET published_at = now() WHERE event_id = $1")
                        .bind(&event_id)
                        .execute(&mut *tx)
                        .await
                        .map_err(|e| PgError::Query(e.to_string()))?;
                    report.published += 1;
                }
                Ok(Delivery::Deduplicated) => {
                    // A re-claim after a crash mid-publish: the broker already had this id → the
                    // event WAS delivered → mark sent (no ghost, no re-delivery).
                    sqlx::query("UPDATE outbox SET published_at = now() WHERE event_id = $1")
                        .bind(&event_id)
                        .execute(&mut *tx)
                        .await
                        .map_err(|e| PgError::Query(e.to_string()))?;
                    report.deduplicated += 1;
                }
                Err(_transport_err) => {
                    // Bounded retry — bump attempts, do NOT abort the pass (0 lost: the row stays
                    // unsent + claimable). RETURNING gives the new count so we dead-letter at the
                    // bound in the SAME pass (parity with the in-memory `fail_attempt` + bound).
                    let new_attempts: i32 = sqlx::query_scalar(
                        "UPDATE outbox SET attempts = attempts + 1 WHERE event_id = $1 \
                         RETURNING attempts",
                    )
                    .bind(&event_id)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(|e| PgError::Query(e.to_string()))?;
                    if new_attempts as u32 >= max_attempts {
                        // Dead-lettered: SURFACED, never silent (the in-memory relay raises a
                        // DeadLetterAlert here; the durable arm quarantines the row in-place and
                        // logs the same routing detail loudly). The claim predicate now skips it.
                        let subsystem = envelope
                            .type_
                            .0
                            .split('.')
                            .next()
                            .filter(|s| !s.is_empty())
                            .unwrap_or("unknown");
                        eprintln!(
                            "[pg-outbox-relay] LOUD dead-letter: event_id={} \
                             dlq=dlq.{}.{} attempts={} — quarantined after the retry bound, not lost",
                            event_id, envelope.tenant.0, subsystem, new_attempts
                        );
                        report.dead_lettered += 1;
                    } else {
                        report.failed += 1;
                    }
                }
            }
        }

        tx.commit()
            .await
            .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(report)
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
    pub async fn relay_once_crash_after<P: EventPublisher + ?Sized>(
        &self,
        publisher: &P,
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

            match publisher.publish(&envelope.subject, &envelope, &envelope.event_id) {
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

    // --- The durable `OutboxStore` backing verbs (MR-009b W3b.2). ALL outbox SQL for the durable
    // `myelin_events::DurableOutboxBacking` impl (`crate::outbox_durable::PgOutboxBacking`) lives
    // HERE — the ONE sanctioned, lint-excluded, relay-INTERNAL outbox-query site (the same posture
    // as `co_commit_in_tx` / `relay_once`). The dead-letter partition is `published_at IS NULL AND
    // attempts >= MAX_PUBLISH_ATTEMPTS` (no parallel table, no schema change; `attempts` already
    // exists in the frozen shape). ---

    /// **The durable arm of `OutboxTransaction::commit`: commit every staged row in ONE atomic tx**
    /// (all-or-nothing — a partial commit would be silent data loss). Allocates the per-aggregate
    /// `seq` INSIDE the tx as `COALESCE(MAX(seq)+1, 0)` (the [`co_commit_in_tx`](Self::co_commit_in_tx)
    /// discipline; sequential inserts in the same tx each see the prior seq → contiguous). A PLAIN
    /// `INSERT` (NO `ON CONFLICT`) so a duplicate `event_id` ABORTS the tx and the call returns an
    /// error (reject parity with the in-memory arm — NOT `ON CONFLICT DO NOTHING`). A racing
    /// (`aggregate`, `seq`) collision under concurrency is retried (bounded) so the loser gets the
    /// next contiguous seq → gap-free, true-commit-order seqs (EB-03, durably).
    ///
    /// **This is the REJECT arm.** A caller that emits DETERMINISTIC ids (derived from a triggering
    /// `event_id`) and needs an idempotent crash-window re-emit uses
    /// [`commit_staged_absorb`](Self::commit_staged_absorb) instead (H1) — it `ON CONFLICT (event_id)
    /// DO NOTHING`s a byte-identical re-emit while still rejecting a divergent-payload collision.
    pub async fn commit_staged_atomic(&self, rows: &[OutboxRow]) -> Result<(), PgError> {
        if rows.is_empty() {
            return Ok(());
        }
        for _ in 0..SEQ_CONTENTION_RETRIES {
            match self.try_commit_staged(rows).await {
                CommitAttempt::Committed => return Ok(()),
                CommitAttempt::SeqContention => continue, // racing committer — retry for the next seq.
                CommitAttempt::DuplicateEventId(id) => {
                    return Err(PgError::Query(format!(
                        "outbox UNIQUE(event_id) violation on EventId(\"{id}\") — duplicate emit"
                    )))
                }
                CommitAttempt::Db(e) => return Err(e),
            }
        }
        Err(PgError::Query(
            "outbox commit_staged exhausted seq-contention retries (hot-aggregate livelock?)"
                .into(),
        ))
    }

    async fn try_commit_staged(&self, rows: &[OutboxRow]) -> CommitAttempt {
        let mut tx = match self.pool.begin().await {
            Ok(tx) => tx,
            Err(e) => return CommitAttempt::Db(PgError::Query(e.to_string())),
        };
        let aggregates = rows.iter().map(|row| row.aggregate.0.as_str()).collect();
        if let Err(error) = Self::lock_outbox_aggregates_in_tx(&mut tx, aggregates).await {
            return CommitAttempt::Db(error);
        }
        for row in rows {
            let payload = match serde_json::to_value(&row.envelope) {
                Ok(v) => v,
                Err(e) => {
                    return CommitAttempt::Db(PgError::Query(format!("serialize envelope: {e}")))
                }
            };
            // Plain INSERT (NO `ON CONFLICT`): a duplicate event_id MUST abort (reject parity), a
            // racing (aggregate, seq) collision surfaces for a retry. The seq is allocated in-tx.
            let res = sqlx::query(
                "INSERT INTO outbox (event_id, aggregate, seq, subject, envelope) \
                 VALUES ($1, $2, \
                 COALESCE((SELECT MAX(seq) + 1 FROM outbox WHERE aggregate = $2), 0), $3, $4)",
            )
            .bind(&row.event_id.0)
            .bind(&row.aggregate.0)
            .bind(&row.subject.0)
            .bind(payload)
            .execute(&mut *tx)
            .await;
            if let Err(e) = res {
                return classify_insert_error(e, &row.event_id.0);
            }
        }
        match tx.commit().await {
            Ok(()) => CommitAttempt::Committed,
            // The UNIQUE(aggregate, seq) collision can also surface at COMMIT under concurrency.
            Err(e) => classify_insert_error(e, ""),
        }
    }

    /// **The ABSORB-mode commit (H1 — peer-review #7 re-prosecution): commit staged rows idempotently,
    /// ABSORBING a DETERMINISTIC duplicate `event_id` instead of rejecting it.** Identical to
    /// [`commit_staged_atomic`](Self::commit_staged_atomic) EXCEPT a re-inserted `event_id` is
    /// `ON CONFLICT (event_id) DO NOTHING` (absorbed, not `Err`) — AFTER verifying the stored
    /// envelope is byte-identical to the re-emitted one (a divergent payload under the same id is a
    /// GENUINE collision and is still rejected, preserving the reject-parity safety the plain INSERT
    /// gave). This is the path the CI dispatcher's DETERMINISTIC co-emitted `ci.run.started` /
    /// `ci.check.updated` (ids derived from the run + subject) take: a crash-window redelivery re-runs
    /// the handler and re-emits the SAME ids; without absorb-mode `commit_staged_atomic` returns
    /// `Err("duplicate emit")` → the handler returns `Retry` → the message NEVER acks → an UNBOUNDED
    /// LIVELOCK (H1). Absorbing the deterministic re-emit lets the redelivery return `Done`, the
    /// consumer-runtime dedup mark commits, and the events stay present exactly once.
    ///
    /// The per-aggregate seq is allocated in-tx exactly as the reject arm; a `DO NOTHING` conflict
    /// consumes NO seq (the row is not inserted), so gap-freeness across true inserts is preserved. A
    /// racing `(aggregate, seq)` collision is retried (bounded) the same way.
    pub async fn commit_staged_absorb(&self, rows: &[OutboxRow]) -> Result<(), PgError> {
        if rows.is_empty() {
            return Ok(());
        }
        for _ in 0..SEQ_CONTENTION_RETRIES {
            match self.try_commit_staged_absorb(rows).await {
                CommitAttempt::Committed => return Ok(()),
                CommitAttempt::SeqContention => continue,
                CommitAttempt::DuplicateEventId(id) => {
                    // Here a "DuplicateEventId" means a DIVERGENT payload under an already-present id —
                    // a genuine collision, NOT the benign deterministic re-emit (which was absorbed).
                    return Err(PgError::Query(format!(
                        "outbox event_id {id} already present with a DIFFERENT payload — a genuine \
                         collision (absorb-mode verifies payload equality; a deterministic re-emit is \
                         byte-identical and is absorbed, this is not)"
                    )));
                }
                CommitAttempt::Db(e) => return Err(e),
            }
        }
        Err(PgError::Query(
            "outbox commit_staged_absorb exhausted seq-contention retries (hot-aggregate livelock?)"
                .into(),
        ))
    }

    async fn try_commit_staged_absorb(&self, rows: &[OutboxRow]) -> CommitAttempt {
        let mut tx = match self.pool.begin().await {
            Ok(tx) => tx,
            Err(e) => return CommitAttempt::Db(PgError::Query(e.to_string())),
        };
        let aggregates = rows.iter().map(|row| row.aggregate.0.as_str()).collect();
        if let Err(error) = Self::lock_outbox_aggregates_in_tx(&mut tx, aggregates).await {
            return CommitAttempt::Db(error);
        }
        for row in rows {
            let payload = match serde_json::to_value(&row.envelope) {
                Ok(v) => v,
                Err(e) => {
                    return CommitAttempt::Db(PgError::Query(format!("serialize envelope: {e}")))
                }
            };
            // ON CONFLICT (event_id) DO NOTHING: a deterministic re-emit is ABSORBED (rows_affected
            // == 0); a genuine seq collision still surfaces on the (aggregate, seq) unique for a retry.
            let res = sqlx::query(
                "INSERT INTO outbox (event_id, aggregate, seq, subject, envelope) \
                 VALUES ($1, $2, \
                 COALESCE((SELECT MAX(seq) + 1 FROM outbox WHERE aggregate = $2), 0), $3, $4) \
                 ON CONFLICT (event_id) DO NOTHING",
            )
            .bind(&row.event_id.0)
            .bind(&row.aggregate.0)
            .bind(&row.subject.0)
            .bind(&payload)
            .execute(&mut *tx)
            .await;
            let res = match res {
                Ok(r) => r,
                Err(e) => return classify_insert_error(e, &row.event_id.0),
            };
            if res.rows_affected() == 0 {
                // Absorbed (the id was already present). VERIFY payload equality WITHIN the tx: a
                // divergent payload under the same id is a genuine collision → reject (reject-parity).
                let existing: Result<serde_json::Value, sqlx::Error> =
                    sqlx::query_scalar("SELECT envelope FROM outbox WHERE event_id = $1")
                        .bind(&row.event_id.0)
                        .fetch_one(&mut *tx)
                        .await;
                match existing {
                    Ok(stored) if stored == payload => { /* byte-identical deterministic re-emit — absorb. */
                    }
                    Ok(_) => return CommitAttempt::DuplicateEventId(row.event_id.0.clone()),
                    Err(e) => return CommitAttempt::Db(PgError::Query(e.to_string())),
                }
            }
        }
        match tx.commit().await {
            Ok(()) => CommitAttempt::Committed,
            Err(e) => classify_insert_error(e, ""),
        }
    }

    /// `outbox_depth` survival signal for the durable backing: unsent AND not-yet-dead rows
    /// (`published_at IS NULL AND attempts < MAX_PUBLISH_ATTEMPTS`).
    pub async fn unsent_depth(&self) -> Result<i64, PgError> {
        sqlx::query_scalar(
            "SELECT count(*) FROM outbox WHERE published_at IS NULL AND attempts < $1",
        )
        .bind(MAX_PUBLISH_ATTEMPTS as i32)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| PgError::Query(e.to_string()))
    }

    /// The dead-letter count: unsent rows that exhausted the retry bound
    /// (`published_at IS NULL AND attempts >= MAX_PUBLISH_ATTEMPTS`).
    pub async fn dead_count(&self) -> Result<i64, PgError> {
        sqlx::query_scalar(
            "SELECT count(*) FROM outbox WHERE published_at IS NULL AND attempts >= $1",
        )
        .bind(MAX_PUBLISH_ATTEMPTS as i32)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| PgError::Query(e.to_string()))
    }

    /// The `recorded_at` of the oldest still-unsent (not-yet-dead) row — the outbox-age anchor.
    pub async fn oldest_unsent_recorded_at(&self) -> Result<Option<String>, PgError> {
        sqlx::query_scalar(
            "SELECT envelope ->> 'recorded_at' FROM outbox \
             WHERE published_at IS NULL AND attempts < $1 \
             ORDER BY envelope ->> 'recorded_at' ASC, aggregate ASC, seq ASC LIMIT 1",
        )
        .bind(MAX_PUBLISH_ATTEMPTS as i32)
        .fetch_optional(&self.pool)
        .await
        .map(Option::flatten)
        .map_err(|e| PgError::Query(e.to_string()))
    }

    /// The committed-and-live count (sent + unsent-retrying; NOT dead-lettered) — mirrors the
    /// in-memory `order.len()`, which drops a row on dead-letter.
    pub async fn committed_live_count(&self) -> Result<i64, PgError> {
        sqlx::query_scalar(
            "SELECT count(*) FROM outbox WHERE NOT (published_at IS NULL AND attempts >= $1)",
        )
        .bind(MAX_PUBLISH_ATTEMPTS as i32)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| PgError::Query(e.to_string()))
    }

    /// Read a committed-and-live row by `event_id` (a dead-lettered row reads as absent — parity
    /// with the in-memory `row()`, which reads the live map a dead row was removed from).
    pub async fn committed_row(&self, id: &EventId) -> Result<Option<OutboxRow>, PgError> {
        let row = sqlx::query(&format!(
            "SELECT {ROW_PROJECTION} FROM outbox \
             WHERE event_id = $1 AND NOT (published_at IS NULL AND attempts >= $2)"
        ))
        .bind(&id.0)
        .bind(MAX_PUBLISH_ATTEMPTS as i32)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        row.as_ref().map(row_from_pg).transpose()
    }

    /// Snapshot the committed-and-live rows in per-aggregate COMMIT order (`(aggregate, seq)` —
    /// the order the trait contract promises). NOTE (W3b.2 verifier finding): `event_id` mint
    /// order does NOT track commit order — a tx minted earlier can commit later — so ordering by
    /// event_id would violate the `committed_rows` contract; `seq` is allocated inside the commit
    /// tx and is the true per-aggregate commit sequence.
    pub async fn committed_live_rows(&self) -> Result<Vec<OutboxRow>, PgError> {
        let rows = sqlx::query(&format!(
            "SELECT {ROW_PROJECTION} FROM outbox \
             WHERE NOT (published_at IS NULL AND attempts >= $1) ORDER BY aggregate ASC, seq ASC"
        ))
        .bind(MAX_PUBLISH_ATTEMPTS as i32)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        rows.iter().map(row_from_pg).collect()
    }

    /// Snapshot every retained outbox witness, including terminal dead letters. This query is the
    /// load-bearing recovery view; unlike `committed_live_rows` it deliberately has no live-set
    /// predicate, while preserving the same per-aggregate commit ordering.
    pub async fn retained_rows(&self) -> Result<Vec<OutboxRow>, PgError> {
        let rows = sqlx::query(&format!(
            "SELECT {ROW_PROJECTION} FROM outbox ORDER BY aggregate ASC, seq ASC"
        ))
        .fetch_all(&self.pool)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        rows.iter().map(row_from_pg).collect()
    }

    /// Snapshot the dead-lettered rows (retained, not deleted — parity with the in-memory
    /// `dead_letters()` snapshot).
    pub async fn dead_rows(&self) -> Result<Vec<OutboxRow>, PgError> {
        let rows = sqlx::query(&format!(
            "SELECT {ROW_PROJECTION} FROM outbox \
             WHERE published_at IS NULL AND attempts >= $1 ORDER BY aggregate ASC, seq ASC"
        ))
        .bind(MAX_PUBLISH_ATTEMPTS as i32)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        rows.iter().map(row_from_pg).collect()
    }
}

#[cfg(test)]
mod validation_config_tests {
    use myelin_events::{Actor, CorrelationId, DataRole, EventType, PiiKeyRef, Visibility};
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_tenancy::TenantId;

    use super::*;

    fn envelope() -> EventEnvelope {
        EventEnvelope {
            event_id: EventId("unit-event".into()),
            type_: EventType("issue.issue.updated".into()),
            schema_ver: 1,
            tenant: TenantId("acme".into()),
            region: Region("no-osl".into()),
            actor: Actor(Principal::stub(
                PrincipalId("relay-unit".into()),
                PrincipalKind::Service,
                TenantId("acme".into()),
            )),
            subject: ArtifactRef("myelin://acme/issue/issue/one".into()),
            aggregate: AggregateKey("issue:one".into()),
            causation_id: None,
            correlation_id: CorrelationId("unit-event".into()),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-07-18T00:00:00Z".into()),
            recorded_at: Timestamp("2026-07-18T00:00:01Z".into()),
            payload: serde_json::json!({}),
        }
    }

    fn claimed(envelope: &EventEnvelope) -> ClaimedRow {
        ClaimedRow {
            event_id: envelope.event_id.0.clone(),
            aggregate: envelope.aggregate.0.clone(),
            seq: 0,
            subject: envelope.subject.0.clone(),
            payload: serde_json::to_value(envelope).expect("serialize unit envelope"),
        }
    }

    fn validation() -> RelayValidationConfig {
        RelayValidationConfig::new(Region("no-osl".into()), 64 * 1024).expect("valid scope")
    }

    fn reason(envelope: &EventEnvelope) -> &'static str {
        validate_claimed_row(&claimed(envelope), &validation())
            .expect_err("invalid envelope")
            .code
    }

    #[test]
    fn strict_relay_scope_requires_region_and_a_real_finite_limit() {
        assert_eq!(
            RelayValidationConfig::new(Region(" ".into()), 1024),
            Err(RelayValidationConfigError::EmptyRegion)
        );
        assert_eq!(
            RelayValidationConfig::new(Region("no-osl".into()), 0),
            Err(RelayValidationConfigError::InvalidEnvelopeLimit {
                max: MAX_CONFIGURED_ENVELOPE_BYTES
            })
        );
        assert_eq!(
            RelayValidationConfig::new(Region("no-osl".into()), MAX_CONFIGURED_ENVELOPE_BYTES + 1,),
            Err(RelayValidationConfigError::InvalidEnvelopeLimit {
                max: MAX_CONFIGURED_ENVELOPE_BYTES
            })
        );
        assert!(RelayValidationConfig::new(Region("no-osl".into()), 256 * 1024).is_ok());
    }

    #[test]
    fn strict_validation_uses_typed_subject_and_pii_key_authorities() {
        let mut valid_pii = envelope();
        valid_pii.contains_personal_data = true;
        valid_pii.pii_key_ref = Some(PiiKeyRef("kms://acme/0/subject:u42".into()));
        assert!(validate_claimed_row(&claimed(&valid_pii), &validation()).is_ok());

        let mut false_with_key = envelope();
        false_with_key.pii_key_ref = Some(PiiKeyRef("kms://acme/0/tenant".into()));
        assert_eq!(reason(&false_with_key), "pii_presence_mismatch");

        let mut true_without_key = envelope();
        true_without_key.contains_personal_data = true;
        assert_eq!(reason(&true_without_key), "pii_presence_mismatch");

        let mut malformed_key = valid_pii.clone();
        malformed_key.pii_key_ref = Some(PiiKeyRef("kms://acme/0/subject:u42/extra".into()));
        assert_eq!(reason(&malformed_key), "invalid_pii_key_ref");

        let mut cross_tenant_key = valid_pii;
        cross_tenant_key.pii_key_ref = Some(PiiKeyRef("kms://foreign/0/tenant".into()));
        assert_eq!(reason(&cross_tenant_key), "pii_key_tenant_mismatch");

        let mut malformed_subject = envelope();
        malformed_subject.subject = ArtifactRef("https://acme/issue/issue/one".into());
        assert_eq!(reason(&malformed_subject), "invalid_artifact_ref");

        let mut noncanonical_subject = envelope();
        noncanonical_subject.subject = ArtifactRef("myelin://acme/issue/issue/one#step-01".into());
        assert_eq!(reason(&noncanonical_subject), "invalid_artifact_ref");

        let mut cross_tenant_subject = envelope();
        cross_tenant_subject.subject = ArtifactRef("myelin://foreign/issue/issue/one".into());
        assert_eq!(reason(&cross_tenant_subject), "subject_tenant_mismatch");
    }

    #[test]
    fn schema_versions_are_one_based() {
        let mut zero = envelope();
        zero.schema_ver = 0;
        assert_eq!(reason(&zero), "invalid_schema_version");
    }

    #[test]
    fn staged_rows_cannot_bypass_in_transaction_sequence_allocation() {
        let envelope = envelope();
        let mut row = OutboxRow {
            event_id: envelope.event_id.clone(),
            aggregate: envelope.aggregate.clone(),
            seq: 0,
            subject: envelope.subject.clone(),
            envelope,
            published_at: None,
            attempts: 0,
        };
        assert!(PgRelay::validate_staged_shape(&row).is_ok());

        row.seq = 7;
        let error = PgRelay::validate_staged_shape(&row)
            .expect_err("a staged caller must not preallocate the relay sequence");
        assert!(error.to_string().contains("preallocated sequence"));
    }
}
