//! # `PgMigrator` — the race-safe LIVE migration DRIVER (the P-S12 named floor)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/storage.md` §3.1 (Tier 1 OLTP: forward-only
//! online migrations). This is the **concrete DDL-execution driver** the migration vocabulary
//! ([`crate::migration`]) named as a floor: *"the concrete DDL execution against Postgres is a
//! named floor (the driver, P-S12)"*. It lands here, behind `--features integration` (it pulls the
//! real `sqlx` client), exactly like the other live-PG code ([`crate::pg`]).
//!
//! ## THE BUG THIS FIXES (the pg_type race)
//! [`crate::pg::PgStore::migrate`] (and git's `check_status` projection, and the substrate boot
//! runner before this) ran each migration as a bare `sqlx::raw_sql(ddl).execute(&pool)` with NO
//! advisory lock and NO version table. When **multiple processes/tests migrate the SAME database
//! concurrently**, each `CREATE TABLE` creates a row type, and two concurrent `CREATE TABLE`s race
//! on Postgres's `pg_type_typname_nsp_index` — a duplicate-key error that fails one of the racers.
//! `CREATE TABLE IF NOT EXISTS` does NOT close this: the existence check and the row-type insert are
//! not atomic against a concurrent creator.
//!
//! ## THE FIX — forward-only, idempotent, SERIALIZED, version-recorded
//! [`PgMigrator::apply`] is the production-shaped migrator:
//!   1. Acquire a DEDICATED connection from the pool.
//!   2. Take a Postgres **session advisory lock** on a FIXED app-wide key
//!      ([`MIGRATION_LOCK_KEY`]). This SERIALIZES all migration across processes/tests — only one
//!      migrator runs the DDL at a time, so two `CREATE TABLE`s never race. The lock is held on the
//!      dedicated connection (NOT across the whole pool); dropping that connection releases it as a
//!      panic-safe backstop even if `pg_advisory_unlock` is never reached.
//!   3. Under the lock, `CREATE TABLE IF NOT EXISTS myelin_applied_migration (…)` — the version
//!      table that records what has been applied.
//!   4. For each migration in order: compute the BLAKE3 checksum of its current DDL. If its `id` is
//!      already in `myelin_applied_migration`, compare the recorded checksum and SKIP **only** when
//!      it is identical. A changed DDL under an existing id is immutable-history drift and fails
//!      loudly before any later migration runs. Otherwise validate it is forward-only (reject a
//!      destructive `DROP` via [`is_destructive`]), run the DDL, and INSERT the `(id, checksum)` row.
//!   5. Release the advisory lock.
//!
//! Because step 4 SKIPS an identically applied id while holding the lock, the migrator is both
//! *idempotent* (re-running applies nothing) and *concurrency-safe* (the lock serialises the
//! first-run DDL, the checksum-verified skip prevents a re-run). Migration ids are immutable: edit
//! history by adding a new forward migration, never by changing the DDL behind an applied id. The
//! regression test
//! (`tests/integration_migrate_concurrent.rs`) spawns N≥8 concurrent migrators against one DB and
//! asserts every one returns `Ok` and each id appears EXACTLY once — it fails WITHOUT the lock (it
//! reproduces the original `pg_type_typname_nsp_index` race).
//!
//! ## Not a host-exec orchestration site
//! This driver issues **`sqlx`** statements only — no `Command`/host process exec. It is OLTP-tier
//! data-plane code, fully subject to the same lints as [`crate::pg`]; it needs no no-host-exec
//! exclusion.

use crate::migration::{is_blocking_alter, is_destructive, HotTables, Migrations};
use crate::pg::PgError;
use sqlx::postgres::PgPool;
use sqlx::Executor;
use std::collections::BTreeMap;

/// The FIXED app-wide Postgres advisory-lock key all migration serialises on. Derived from a stable
/// string (`"myelin.schema.migrate"`) so the constant is documented + reproducible, NOT a magic
/// number: it is the first 8 bytes of the BLAKE3 hash of that string, read as a big-endian `i64`.
/// Every migrator across every process takes `pg_advisory_lock(MIGRATION_LOCK_KEY)`, so migration is
/// globally serialized — the fix for the concurrent-`CREATE TABLE` `pg_type` race. A single fixed
/// key (not per-migration) is deliberate: ALL schema DDL serialises against ALL other schema DDL on
/// the same database, which is what makes the `pg_type_typname_nsp_index` row-type insert safe.
pub const MIGRATION_LOCK_KEY: i64 = migration_lock_key();

