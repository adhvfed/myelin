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

use myelin_events::{
    ConsumerName, DurableBusErasure, DurableDedup, ErasedSubject, EventId, PiiKeyRef, Region,
    TenantId, Timestamp,
};

use crate::migration::{Migration, Migrations};

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

// =================================================================================================
// Durable PG backing for the Bus's PII-free erasure ledger (contract 10.8, MR-009b W6c-events)
//
// `myelin-events` is a §2.9 DAG SINK (it cannot name a `PgPool`), so — exactly like the
// `consumer_dedup` seam above — the [`myelin_events::DurableBusErasure`] trait is defined IN events
// and this module ships the REAL PG impl + the `bus_erasure_ledger` table. Wired at the
// `EventsRuntime` composition root ([`crate::events_serve`]) via `BusErasureLedger::durable`.
// =================================================================================================

/// The `bus_erasure_ledger` table (contract 10.8, W6c-events) — `(tenant, region, subject)`-keyed.
/// Holds ONLY the OPAQUE subject discriminator + the opaque `pii_key_ref` NAMES (never key material,
/// never a payload) + the audit `erased_at` — PII-free by construction. **NON-shred-erasable + NO RLS
/// by construction** (mirrors the W6a `identity_pseudonym_erasure_ledger` / W6b
/// `post_pit_erasure_ledger`): it MUST survive the crypto-shred it records AND a restore, so
/// `BusHolder::re_erase_after_restore` can replay it against a resurrected pre-erase backup — a
/// crypto-shred/RLS lever on THIS table would defeat that. Partition isolation is the explicit
/// `(tenant, region)` predicate on every statement (a NAMED tenant-predicate exclusion, like
/// `DurableDedupBacking` / `pgrelay` / `reerase_durable`). Forward-only (`IF NOT EXISTS`); the
/// idempotent `key_refs` merge is an `ON CONFLICT … DO UPDATE` array-merge (union + dedup + sort).
pub const BUS_ERASURE_LEDGER_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS bus_erasure_ledger (
    tenant    text   NOT NULL,
    region    text   NOT NULL,
    subject   text   NOT NULL,
    key_refs  text[] NOT NULL DEFAULT '{}',
    erased_at text   NOT NULL,
    PRIMARY KEY (tenant, region, subject)
);";

/// The forward-only migration set the durable Bus erasure ledger binds to (id `0053`, next in the free
/// `0053+` range after the W6b `0052_post_pit_erasure_ledger`). Applied via the MR-022
/// [`crate::provider::SubstrateProvider::migrate`] at boot; idempotent on re-boot (`CREATE TABLE IF
/// NOT EXISTS`).
pub fn bus_erasure_durable_migrations() -> Migrations {
    Migrations::of([Migration::plain(
        "0053_bus_erasure_ledger",
        BUS_ERASURE_LEDGER_MIGRATION,
    )])
}

/// The REAL durable `bus_erasure_ledger` backing over the OLTP `PgPool` (MR-009b W6c-events). Cloneable
/// (the pool is an `Arc`-backed handle). Constructed by the events `serve()` composition root over the
/// SAME pool the relay/stores use, and injected into [`myelin_events::BusErasureLedger::durable`].
///
/// **NON-shred-erasable + NO RLS** (see [`BUS_ERASURE_LEDGER_MIGRATION`]): every statement carries the
/// explicit `(tenant, region)` predicate — a NAMED tenant-predicate exclusion, exactly like the sibling
/// [`DurableDedupBacking`]. The sync [`DurableBusErasure`] seam bridges onto the tokio runtime handle
/// (`block_in_place` + `block_on`, the same bridge [`DurableDedupBacking`] uses).
#[derive(Clone)]
pub struct DurableBusErasureBacking {
    pool: PgPool,
    rt: tokio::runtime::Handle,
}

impl DurableBusErasureBacking {
    /// Wrap a pool as the durable Bus erasure-ledger backing. `rt` is the runtime handle the sync trait
    /// methods drive the async sqlx client on. The caller must have applied
    /// [`BUS_ERASURE_LEDGER_MIGRATION`] (via [`bus_erasure_durable_migrations`]).
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
        // The idempotent merge (10.8) as ONE honest upsert: on conflict the stored `key_refs` are
        // UNIONed with the incoming refs, then de-duplicated + sorted (`DISTINCT … ORDER BY` — matching
        // the in-memory ledger's distinct, `PiiKeyRef.0`-sorted refs), and the FIRST `erased_at` is
        // KEPT (`erased_at` is NOT in the DO UPDATE SET) — a later erase that locates more keys merges
        // them without moving the recorded time. Deterministic array order so the replay artifact is
        // reproducible.
        //
        // The incoming refs are normalized (sorted + deduped) in RUST before binding (W6c verifier
        // finding, probe-proven): the no-conflict INSERT arm stores the bound array VERBATIM — only
        // the DO UPDATE arm runs the DISTINCT/ORDER BY — so an unnormalized first insert would
        // diverge from the memory arm (which normalizes on every record) and a duplicated ref would
        // double-count `keys_resurrected_by_restore` in the re-erasure receipt. Rust-side sort uses
        // byte order, matching the memory arm exactly (immune to DB collation, unlike ORDER BY).
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
            // Fail-direction: return Err so the ledger's infallible `record` panics fail-static (an
            // unrecorded erasure is a silent resurrection path across a restore — never swallowed).
            .map_err(|e| e.to_string())
        })
    }

    fn is_erased(&self, tenant: &TenantId, region: &Region, subject: &str) -> bool {
        self.block(async {
            // `EXISTS(..)` decodes as bool. FAIL-STATIC on a store fault: a read failure must NOT be
            // swallowed to a false "not erased" — that would let a resurrected subject read as
            // never-erased (a silent resurrection path).
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
                     subject={subject} tenant={} — an incomplete read is a silent resurrection path \
                     (EB-16/BUS-D8): {e}",
                    tenant.0
                )
            });
            exists
        })
    }

    fn entries(&self, tenant: &TenantId, region: &Region) -> Vec<ErasedSubject> {
        self.block(async {
            // The `(tenant, region)`-scoped, subject-sorted replay set. FAIL-STATIC on a store fault:
            // an incomplete replay set would let a resurrected subject escape the post-restore
            // re-erasure pass (BUS-D8 would report green while a real identity resolves).
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
                     tenant={} — an incomplete replay set would let a resurrected subject escape the \
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
