//! **The durable Postgres [`RebuildJournal`] — the production default.**
//!
//! [`crate::rebuild`] defines the coordinator's phases and their ordering; this module is where
//! those phases become durable. The distinction matters more here than in most stores: the entire
//! safety argument for the migration rests on the journal surviving the process. A coordinator whose
//! phase lives in memory would, on restart after a crash mid-rebuild, re-enter at
//! [`RebuildPhase::Claimed`](crate::rebuild::RebuildPhase::Claimed) and **re-wipe an index it had
//! already spent an hour replaying** — and, worse, would consider reads open because it has no
//! record saying otherwise.
//!
//! ## The compare-and-set is the whole exclusivity guarantee
//!
//! [`PgRebuildJournal::compare_and_store`] is a single conditional statement — an `UPDATE ... WHERE
//! fence_epoch = $expected`, or an `INSERT ... ON CONFLICT DO NOTHING` for the initial claim. It is
//! never a read followed by a write. Two coordinators racing to claim the same `(tenant, region)`
//! both read the same epoch; if the store were read-then-write both would believe they won, and one
//! would wipe the index the other was replaying into.
//!
//! Postgres decides the winner, exactly once, by row count. `rows_affected() == 1` is the win.
//!
//! ## An unreachable store is an error, never a lost race
//!
//! The trait separates `Ok(false)` ("someone else holds this") from `Err` ("the store is
//! unreachable"), and this implementation keeps them separate. Collapsing a connection failure into
//! `Ok(false)` would tell a caller to stand down because the rebuild is in good hands, at the exact
//! moment nothing is in anyone's hands.
//!
//! ## Scanner posture (`no-in-memory-durable-store`)
//!
//! [`PgRebuildJournal`] holds a `PgPool` (the durability proof) and a runtime handle, and NO
//! in-memory collection — the same shape as
//! [`myelin_storage::outbox_durable::PgOutboxBacking`](myelin_storage). The in-memory
//! `MemoryRebuildJournal` double is `test-support`-gated, so the scanner strips it from the
//! production graph.

use sqlx::postgres::PgPool;
use sqlx::Row;

use crate::rebuild::{RebuildError, RebuildJournal, RebuildKey, RebuildPhase, RebuildRecord};

/// The delimiter joining replayed-owner scope keys into the `owners_replayed` column.
///
/// ASCII UNIT SEPARATOR. A scope key is `<owner>:<selector>` built from subsystem tokens and
/// artifact selectors, none of which may contain a control character — so this delimiter cannot
/// collide with the data. [`encode_owners`] verifies that rather than assuming it: a key containing
/// the separator is refused, because silently splitting one owner into two would make a resumed
/// replay skip a corpus.
const OWNER_DELIMITER: char = '\u{1f}';

/// Read the rebuild row for a `(tenant, region)`.
pub const SELECT_REBUILD_JOB_QUERY: &str = "\
SELECT phase, fence_epoch, high_water_mark, high_water_seqs, pre_wipe_docs, owners_replayed,
       lease_holder, lease_expires_at
  FROM search_rebuild_job
 WHERE tenant = $1 AND region = $2";

/// The INITIAL claim: insert iff no row exists. `ON CONFLICT DO NOTHING` makes a lost race report
/// `rows_affected() == 0` rather than raising, so the caller reads it as "someone else claimed".
pub const INSERT_REBUILD_JOB_QUERY: &str = "\
INSERT INTO search_rebuild_job
       (tenant, region, phase, fence_epoch, high_water_mark, owners_replayed,
        lease_holder, lease_expires_at, high_water_seqs, pre_wipe_docs)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $11)
    ON CONFLICT (tenant, region) DO NOTHING";

/// Every subsequent write: conditional on the fence epoch the caller claimed under. A holder whose
/// lease was stolen carries a stale epoch, matches no row, and is refused.
pub const UPDATE_REBUILD_JOB_QUERY: &str = "\
UPDATE search_rebuild_job
   SET phase = $3,
       fence_epoch = $4,
       high_water_mark = $5,
       owners_replayed = $6,
       lease_holder = $7,
       lease_expires_at = $8,
       high_water_seqs = $10,
       pre_wipe_docs = $11,
       updated_at = now()
 WHERE tenant = $1 AND region = $2 AND fence_epoch = $9";