/// Compute [`MIGRATION_LOCK_KEY`] at compile time from the stable lock string. `const`, so the key
/// is a documented derivation rather than a hand-picked literal.
const fn migration_lock_key() -> i64 {
    // BLAKE3 of "myelin.schema.migrate", first 8 bytes big-endian. Computed once (const) so the
    // value is fixed + reproducible; the bytes below are that digest's prefix.
    //
    // blake3("myelin.schema.migrate") =
    //   f5d26c962c935805fabdee9b8a9cca04d8242e1947e62b15a4a70e4c2b2e3953
    // We take the first 8 bytes as a big-endian i64. The `migration_lock_key_matches_digest` test
    // re-derives this from blake3 and asserts equality, so the literal can never silently drift.
    const BYTES: [u8; 8] = [0xf5, 0xd2, 0x6c, 0x96, 0x2c, 0x93, 0x58, 0x05];
    i64::from_be_bytes(BYTES)
}

/// The version table that records each applied migration id + a checksum of its DDL. `text`
/// primary key on `id` makes a second insert of the same id a no-op-by-skip (we check-then-insert
/// under the advisory lock, so the PK is the belt to the lock's braces). The `checksum` lets a
/// later run detect that a migration's DDL changed under a stable id (a contract drift — flagged by
/// reading it back, never silently re-applied).
const APPLIED_MIGRATION_DDL: &str = "\
CREATE TABLE IF NOT EXISTS myelin_applied_migration (
    id         text        PRIMARY KEY,
    applied_at timestamptz NOT NULL DEFAULT now(),
    checksum   text        NOT NULL
);";

/// The race-safe forward-only live migrator (the P-S12 driver floor). Stateless — its one method
/// [`PgMigrator::apply`] takes the pool + the [`Migrations`] to apply.
pub struct PgMigrator;

/// One migration id that maps to different DDL checksums in two registered migration sets.
/// Reusing an id for byte-identical DDL is harmless and is not a collision; reusing it for any
/// other bytes is immutable-history drift that would make service startup order-dependent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MigrationChecksumCollision {
    /// The duplicated migration id.
    pub id: String,
    /// Name of the first migration set in which the id appeared.
    pub first_set: String,
    /// Checksum registered by `first_set`.
    pub first_checksum: String,
    /// Name of the later migration set that reused the id with different DDL.
    pub second_set: String,
    /// Incompatible checksum registered by `second_set`.
    pub second_checksum: String,
}

/// Find incompatible migration-id reuse across named migration sets. Exact `(id, DDL)` reuse is
/// intentionally admitted—for example a writer-critical subset may share entries with its full
/// service migration set. A shared id with different DDL is returned as a concrete collision.
pub fn migration_checksum_collisions<'a>(
    sets: impl IntoIterator<Item = (&'a str, &'a Migrations)>,
) -> Vec<MigrationChecksumCollision> {
    let mut first_by_id: BTreeMap<&str, (&str, String)> = BTreeMap::new();
    let mut collisions = Vec::new();
    for (set_name, migrations) in sets {
        for migration in &migrations.0 {
            let checksum = ddl_checksum(migration.ddl);
            match first_by_id.get(migration.id) {
                None => {
                    first_by_id.insert(migration.id, (set_name, checksum));
                }
                Some((_first_set, first_checksum)) if *first_checksum == checksum => {}
                Some((first_set, first_checksum)) => {
                    collisions.push(MigrationChecksumCollision {
                        id: migration.id.to_string(),
                        first_set: (*first_set).to_string(),
                        first_checksum: first_checksum.clone(),
                        second_set: set_name.to_string(),
                        second_checksum: checksum,
                    });
                }
            }
        }
    }
    collisions
}

