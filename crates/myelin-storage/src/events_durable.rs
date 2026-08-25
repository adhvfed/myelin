use sqlx::postgres::PgPool;

use myelin_events::{
    BrokerDeliveryRef, CoCommitError, CoCommitTx, ConsumerName, DeadLetterRecord, DedupError,
    DedupResult, DeliveryQuarantineReason, DurableBusErasure, DurableDeadLetter, DurableDedup,
    DurableDeliveryQuarantine, ErasedSubject, EventId, PiiKeyRef, Region, TenantId, Timestamp,
};

use crate::migration::{Migration, Migrations};

#[derive(Clone)]
pub struct DurableDedupBacking {
    pool: PgPool,
    rt: tokio::runtime::Handle,
}

impl DurableDedupBacking {
    pub fn new(pool: PgPool, rt: tokio::runtime::Handle) -> DurableDedupBacking {
        DurableDedupBacking { pool, rt }
    }

    fn block<F: std::future::Future>(&self, fut: F) -> F::Output {
        tokio::task::block_in_place(|| self.rt.block_on(fut))
    }
}

impl DurableDedup for DurableDedupBacking {
    fn mark_handled(&self, consumer: &ConsumerName, event_id: &EventId) -> DedupResult<bool> {
        self.block(async {
            sqlx::query(
                "INSERT INTO consumer_dedup (consumer, event_id) VALUES ($1, $2) \
                 ON CONFLICT (consumer, event_id) DO NOTHING",
            )
            .bind(&consumer.0)
            .bind(&event_id.0)
            .execute(&self.pool)
            .await
            .map(|res| res.rows_affected() == 1)
            .map_err(|_| DedupError::Unavailable)
        })
    }

    fn is_handled(&self, consumer: &ConsumerName, event_id: &EventId) -> DedupResult<bool> {
        self.block(async {
            sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM consumer_dedup WHERE consumer = $1 AND event_id = $2)",
            )
            .bind(&consumer.0)
            .bind(&event_id.0)
            .fetch_one(&self.pool)
            .await
            .map_err(|_| DedupError::Unavailable)
        })
    }

    fn revert(&self, consumer: &ConsumerName, event_id: &EventId) -> DedupResult<()> {
        self.block(async {
            sqlx::query("DELETE FROM consumer_dedup WHERE consumer = $1 AND event_id = $2")
                .bind(&consumer.0)
                .bind(&event_id.0)
                .execute(&self.pool)
                .await
                .map(|_| ())
                .map_err(|_| DedupError::Unavailable)
        })
    }

    fn forget(&self, consumer: &ConsumerName, event_id: &EventId) -> DedupResult<bool> {
        self.block(async {
            sqlx::query("DELETE FROM consumer_dedup WHERE consumer = $1 AND event_id = $2")
                .bind(&consumer.0)
                .bind(&event_id.0)
                .execute(&self.pool)
                .await
                .map(|res| res.rows_affected() > 0)
                .map_err(|_| DedupError::Unavailable)
        })
    }

    fn begin_co_commit(
        &self,
        consumer: &ConsumerName,
        event_id: &EventId,
        tenant: &TenantId,
        region: &Region,
    ) -> DedupResult<(Box<dyn CoCommitTx>, bool)> {
        let acquired: Result<(sqlx::Transaction<'static, sqlx::Postgres>, bool), sqlx::Error> =
            self.block(async {
                let mut tx = self.pool.begin().await?;
                sqlx::query(
                    "SELECT set_config('myelin.tenant_id', $1, true), \
                            set_config('myelin.region', $2, true)",
                )
                .bind(&tenant.0)
                .bind(&region.0)
                .execute(&mut *tx)
                .await?;
                let res = sqlx::query(
                    "INSERT INTO consumer_dedup (consumer, event_id) VALUES ($1, $2) \
                     ON CONFLICT (consumer, event_id) DO NOTHING",
                )
                .bind(&consumer.0)
                .bind(&event_id.0)
                .execute(&mut *tx)
                .await?;
                Ok((tx, res.rows_affected() == 1))
            });
        acquired
            .map(|(tx, fresh)| {
                (
                    Box::new(DurableCoCommit {
                        tx: Some(tx),
                        rt: self.rt.clone(),
                    }) as Box<dyn CoCommitTx>,
                    fresh,
                )
            })
            .map_err(|_| DedupError::Unavailable)
    }
}

struct DurableCoCommit {
    tx: Option<sqlx::Transaction<'static, sqlx::Postgres>>,
    rt: tokio::runtime::Handle,
}

