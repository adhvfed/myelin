use myelin_events::relay::{
    BusTransport, Delivery, DrainReport, EventPublisher, MAX_PUBLISH_ATTEMPTS,
};
use myelin_events::{
    validate_event_type, AggregateKey, ArtifactRef, EventEnvelope, EventId, OutboxRow, Region,
    StreamSubject, Timestamp,
};
use sqlx::postgres::PgPool;
use sqlx::{Postgres, Row, Transaction};
use std::collections::HashSet;

use crate::kms::PiiKeyRef as KmsPiiKeyRef;
use crate::pg::PgError;

const SEQ_CONTENTION_RETRIES: u32 = 128;

pub const MAX_CONFIGURED_ENVELOPE_BYTES: usize = 16 * 1024 * 1024;

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
    if envelope.type_.0.starts_with("signal.") || envelope.type_.0 == "notif.signal.snapshot" {
        validate_signal_subject(&envelope.subject.0, &envelope.tenant)?;
    } else {
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

fn validate_signal_subject(
    subject: &str,
    tenant: &myelin_tenancy::TenantId,
) -> Result<(), PermanentRowError> {
    let prefix = format!("sig.{}.", tenant.0);
    let Some(route) = subject.strip_prefix(&prefix) else {
        return Err(PermanentRowError::new(
            "invalid_signal_subject",
            "signal subject is outside the envelope tenant",
        ));
    };
    let mut tokens = route.split('.');
    let severity = tokens.next().unwrap_or_default();
    let valid_severity = matches!(
        severity,
        "info" | "notice" | "warning" | "error" | "critical"
    );
    let rules = tokens.collect::<Vec<_>>();
    let valid_rule = !rules.is_empty()
        && rules.iter().all(|token| {
            let mut chars = token.chars();
            matches!(chars.next(), Some(first) if first.is_ascii_lowercase())
                && chars.all(|character| {
                    character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
                })
        });
    if !valid_severity || !valid_rule || subject.len() > 512 {
        return Err(PermanentRowError::new(
            "invalid_signal_subject",
            "signal subject does not match sig.<tenant>.<severity>.<rule>",
        ));
    }
    Ok(())
}

const ROW_PROJECTION: &str = "event_id, aggregate, seq, subject, envelope, attempts, \
     to_char(published_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS published_at_str";

enum CommitAttempt {
    Committed,
    SeqContention,
    DuplicateEventId(String),
    Db(PgError),
}

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

#[derive(Clone)]
pub struct PgRelay {
    pool: PgPool,
}

impl PgRelay {
    pub fn new(pool: PgPool) -> PgRelay {
        PgRelay { pool }
    }

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
        sqlx::query(&format!(
            "INSERT INTO {state_table} (id, event_id) VALUES ($1, $2) ON CONFLICT (id) DO NOTHING"
        ))
        .bind(state_id)
        .bind(&envelope.event_id.0)
        .execute(&mut *tx)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
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

        let published = self
            .relay_once_scoped_in_tx(&mut tx, publisher, batch, config)
            .await?;

        tx.commit()
            .await
            .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(published)
    }

    pub(crate) async fn relay_once_scoped_in_tx<P: EventPublisher + ?Sized>(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        publisher: &P,
        batch: i64,
        config: &RelayValidationConfig,
    ) -> Result<usize, PgError> {
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
        .fetch_all(&mut **tx)
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
                    .execute(&mut **tx)
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
                .execute(&mut **tx)
                .await
                .map_err(|e| PgError::Query(e.to_string()))?;
            published += 1;
        }

        Ok(published)
    }

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
                    sqlx::query("UPDATE outbox SET published_at = now() WHERE event_id = $1")
                        .bind(&event_id)
                        .execute(&mut *tx)
                        .await
                        .map_err(|e| PgError::Query(e.to_string()))?;
                    report.deduplicated += 1;
                }
                Err(_transport_err) => {
                    let new_attempts: i32 = sqlx::query_scalar(
                        "UPDATE outbox SET attempts = attempts + 1 WHERE event_id = $1 \
                         RETURNING attempts",
                    )
                    .bind(&event_id)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(|e| PgError::Query(e.to_string()))?;
                    if new_attempts as u32 >= max_attempts {
                        let subsystem = envelope
                            .type_
                            .0
                            .split('.')
                            .next()
                            .filter(|s| !s.is_empty())
                            .unwrap_or("unknown");
                        eprintln!(
                            "[pg-outbox-relay] LOUD dead-letter: event_id={} \
                             dlq=dlq.{}.{} attempts={} - quarantined after the retry bound, not lost",
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

            if published >= crash_after {
                drop(tx);
                return Ok(published);
            }

            sqlx::query("UPDATE outbox SET published_at = now() WHERE event_id = $1")
                .bind(&event_id)
                .execute(&mut *tx)
                .await
                .map_err(|e| PgError::Query(e.to_string()))?;
        }

        drop(tx);
        Ok(published)
    }

    pub async fn outbox_depth(&self) -> Result<i64, PgError> {
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM outbox WHERE published_at IS NULL")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(n)
    }

    pub async fn commit_staged_atomic(&self, rows: &[OutboxRow]) -> Result<(), PgError> {
        if rows.is_empty() {
            return Ok(());
        }
        for _ in 0..SEQ_CONTENTION_RETRIES {
            match self.try_commit_staged(rows).await {
                CommitAttempt::Committed => return Ok(()),
                CommitAttempt::SeqContention => continue,
                CommitAttempt::DuplicateEventId(id) => {
                    return Err(PgError::Query(format!(
                        "outbox UNIQUE(event_id) violation on EventId(\"{id}\") - duplicate emit"
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
            Err(e) => classify_insert_error(e, ""),
        }
    }

    pub async fn commit_staged_absorb(&self, rows: &[OutboxRow]) -> Result<(), PgError> {
        if rows.is_empty() {
            return Ok(());
        }
        for _ in 0..SEQ_CONTENTION_RETRIES {
            match self.try_commit_staged_absorb(rows).await {
                CommitAttempt::Committed => return Ok(()),
                CommitAttempt::SeqContention => continue,
                CommitAttempt::DuplicateEventId(id) => {
                    return Err(PgError::Query(format!(
                        "outbox event_id {id} already present with a DIFFERENT payload - a genuine \
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
                let existing: Result<serde_json::Value, sqlx::Error> =
                    sqlx::query_scalar("SELECT envelope FROM outbox WHERE event_id = $1")
                        .bind(&row.event_id.0)
                        .fetch_one(&mut *tx)
                        .await;
                match existing {
                    Ok(stored) if stored == payload => {
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

    pub async fn unsent_depth(&self) -> Result<i64, PgError> {
        sqlx::query_scalar(
            "SELECT count(*) FROM outbox WHERE published_at IS NULL AND attempts < $1",
        )
        .bind(MAX_PUBLISH_ATTEMPTS as i32)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| PgError::Query(e.to_string()))
    }

    pub async fn dead_count(&self) -> Result<i64, PgError> {
        sqlx::query_scalar(
            "SELECT count(*) FROM outbox WHERE published_at IS NULL AND attempts >= $1",
        )
        .bind(MAX_PUBLISH_ATTEMPTS as i32)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| PgError::Query(e.to_string()))
    }

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

    pub async fn committed_live_count(&self) -> Result<i64, PgError> {
        sqlx::query_scalar(
            "SELECT count(*) FROM outbox WHERE NOT (published_at IS NULL AND attempts >= $1)",
        )
        .bind(MAX_PUBLISH_ATTEMPTS as i32)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| PgError::Query(e.to_string()))
    }

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

    pub async fn retained_rows(&self) -> Result<Vec<OutboxRow>, PgError> {
        let rows = sqlx::query(&format!(
            "SELECT {ROW_PROJECTION} FROM outbox ORDER BY aggregate ASC, seq ASC"
        ))
        .fetch_all(&self.pool)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        rows.iter().map(row_from_pg).collect()
    }

    pub async fn retained_rows_bounded(
        &self,
        maximum_rows: usize,
        maximum_envelope_bytes: usize,
    ) -> Result<Vec<OutboxRow>, PgError> {
        let fetch_limit = i64::try_from(maximum_rows.saturating_add(1)).unwrap_or(i64::MAX);
        let byte_limit = i64::try_from(maximum_envelope_bytes).unwrap_or(i64::MAX);
        let rows = sqlx::query(
            "WITH retained AS (
               SELECT event_id,aggregate,seq,subject,envelope,attempts,published_at,
                      pg_column_size(envelope)::bigint AS envelope_bytes
                 FROM outbox ORDER BY aggregate ASC,seq ASC LIMIT $1
             ), measured AS (
               SELECT event_id,aggregate,seq,subject,envelope,attempts,published_at,
                      sum(envelope_bytes) OVER (
                        ORDER BY aggregate ASC,seq ASC
                      )::bigint AS aggregate_bytes
                 FROM retained
             )
             SELECT event_id,aggregate,seq,subject,
                    CASE WHEN aggregate_bytes <= $2 THEN envelope ELSE NULL END AS envelope,
                    attempts,
                    to_char(published_at AT TIME ZONE 'UTC',
                            'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS published_at_str,
                    aggregate_bytes
               FROM measured ORDER BY aggregate ASC,seq ASC",
        )
        .bind(fetch_limit)
        .bind(byte_limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        if rows.len() > maximum_rows {
            return Err(PgError::Query(
                "retained outbox snapshot exceeds its row limit".into(),
            ));
        }
        let mut decoded = Vec::with_capacity(rows.len());
        for row in &rows {
            let aggregate_bytes: i64 = row
                .try_get("aggregate_bytes")
                .map_err(|e| PgError::Query(e.to_string()))?;
            if aggregate_bytes > byte_limit {
                return Err(PgError::Query(
                    "retained outbox snapshot exceeds its envelope byte limit".into(),
                ));
            }
            decoded.push(row_from_pg(row)?);
        }
        Ok(decoded)
    }

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
    fn strict_validation_admits_only_tenant_bound_canonical_signal_subjects() {
        let mut signal = envelope();
        signal.type_ = EventType("signal.opened".into());
        signal.subject = ArtifactRef("sig.acme.notice.git.review_requested".into());
        signal.aggregate = AggregateKey("signal:review-one".into());
        assert!(validate_claimed_row(&claimed(&signal), &validation()).is_ok());

        for subject in [
            "sig.other.notice.git.review_requested",
            "sig.acme.urgent.git.review_requested",
            "sig.acme.notice.Git.review_requested",
            "sig.acme.notice",
            "sig.acme.notice.git.*",
        ] {
            signal.subject = ArtifactRef(subject.into());
            assert_eq!(reason(&signal), "invalid_signal_subject", "{subject}");
        }
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