impl PgMigrator {
    /// Apply `migrations` against `pool`, forward-only, idempotent, SERIALIZED (advisory lock), and
    /// version-recorded. See the module docs for the full discipline. Returns `Ok(())` once every
    /// migration is recorded as applied (whether this call ran its DDL or checksum-verified an
    /// already-applied id). A destructive (`DROP`) migration is rejected ([`PgError::Migrate`])
    /// before any DDL runs. An existing id with a different checksum is rejected before any later
    /// migration runs; changing an applied migration in place is never silently accepted.
    pub async fn apply(pool: &PgPool, migrations: &Migrations) -> Result<(), PgError> {
        // (1) A DEDICATED connection — the advisory lock is a SESSION lock, so it must live on one
        //     connection for its whole lifetime (a pool round-trip could land on a different
        //     backend and silently not hold the lock). Holding it here, not across the pool, also
        //     means a panic/drop releases the lock (the backstop the prompt asks for).
        let mut conn = pool
            .acquire()
            .await
            .map_err(|e| PgError::Migrate(format!("acquire migration connection: {e}")))?;

        // (2) SERIALIZE: take the fixed app-wide session advisory lock. Blocks until any other
        //     migrator (this process or another) releases it — so the concurrent CREATE TABLE that
        //     races on pg_type_typname_nsp_index can no longer happen (only one migrator runs DDL).
        // Use the concrete `&mut PgConnection` throughout (not the `PoolConnection`'s `as_mut()`):
        // `Executor for &mut PgConnection` resolves without the higher-ranked-lifetime ambiguity that
        // `PoolConnection`'s deref introduces, so the resulting future is provably `Send` (it is
        // `tokio::spawn`ed by the concurrent regression test).
        let conn: &mut sqlx::PgConnection = &mut conn;
        sqlx::query("SELECT pg_advisory_lock($1)")
            .bind(MIGRATION_LOCK_KEY)
            .execute(&mut *conn)
            .await
            .map_err(|e| PgError::Migrate(format!("acquire advisory migration lock: {e}")))?;

        // Run the body under the lock; ALWAYS release the lock afterwards (even on error), then
        // propagate the body's result. The dedicated connection dropping is the panic-safe backstop.
        let result = Self::apply_locked(&mut *conn, migrations).await;

        // (5) Release the session advisory lock explicitly (the connection drop would also release
        //     it, but the explicit unlock returns it promptly so the next migrator proceeds).
        let _ = sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(MIGRATION_LOCK_KEY)
            .execute(&mut *conn)
            .await;

        result
    }

    /// **The reconciled boot entry (MR-022 / SI-010): VALIDATE then EXECUTE.** This is the one call
    /// the production composition root ([`crate::provider::SubstrateProvider::migrate`]) makes so the
    /// boot path *actually executes the DDL* — closing the SI-010 gap where the substrate boot-time
    /// [`myelin_substrate::migrations::MigrationRunner::run`] validated the set (forward-only /
    /// hot-table refusals) but executed NOTHING.
    ///
    /// It runs the SAME forward-only + hot-table checks that boot-time validator runs — using the
    /// shared, single-authority predicates ([`is_destructive`] / [`is_blocking_alter`] /
    /// [`HotTables`]) so the two never drift — and ONLY THEN hands the admitted set to the race-safe
    /// driver [`apply`](Self::apply) (advisory lock + `myelin_applied_migration` version table +
    /// idempotent skip). A rejected set runs NO DDL and takes NO lock (the validation is a pure
    /// pre-flight before any connection is acquired). The result: validate → apply, in one call, so
    /// after boot the substrate tables exist in the live DB and a re-boot is idempotent.
    ///
    /// (The narrower [`apply`](Self::apply) — which validates only forward-only inside the lock — is
    /// retained for the concurrent-migrate regression test + the pre-existing `PgStore::migrate`
    /// caller; `apply_validated` is the hot-table-AWARE boot reconciliation MR-022 adds.)
    pub async fn apply_validated(
        pool: &PgPool,
        migrations: &Migrations,
        hot_tables: &HotTables,
    ) -> Result<(), PgError> {
        // (1) VALIDATE — forward-only + hot-table, before any DDL runs or any lock is taken. This
        //     mirrors `myelin_substrate::migrations::MigrationRunner::run` exactly (the substrate
        //     boot-time validator), via the shared single-authority predicates.
        for m in &migrations.0 {
            if is_destructive(m.ddl) {
                return Err(PgError::Migrate(format!(
                    "migration {} is destructive (DROP) — forward-only migrations only; a rollback \
                     is a NEW forward migration, never a down (§9.1)",
                    m.id
                )));
            }
            if let Some(table) = m.table {
                if hot_tables.is_hot(table) && is_blocking_alter(m.ddl) {
                    return Err(PgError::Migrate(format!(
                        "migration {} takes a blocking ALTER on the declared-HOT table `{}` — a \
                         hot-table change must be expand→backfill→contract, never one blocking \
                         ALTER that locks writes at QPS (§9.4)",
                        m.id, table
                    )));
                }
            }
        }
        // (2) EXECUTE — the admitted set, race-safe + idempotent + version-recorded.
        Self::apply(pool, migrations).await
    }