impl DurableCoCommit {
    fn block<F: std::future::Future>(&self, fut: F) -> F::Output {
        tokio::task::block_in_place(|| self.rt.block_on(fut))
    }
}

impl CoCommitTx for DurableCoCommit {
    fn connection(&mut self) -> Option<&mut dyn core::any::Any> {
        self.tx
            .as_mut()
            .map(|t| (&mut **t) as &mut dyn core::any::Any)
    }

    fn commit(mut self: Box<Self>) -> Result<(), CoCommitError> {
        let Some(tx) = self.tx.take() else {
            return Ok(());
        };
        self.block(async { tx.commit().await })
            .map(|_| ())
            .map_err(|e| CoCommitError(e.to_string()))
    }

    fn rollback(mut self: Box<Self>) {
        if let Some(tx) = self.tx.take() {
            let _ = self.block(async { tx.rollback().await });
        }
    }
}

pub fn consumer_dead_letter_migrations() -> Migrations {
    Migrations::of([Migration::plain(
        "0002_consumer_dead_letter",
        myelin_events::CONSUMER_DEAD_LETTER_MIGRATION,
    )])
}

#[derive(Clone)]
pub struct DurableDeadLetterBacking {
    pool: PgPool,
    rt: tokio::runtime::Handle,
}

impl DurableDeadLetterBacking {
    pub fn new(pool: PgPool, rt: tokio::runtime::Handle) -> DurableDeadLetterBacking {
        DurableDeadLetterBacking { pool, rt }
    }

    fn block<F: std::future::Future>(&self, fut: F) -> F::Output {
        tokio::task::block_in_place(|| self.rt.block_on(fut))
    }
}

impl DurableDeadLetter for DurableDeadLetterBacking {
    fn record(
        &self,
        consumer: &ConsumerName,
        event_id: &EventId,
        reason: &str,
    ) -> Result<(), String> {
        self.block(async {
            sqlx::query(
                "INSERT INTO consumer_dead_letter (consumer, event_id, reason) \
                 VALUES ($1, $2, $3) \
                 ON CONFLICT (consumer, event_id) DO NOTHING",
            )
            .bind(&consumer.0)
            .bind(&event_id.0)
            .bind(reason)
            .execute(&self.pool)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
        })
    }

    fn dead_letters(&self, consumer: &ConsumerName) -> Vec<DeadLetterRecord> {
        self.block(async {
            let rows: Vec<(String, String)> = match sqlx::query_as(
                "SELECT event_id, reason FROM consumer_dead_letter \
                 WHERE consumer = $1 ORDER BY occurred_at, event_id",
            )
            .bind(&consumer.0)
            .fetch_all(&self.pool)
            .await
            {
                Ok(rows) => rows,
                Err(e) => {
                    eprintln!(
                        "[consumer-dlq] LOUD: durable dead_letters read failed for consumer={}: {e}",
                        consumer.0
                    );
                    return Vec::new();
                }
            };
            rows.into_iter()
                .map(|(event_id, reason)| DeadLetterRecord {
                    consumer: consumer.clone(),
                    event_id: EventId(event_id),
                    reason,
                })
                .collect()
        })
    }
}

pub fn consumer_delivery_quarantine_migrations() -> Migrations {
    Migrations::of([Migration::plain(
        "0004_consumer_delivery_quarantine",
        myelin_events::CONSUMER_DELIVERY_QUARANTINE_MIGRATION,
    )])
}

#[derive(Clone)]
pub struct DurableDeliveryQuarantineBacking {
    pool: PgPool,
    rt: tokio::runtime::Handle,
}

impl DurableDeliveryQuarantineBacking {
    pub fn new(pool: PgPool, rt: tokio::runtime::Handle) -> Self {
        Self { pool, rt }
    }

    fn block<F: std::future::Future>(&self, fut: F) -> F::Output {
        tokio::task::block_in_place(|| self.rt.block_on(fut))
    }
}

impl DurableDeliveryQuarantine for DurableDeliveryQuarantineBacking {
    fn record(
        &self,
        consumer: &str,
        broker_ref: &BrokerDeliveryRef,
        reason: DeliveryQuarantineReason,
        delivery_attempt: u64,
    ) -> Result<(), String> {
        let stream_sequence = i64::try_from(broker_ref.stream_sequence)
            .map_err(|_| "stream sequence exceeds durable range".to_string())?;
        let delivery_attempt = i64::try_from(delivery_attempt)
            .map_err(|_| "delivery attempt exceeds durable range".to_string())?;
        if consumer.is_empty()
            || broker_ref.stream.is_empty()
            || stream_sequence <= 0
            || delivery_attempt <= 0
        {
            return Err("invalid delivery quarantine reference".into());
        }
        self.block(async {
            sqlx::query(
                "INSERT INTO consumer_delivery_quarantine \
                 (consumer, stream, stream_sequence, reason_code, delivery_attempt) \
                 VALUES ($1, $2, $3, $4, $5) \
                 ON CONFLICT (consumer, stream, stream_sequence) DO NOTHING",
            )
            .bind(consumer)
            .bind(&broker_ref.stream)
            .bind(stream_sequence)
            .bind(reason.code())
            .bind(delivery_attempt)
            .execute(&self.pool)
            .await
            .map(|_| ())
            .map_err(|_| "delivery quarantine write failed".to_string())
        })
    }
}