/// **The REAL durable rebuild journal over the OLTP pool.**
///
/// Cloneable (the pool is an `Arc`-backed handle). The caller must have applied the Search
/// forward-only migrations (`0011_search_rebuild_job`), which the shell's `serve(AppSpec)` migrate
/// step does at boot.
///
/// Holds a `PgPool` + a `tokio::runtime::Handle` — the sync→async bridge, so the synchronous
/// coordinator and indexer are not forced to colour their whole call graph async. Same posture as
/// `PgOutboxBacking`.
#[derive(Clone)]
pub struct PgRebuildJournal {
    pool: PgPool,
    rt: tokio::runtime::Handle,
}

impl PgRebuildJournal {
    /// Wrap a pool as the durable rebuild journal. `rt` is the runtime handle the sync trait methods
    /// drive the async sqlx client on.
    pub fn new(pool: PgPool, rt: tokio::runtime::Handle) -> PgRebuildJournal {
        PgRebuildJournal { pool, rt }
    }

    /// The pool this journal is bound to.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    fn block<F: std::future::Future>(&self, fut: F) -> F::Output {
        tokio::task::block_in_place(|| self.rt.block_on(fut))
    }
}

/// Join replayed-owner scope keys for the `owners_replayed` column.
///
/// Refuses a key containing the delimiter rather than emitting an ambiguous row: a mis-split key
/// would make a resumed replay believe a corpus was already done and skip it, which is a silent
/// data-loss shape, not a formatting nit.
fn encode_owners(owners: &std::collections::BTreeSet<String>) -> Result<String, RebuildError> {
    if owners.iter().any(|o| o.contains(OWNER_DELIMITER)) {
        return Err(RebuildError::Journal(
            "a replayed-owner scope key contains the reserved delimiter".into(),
        ));
    }
    Ok(owners
        .iter()
        .cloned()
        .collect::<Vec<_>>()
        .join(&OWNER_DELIMITER.to_string()))
}

/// Serialize the per-aggregate `seq` watermark for the `high_water_seqs` column.
///
/// JSON, because the map is `aggregate -> seq` and an aggregate key is arbitrary producer text —
/// a delimiter-joined encoding would need the same escaping argument the owner column makes, and
/// here the values are structured rather than a flat set. A serialization failure is LOUD: a
/// silently-empty watermark would make catch-up skip every event.
fn encode_seqs(seqs: &std::collections::BTreeMap<String, u64>) -> Result<String, RebuildError> {
    serde_json::to_string(seqs)
        .map_err(|_| RebuildError::Journal("the high-water watermark did not serialize".into()))
}

/// Parse the `high_water_seqs` column back into the watermark.
///
/// A NULL / absent column is an EMPTY map, which legitimately means "no aggregate had committed rows
/// at fence time, so apply nothing" — the shape a fresh cell produces.
///
/// A MALFORMED column is an ERROR, not an empty map. Degrading it to empty would make catch-up
/// silently apply zero pre-fence events, and — because an empty expectation makes the parity legs
/// compare `0 == 0` — that can pass verification. A corrupted journal row must stop the rebuild, not
/// quietly narrow it.
fn decode_seqs(
    raw: Option<&str>,
) -> Result<std::collections::BTreeMap<String, u64>, RebuildError> {
    match raw {
        None => Ok(std::collections::BTreeMap::new()),
        Some(s) => serde_json::from_str(s).map_err(|_| {
            RebuildError::Journal(
                "the stored high-water watermark is malformed — refusing to treat a corrupt bound \
                 as an empty one"
                    .into(),
            )
        }),
    }
}