    /// The under-the-lock body: ensure the version table, then apply-or-skip each migration in
    /// order. Split out so [`apply`](Self::apply) can guarantee the unlock runs whatever this
    /// returns.
    async fn apply_locked(
        conn: &mut sqlx::PgConnection,
        migrations: &Migrations,
    ) -> Result<(), PgError> {
        // (3) The version table — created under the lock, so even its own CREATE cannot race.
        //     We execute multi-statement DDL through the `Executor::execute(&str)` simple-query path
        //     (NOT `raw_sql`): it supports a multi-statement script AND keeps the future `Send`
        //     (`raw_sql`'s executor bound trips a higher-ranked-lifetime ambiguity that makes the
        //     spawned future non-`Send`; the concurrent regression test `tokio::spawn`s this).
        conn.execute(APPLIED_MIGRATION_DDL)
            .await
            .map_err(|e| PgError::Migrate(format!("create myelin_applied_migration: {e}")))?;

        // (4) Apply each migration in order, idempotently.
        for m in &migrations.0 {
            // forward-only: a destructive DROP is never admitted by the driver (the same predicate
            // the OnlineMigrationRunner + the forward-only-migration lint use).
            if is_destructive(m.ddl) {
                return Err(PgError::Migrate(format!(
                    "migration {} is destructive (DROP) — forward-only migrations only; a rollback \
                     is a NEW forward migration, never a down (storage §3.1)",
                    m.id
                )));
            }

            // Already applied? SKIP only after proving the immutable DDL is byte-identical. The
            // checksum is read under the same app-wide advisory lock as execution, so a concurrent
            // migrator cannot change the decision between this probe and a later DDL. There are no
            // implicit aliases: accepting a second checksum would hide exactly the drift this
            // ledger exists to detect.
            let expected_checksum = ddl_checksum(m.ddl);
            let recorded_checksum: Option<String> =
                sqlx::query_scalar("SELECT checksum FROM myelin_applied_migration WHERE id = $1")
                    .bind(m.id)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(|e| {
                        PgError::Migrate(format!("check applied migration {}: {e}", m.id))
                    })?;
            if let Some(recorded_checksum) = recorded_checksum {
                verify_recorded_checksum(m.id, &recorded_checksum, &expected_checksum)?;
                continue;
            }

            // Run the forward-only DDL (a multi-statement script runs as one simple query via the
            // `Executor::execute(&str)` path — see the version-table note above on why not `raw_sql`).
            conn.execute(m.ddl)
                .await
                .map_err(|e| PgError::Migrate(format!("apply migration {}: {e}", m.id)))?;

            // Record it applied, with a BLAKE3 checksum of the DDL (drift-detectable).
            sqlx::query(
                "INSERT INTO myelin_applied_migration (id, checksum) VALUES ($1, $2) \
                 ON CONFLICT (id) DO NOTHING",
            )
            .bind(m.id)
            .bind(&expected_checksum)
            .execute(&mut *conn)
            .await
            .map_err(|e| PgError::Migrate(format!("record migration {}: {e}", m.id)))?;
        }
        Ok(())
    }

    /// Whether a migration `id` is recorded as applied in `myelin_applied_migration`. A small read
    /// helper the concurrent regression test uses to assert applied-once.
    pub async fn is_applied(pool: &PgPool, id: &str) -> Result<bool, PgError> {
        let row: Option<i32> =
            sqlx::query_scalar("SELECT 1 FROM myelin_applied_migration WHERE id = $1")
                .bind(id)
                .fetch_optional(pool)
                .await
                .map_err(|e| PgError::Migrate(format!("query applied migration {id}: {e}")))?;
        Ok(row.is_some())
    }