pub const BUS_ERASURE_LEDGER_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS bus_erasure_ledger (
    tenant    text   NOT NULL,
    region    text   NOT NULL,
    subject   text   NOT NULL,
    key_refs  text[] NOT NULL DEFAULT '{}',
    erased_at text   NOT NULL,
    PRIMARY KEY (tenant, region, subject)
);";

pub fn bus_erasure_durable_migrations() -> Migrations {
    Migrations::of([Migration::plain(
        "0053_bus_erasure_ledger",
        BUS_ERASURE_LEDGER_MIGRATION,
    )])
}

#[derive(Clone)]
pub struct DurableBusErasureBacking {
    pool: PgPool,
    rt: tokio::runtime::Handle,
}

impl DurableBusErasureBacking {
    pub fn new(pool: PgPool, rt: tokio::runtime::Handle) -> DurableBusErasureBacking {
        DurableBusErasureBacking { pool, rt }
    }

    fn block<F: std::future::Future>(&self, fut: F) -> F::Output {
        tokio::task::block_in_place(|| self.rt.block_on(fut))
    }
}

impl DurableBusErasure for DurableBusErasureBacking {
    fn record(
        &self,
        tenant: &TenantId,
        region: &Region,
        subject: &str,
        key_refs: &[PiiKeyRef],
        erased_at: &Timestamp,
    ) -> Result<(), String> {
        let mut refs: Vec<String> = key_refs.iter().map(|k| k.0.clone()).collect();
        refs.sort();
        refs.dedup();
        self.block(async {
            sqlx::query(
                "INSERT INTO bus_erasure_ledger (tenant, region, subject, key_refs, erased_at) \
                 VALUES ($1, $2, $3, $4, $5) \
                 ON CONFLICT (tenant, region, subject) DO UPDATE SET \
                   key_refs = ( \
                     SELECT array( \
                       SELECT DISTINCT r \
                       FROM unnest(bus_erasure_ledger.key_refs || EXCLUDED.key_refs) AS r \
                       ORDER BY r) \
                   )",
            )
            .bind(&tenant.0)
            .bind(&region.0)
            .bind(subject)
            .bind(&refs)
            .bind(&erased_at.0)
            .execute(&self.pool)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
        })
    }

    fn is_erased(&self, tenant: &TenantId, region: &Region, subject: &str) -> bool {
        self.block(async {
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM bus_erasure_ledger \
                 WHERE tenant = $1 AND region = $2 AND subject = $3)",
            )
            .bind(&tenant.0)
            .bind(&region.0)
            .bind(subject)
            .fetch_one(&self.pool)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "BUS ERASURE-LEDGER DURABILITY FAILURE (fail-static): is_erased read failed for \
                     subject={subject} tenant={} - an incomplete read is a silent resurrection path \
                     (EB-16/BUS-D8): {e}",
                    tenant.0
                )
            });
            exists
        })
    }

    fn entries(&self, tenant: &TenantId, region: &Region) -> Vec<ErasedSubject> {
        self.block(async {
            let rows: Vec<(String, Vec<String>, String)> = sqlx::query_as(
                "SELECT subject, key_refs, erased_at FROM bus_erasure_ledger \
                 WHERE tenant = $1 AND region = $2 ORDER BY subject",
            )
            .bind(&tenant.0)
            .bind(&region.0)
            .fetch_all(&self.pool)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "BUS ERASURE-LEDGER DURABILITY FAILURE (fail-static): entries read failed for \
                     tenant={} - an incomplete replay set would let a resurrected subject escape the \
                     post-restore re-erasure pass (EB-16/BUS-D8): {e}",
                    tenant.0
                )
            });
            rows.into_iter()
                .map(|(subject, key_refs, erased_at)| ErasedSubject {
                    subject,
                    key_refs: key_refs.into_iter().map(PiiKeyRef).collect(),
                    erased_at: Timestamp(erased_at),
                })
                .collect()
        })
    }
}