/// Split the `owners_replayed` column back into scope keys. Empty segments are dropped (an empty
/// column is "no owners replayed", not "one owner with an empty name").
fn decode_owners(raw: &str) -> std::collections::BTreeSet<String> {
    raw.split(OWNER_DELIMITER)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

impl RebuildJournal for PgRebuildJournal {
    fn load(&self, key: &RebuildKey) -> Result<Option<RebuildRecord>, RebuildError> {
        self.block(async {
            // The tenant predicate is threaded explicitly (`tenant_id`), not reached through a
            // field chain: this row is tenant-keyed, and the `tenant-predicate` ratchet reads the
            // binding on the statement. Both halves of the partition key are bound — a rebuild job
            // is per `(tenant, region)`, never cell-wide.
            let tenant_id = key.tenant.0.as_str();
            let row = sqlx::query(SELECT_REBUILD_JOB_QUERY)
                .bind(tenant_id)
                .bind(key.region.0.as_str())
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| RebuildError::Journal(e.to_string()))?;
            let Some(row) = row else {
                return Ok(None);
            };
            // An unrecognised phase token is LOUD. Coercing it (to `Claimed`, say) would re-wipe a
            // finished index; treating it as `Complete` would reopen reads over a half-built one.
            // Both are worse than refusing to act on a row this binary does not understand.
            let phase = RebuildPhase::from_token(row.get::<String, _>("phase").as_str())
                .ok_or(RebuildError::UnknownPhase)?;
            Ok(Some(RebuildRecord {
                phase,
                fence_epoch: row.get::<i64, _>("fence_epoch").max(0) as u64,
                high_water_mark: row
                    .get::<Option<i64>, _>("high_water_mark")
                    .map(|v| v.max(0) as u64),
                pre_wipe_docs: row
                    .get::<Option<i64>, _>("pre_wipe_docs")
                    .map(|v| v.max(0) as u64),
                high_water_seqs: decode_seqs(
                    row.get::<Option<String>, _>("high_water_seqs").as_deref(),
                )?,
                owners_replayed: decode_owners(&row.get::<String, _>("owners_replayed")),
                lease_holder: row.get::<Option<String>, _>("lease_holder"),
                lease_expires_at: row.get::<i64, _>("lease_expires_at").max(0) as u64,
            }))
        })
    }

    fn compare_and_store(
        &self,
        key: &RebuildKey,
        expected_epoch: Option<u64>,
        next: &RebuildRecord,
    ) -> Result<bool, RebuildError> {
        let owners = encode_owners(&next.owners_replayed)?;
        let seqs = encode_seqs(&next.high_water_seqs)?;
        // The tenant predicate, threaded explicitly on both write statements (see `load`).
        let tenant_id = key.tenant.0.as_str();
        self.block(async {
            let done = match expected_epoch {
                // The initial claim: insert iff absent. A concurrent claimer that got there first
                // leaves `rows_affected() == 0` — a lost race, not an error.
                None => sqlx::query(INSERT_REBUILD_JOB_QUERY)
                    .bind(tenant_id)
                    .bind(key.region.0.as_str())
                    .bind(next.phase.token())
                    .bind(next.fence_epoch as i64)
                    .bind(next.high_water_mark.map(|v| v as i64))
                    .bind(owners.as_str())
                    .bind(next.lease_holder.as_deref())
                    .bind(next.lease_expires_at as i64)
                    .bind(seqs.as_str()) // $9
                    .bind(Option::<i64>::None) // $10 unused on the insert arm
                    .bind(next.pre_wipe_docs.map(|v| v as i64)) // $11
                    .execute(&self.pool)
                    .await,
                // Every later write: conditional on the epoch this holder claimed under.
                Some(expected) => sqlx::query(UPDATE_REBUILD_JOB_QUERY)
                    .bind(tenant_id)
                    .bind(key.region.0.as_str())
                    .bind(next.phase.token())
                    .bind(next.fence_epoch as i64)
                    .bind(next.high_water_mark.map(|v| v as i64))
                    .bind(owners.as_str())
                    .bind(next.lease_holder.as_deref())
                    .bind(next.lease_expires_at as i64)
                    .bind(expected as i64)
                    .bind(seqs.as_str())
                    .bind(next.pre_wipe_docs.map(|v| v as i64))
                    .execute(&self.pool)
                    .await,
            }
            .map_err(|e| RebuildError::Journal(e.to_string()))?;
            // Postgres decided the race, by row count. Exactly one row means this caller won.
            Ok(done.rows_affected() == 1)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// **The owner-set round-trips through the column encoding.** A resumed replay reads this back
    /// to decide which corpora to skip, so a lossy round-trip skips a corpus silently.
    #[test]
    fn the_owner_set_round_trips() {
        let owners: BTreeSet<String> = ["git:blob:all", "knowledge:page:all", "chat:message:all"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let encoded = encode_owners(&owners).expect("encodes");
        assert_eq!(decode_owners(&encoded), owners);
    }

    /// **A malformed watermark column is an ERROR, not a silent empty bound.** Degrading it to
    /// empty makes catch-up apply nothing and — because an empty expectation makes parity compare
    /// `0 == 0` — that can pass verification over a wiped index.
    #[test]
    fn a_malformed_watermark_is_refused_not_silently_emptied() {
        assert!(
            matches!(decode_seqs(Some("{not json")), Err(RebuildError::Journal(_))),
            "a corrupt bound must stop the rebuild"
        );
        // A genuinely absent column is the legitimate empty case (a fresh cell).
        assert_eq!(decode_seqs(None).unwrap().len(), 0);
        assert_eq!(decode_seqs(Some("{}")).unwrap().len(), 0);
        let parsed = decode_seqs(Some(r#"{"agg-a":3,"agg-b":7}"#)).unwrap();
        assert_eq!(parsed.get("agg-b"), Some(&7));
    }

    /// An empty owner set encodes to an empty column and decodes back to empty — NOT to a set
    /// containing one empty key (which a resumed replay would try to skip).
    #[test]
    fn an_empty_owner_set_round_trips_as_empty() {
        let empty = BTreeSet::new();
        let encoded = encode_owners(&empty).expect("encodes");
        assert!(encoded.is_empty());
        assert!(decode_owners(&encoded).is_empty());
    }

    /// **A scope key carrying the reserved delimiter is REFUSED, not silently split.** Splitting it
    /// would register two bogus owners and lose the real one — a resumed rebuild would then skip a
    /// corpus it never replayed.
    #[test]
    fn a_key_containing_the_delimiter_is_refused() {
        let mut owners = BTreeSet::new();
        owners.insert(format!("git{OWNER_DELIMITER}smuggled"));
        assert!(
            matches!(encode_owners(&owners), Err(RebuildError::Journal(_))),
            "an ambiguous owner key must be refused, never split"
        );
    }

    /// **The conditional-write SQL is genuinely conditional.** The whole exclusivity guarantee is
    /// that the update carries a `fence_epoch` predicate and the insert cannot overwrite. A refactor
    /// that drops either turns two racing coordinators into a wipe during a replay.
    #[test]
    fn the_write_queries_are_compare_and_set() {
        assert!(
            UPDATE_REBUILD_JOB_QUERY.contains("fence_epoch = $9"),
            "the update is conditional on the claimed fence epoch"
        );
        assert!(
            UPDATE_REBUILD_JOB_QUERY.contains("tenant = $1") && UPDATE_REBUILD_JOB_QUERY.contains("region = $2"),
            "and scoped to one (tenant, region)"
        );
        assert!(
            INSERT_REBUILD_JOB_QUERY.contains("ON CONFLICT (tenant, region) DO NOTHING"),
            "the initial claim never overwrites an existing job"
        );
        assert!(
            SELECT_REBUILD_JOB_QUERY.contains("tenant = $1")
                && SELECT_REBUILD_JOB_QUERY.contains("region = $2"),
            "a load is tenant+region scoped — never a cell-wide read"
        );
    }

    /// Every phase token round-trips, so a durable row written by one build is readable by the next.
    /// An unknown token yields `None`, which the loader turns into a LOUD `UnknownPhase`.
    #[test]
    fn every_phase_token_round_trips() {
        for phase in [
            RebuildPhase::Claimed,
            RebuildPhase::Fenced,
            RebuildPhase::Wiped,
            RebuildPhase::CursorsReset,
            RebuildPhase::Replayed,
            RebuildPhase::CaughtUp,
            RebuildPhase::Verified,
            RebuildPhase::Complete,
        ] {
            assert_eq!(RebuildPhase::from_token(phase.token()), Some(phase));
        }
        assert_eq!(RebuildPhase::from_token("cursors_rest"), None);
        assert_eq!(RebuildPhase::from_token(""), None);
    }
}