    /// How many rows in `myelin_applied_migration` carry this `id` (the applied-EXACTLY-once signal
    /// the concurrent regression test asserts is `1` for every migration after N racing migrators).
    pub async fn applied_count(pool: &PgPool, id: &str) -> Result<i64, PgError> {
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM myelin_applied_migration WHERE id = $1")
                .bind(id)
                .fetch_one(pool)
                .await
                .map_err(|e| PgError::Migrate(format!("count applied migration {id}: {e}")))?;
        Ok(count)
    }

    /// Read-only checksum preflight for a migration set. Every applied id in `migrations` must have
    /// the exact checksum of its current DDL; unapplied ids are allowed because this method audits
    /// compatibility without executing or recording migrations. This is intended for deployment
    /// probes and dogfood audits before enabling a stricter migrator on an existing database.
    ///
    /// Unlike [`apply`](Self::apply), this diagnostic does not take the migration lock. Production
    /// startup must still call `apply`, which repeats the comparison while holding the lock before
    /// it makes an apply/skip decision.
    pub async fn audit_applied_checksums(
        pool: &PgPool,
        migrations: &Migrations,
    ) -> Result<(), PgError> {
        for migration in &migrations.0 {
            let recorded_checksum: Option<String> =
                sqlx::query_scalar("SELECT checksum FROM myelin_applied_migration WHERE id = $1")
                    .bind(migration.id)
                    .fetch_optional(pool)
                    .await
                    .map_err(|e| {
                        PgError::Migrate(format!(
                            "audit applied migration {} checksum: {e}",
                            migration.id
                        ))
                    })?;
            if let Some(recorded_checksum) = recorded_checksum {
                let expected_checksum = ddl_checksum(migration.ddl);
                verify_recorded_checksum(migration.id, &recorded_checksum, &expected_checksum)?;
            }
        }
        Ok(())
    }
}

/// Run an arbitrary DDL closure under the SAME app-wide migration advisory lock — the shared helper
/// for a live-DDL site that is not expressed as a [`Migrations`] set (e.g. git's per-table
/// `check_status` projection DDL). It takes a dedicated connection, takes
/// [`MIGRATION_LOCK_KEY`], runs `ddl` (a `CREATE TABLE IF NOT EXISTS …` script) under the lock, and
/// releases it. This is how a caller gets the race-safety of [`PgMigrator::apply`] for a one-off
/// DDL without duplicating the lock logic by hand.
///
/// The closure form is deliberately a plain `&str` DDL rather than an arbitrary async closure: every
/// current caller runs ONE `raw_sql` script, and keeping the surface narrow keeps the lock
/// discipline in one place. (A migration with a stable id should prefer [`PgMigrator::apply`] so it
/// is version-recorded; this is the escape hatch for an idempotent `IF NOT EXISTS` projection that a
/// service re-runs on every startup and does not version.)
pub async fn with_migration_lock(pool: &PgPool, ddl: &str) -> Result<(), PgError> {
    let mut conn = pool
        .acquire()
        .await
        .map_err(|e| PgError::Migrate(format!("acquire migration connection: {e}")))?;
    let conn: &mut sqlx::PgConnection = &mut conn;
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(MIGRATION_LOCK_KEY)
        .execute(&mut *conn)
        .await
        .map_err(|e| PgError::Migrate(format!("acquire advisory migration lock: {e}")))?;

    // Multi-statement DDL via the `Executor::execute(&str)` simple-query path (Send-safe; see the
    // note in `PgMigrator::apply_locked`).
    let result = conn
        .execute(ddl)
        .await
        .map(|_| ())
        .map_err(|e| PgError::Migrate(format!("apply DDL under migration lock: {e}")));

    let _ = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(MIGRATION_LOCK_KEY)
        .execute(&mut *conn)
        .await;

    result
}

/// The BLAKE3 checksum of a migration's DDL, rendered `blake3:<hex>` — the platform multihash
/// convention (the SAME `blake3:<hex>` shape [`crate::blob`] uses). Recorded in
/// `myelin_applied_migration.checksum` so a later run can detect a migration's DDL changed under a
/// stable id.
pub fn ddl_checksum(ddl: &str) -> String {
    format!("blake3:{}", blake3::hash(ddl.as_bytes()).to_hex())
}

/// Admit an idempotent skip only when the recorded and current DDL checksums are identical.
/// Kept as a pure helper so the fail-closed decision has a DB-free unit proof in addition to the
/// live PostgreSQL regression tests.
fn verify_recorded_checksum(
    id: &str,
    recorded_checksum: &str,
    expected_checksum: &str,
) -> Result<(), PgError> {
    if recorded_checksum == expected_checksum {
        return Ok(());
    }

    Err(PgError::Migrate(format!(
        "migration checksum mismatch for existing id `{id}`: recorded `{recorded_checksum}`, \
         current DDL is `{expected_checksum}`; applied migrations are immutable — restore the \
         original DDL and add a new forward migration id"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lock key literal MUST equal the BLAKE3-derived value — so the documented derivation
    /// ("first 8 bytes of blake3(\"myelin.schema.migrate\")") can never silently drift from the
    /// constant. This is a pure-Rust unit test (no DB), so it runs under plain `--features
    /// integration` compilation.
    #[test]
    fn migration_lock_key_matches_digest() {
        let digest = blake3::hash(b"myelin.schema.migrate");
        let bytes = digest.as_bytes();
        let mut first8 = [0u8; 8];
        first8.copy_from_slice(&bytes[..8]);
        assert_eq!(
            MIGRATION_LOCK_KEY,
            i64::from_be_bytes(first8),
            "MIGRATION_LOCK_KEY must equal the first 8 bytes of blake3(\"myelin.schema.migrate\") \
             as a big-endian i64 — update the BYTES literal if blake3 changes"
        );
    }

    /// The checksum is the stable `blake3:<hex>` multihash of the DDL (the platform convention) and
    /// is deterministic + DDL-sensitive (a one-byte change changes it).
    #[test]
    fn ddl_checksum_is_blake3_multihash() {
        let a = ddl_checksum("CREATE TABLE foo (id text)");
        let b = ddl_checksum("CREATE TABLE foo (id text)");
        let c = ddl_checksum("CREATE TABLE foo (id TEXT)");
        assert!(a.starts_with("blake3:"));
        assert_eq!(a, b, "same DDL → same checksum");
        assert_ne!(a, c, "different DDL → different checksum");
    }

    #[test]
    fn identical_recorded_checksum_admits_idempotent_skip() {
        let checksum = ddl_checksum("CREATE TABLE stable (id text PRIMARY KEY)");
        verify_recorded_checksum("0001_stable", &checksum, &checksum)
            .expect("same id and same DDL checksum must remain idempotent");
    }

    #[test]
    fn changed_ddl_under_existing_id_is_loudly_rejected() {
        let recorded = ddl_checksum("CREATE TABLE stable (id text PRIMARY KEY)");
        let current = ddl_checksum("CREATE TABLE stable (id text PRIMARY KEY, body text)");
        let error = verify_recorded_checksum("0001_stable", &recorded, &current)
            .expect_err("same id with different DDL must be rejected");
        let message = error.to_string();
        assert!(message.contains("checksum mismatch"));
        assert!(message.contains("0001_stable"));
        assert!(message.contains(&recorded));
        assert!(message.contains(&current));
        assert!(message.contains("new forward migration id"));
    }

    #[test]
    fn catalog_allows_exact_shared_entries_and_surfaces_incompatible_id_reuse() {
        let first = Migrations::of([crate::migration::Migration::plain(
            "0001_shared",
            "CREATE TABLE shared (id text PRIMARY KEY)",
        )]);
        let exact_subset = first.clone();
        let incompatible = Migrations::of([crate::migration::Migration::plain(
            "0001_shared",
            "CREATE TABLE shared (id text PRIMARY KEY, body text)",
        )]);

        assert!(migration_checksum_collisions(
            [("full", &first), ("exact_subset", &exact_subset),]
        )
        .is_empty());
        let collisions =
            migration_checksum_collisions([("full", &first), ("incompatible", &incompatible)]);
        assert_eq!(collisions.len(), 1);
        assert_eq!(collisions[0].id, "0001_shared");
        assert_eq!(collisions[0].first_set, "full");
        assert_eq!(collisions[0].second_set, "incompatible");
        assert_ne!(collisions[0].first_checksum, collisions[0].second_checksum);
    }
}
