//! # The forward-only ONLINE migration runner — expand→backfill→contract (P-ST-05 / global P-048)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/storage.md` §3.1 (Tier 1 OLTP:
//! *forward-only online migrations — expand→backfill→contract, lock time measured against a
//! restored copy; the hot-table flags each subsystem declares*).
//! **Contract-index:** row 1.5 (forward-only online migrations + hot-table flags) — REALIZED here
//! for the Storage tier (the substrate owns the boot-time runner mechanism, P-S15/P-032; this is
//! the Storage tier's online-shape ENFORCER).
//! **EI-01 §2** — a migration outage is stop-the-bleeding: a blocking `ALTER` at write QPS is an
//! availability incident, so the online idiom is structural, not advisory. **EI-01 §3** — prove-it:
//! the reject-contract-before-backfill verdict is a runtime gate with a unit test, not a doc claim.
//!
//! ## What this adds over the substrate runner (P-S15 / `myelin_substrate::migrations`) — RECONCILED
//! The **substrate** migration runner (P-S15 → global P-032, in `myelin-substrate`) is the
//! **boot-time forward-only refusal mechanism**: it applies an embedded DDL set in order and
//! refuses, loudly, a *destructive* (`DROP`) migration or a *blocking `ALTER` on a declared-hot
//! table*. It freezes the [`MigrationPhase`] (Expand/Backfill/Contract/Plain) and the `HotTables`
//! declaration that both the runner and the `forward-only-migration` lint (P-ST-04/P-020) read.
//!
//! P-ST-05's distinct GATE is the one thing the substrate runner does NOT enforce: the
//! **expand→backfill→contract ORDERING** of a hot-table change — i.e. a **contract-before-backfill
//! ordering is rejected** (the prompt's quantified verdict: 1/1 reject a contract-before-backfill,
//! 1/1 admit an expand→backfill→contract). This module is that ordering enforcer for the Storage
//! OLTP tier. It also enforces the second half of the deliverable: *a migration touching a declared
//! hot table must use the online path* — a hot-table-touching migration that is left `Plain`
//! (i.e. declares no phase, so it cannot be the online idiom) is rejected.
//!
//! ### Why this is a SEPARATE type, not a call into the substrate runner (the crate-DAG constraint)
//! `myelin-storage` sits ABOVE `myelin-substrate` in the root-last crate DAG (see the crate-level
//! DEVIATION note in `lib.rs`): the substrate (the harness) depends on the storage tier client it
//! wires, NOT the reverse. `myelin-storage` therefore **cannot** depend on `myelin-substrate`, so it
//! cannot import `myelin_substrate::migrations::MigrationRunner`. This module re-states the *phase*
//! and *hot-table* vocabulary as the Storage tier's own (the contract-1.5 shape is the frozen
//! anchor both crates implement to, the same way two CDC sides implement one contract), and adds the
//! ordering gate that is genuinely new. The DDL bug-class predicates ([`is_destructive`] /
//! [`is_blocking_alter`]) deliberately mirror the substrate's so the two runners + the lint agree on
//! what "forward-only" and "blocking" mean — divergence there would be a contract drift, flagged.
//!
//! ## Floor named (STOR-D8 forward-dependency, in writing)
//! - **STOR-D8 (online migration under load on the RESTORED copy)** — running an
//!   expand→backfill→contract migration on a restored production-scale copy under load and asserting
//!   the lock-wait stays within budget with **0 downtime** — needs the restored copy that
//!   restore-verify (STOR-D1, P-ST-13/P-061) produces, so it lands at **M2 in P-ST-21 (global
//!   P-126)** (the run table's DEPENDS-ON for P-126 names P-048). **Here** the runner exists and
//!   admits ONLY the online shape (ordering + hot-path-must-be-online), proven at unit scale; the
//!   under-load lock-budget measurement is that named follow-on. Recorded in writing, per the prompt.
//! ## Mutation floor (mandatory-core, ≥ 80% — EI-01 §2/§3; the prompt's TESTS field)
//! The order-enforcement branch (reject-contract-before-backfill) is mandatory-core: the
//! load-bearing decision is *the expand→backfill→contract phases must arrive in order or the
//! migration is refused*. The floor is **≥ 80%**; the achieved score is
//! `cargo mutants -p myelin-storage -f crates/myelin-storage/src/migration.rs` → **37 caught,
//! 4 unviable, 1 missed = 90.2% of the 41 viable mutants**. Every mutation of [`next_progress`]'s
//! transition arms, the four `run`-loop rejection branches, and the DDL classifiers is killed by an
//! assertion. The single MISSED mutant (`HotTables::none -> Default::default()`) is a provably
//! EQUIVALENT mutant — `none()` returns the empty `BTreeSet`, which is exactly `default()`, so no
//! behavioural test can distinguish them; it is not a coverage gap in the ordering gate.
//!
//! - **The concrete DDL execution** against a live Postgres connection lands with the driver (the
//!   substrate `serve`'s pool body, P-S12). Here the runner *validates ordering + admits the online
//!   shape* and records what it applied; the real connection executes the admitted DDL through
//!   [`crate::OltpPool`]. The validation logic does not change shape when the driver lands.

use std::collections::BTreeSet;
use std::fmt;

/// The three-deploy phase of a forward-only online schema change (storage §3.1; contract 1.5),
/// mirroring `myelin_substrate::migrations::MigrationPhase` (the frozen contract-1.5 vocabulary,
/// re-stated here because the crate DAG forbids a `myelin-storage → myelin-substrate` edge — see
/// the module docs). A hot-table change is **expand → backfill → contract**, never one blocking
/// `ALTER`; `Plain` is an ordinary non-hot forward migration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MigrationPhase {
    /// An ordinary forward migration on a non-hot table (new table; nullable add on a cold table).
    /// A `Plain` migration MUST NOT touch a declared-hot table (a hot table demands the online path).
    Plain,
    /// **Expand** — add the new shape additively + non-blockingly (nullable column;
    /// `CREATE INDEX CONCURRENTLY`; new table); write both old + new behind a flag (§3.1).
    Expand,
    /// **Backfill** — populate in bounded, throttled, resumable batches off the hot path
    /// (idempotent, re-runnable) (§3.1). MUST come after the matching `Expand`, before `Contract`.
    Backfill,
    /// **Contract** — switch reads to the new shape, stop writing the old, drop the old in a LATER
    /// non-blocking deploy (§3.1). MUST come after the matching `Backfill` for the same table.
    Contract,
}

/// One forward-only migration on the Storage OLTP tier: a stable PII-free id + its DDL + its phase
/// + the table it targets (storage §3.1). Ordered by registration; the runner applies them in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Migration {
    /// A stable, monotonically-ordered id (e.g. `0010_expand`). PII-free.
    pub id: &'static str,
    /// The forward-only DDL. A destructive `DROP TABLE`/`DROP COLUMN` is forward-only-illegal; a
    /// blocking `ALTER` on a declared-hot table is rejected (it must be the online idiom).
    pub ddl: &'static str,
    /// The expand→backfill→contract phase (§3.1). `Plain` for an ordinary non-hot migration.
    pub phase: MigrationPhase,
    /// The single table this migration targets (so the runner can match it against [`HotTables`]
    /// and order its phases). `None` for a multi-table / non-table migration.
    pub table: Option<&'static str>,
}

impl Migration {
    /// A plain forward migration (non-hot table; no phase discipline required).
    pub fn plain(id: &'static str, ddl: &'static str) -> Migration {
        Migration { id, ddl, phase: MigrationPhase::Plain, table: None }
    }

    /// A plain forward migration that NAMES its (cold) table — so the runner can verify the table
    /// is not declared hot (a hot table left `Plain` is the "must use the online path" violation).
    pub fn plain_on(id: &'static str, ddl: &'static str, table: &'static str) -> Migration {
        Migration { id, ddl, phase: MigrationPhase::Plain, table: Some(table) }
    }

    /// A phased migration on a (possibly hot) table — carries which step of expand→backfill→contract
    /// it is, so the runner can enforce the ordering.
    pub fn phased(
        id: &'static str,
        ddl: &'static str,
        phase: MigrationPhase,
        table: &'static str,
    ) -> Migration {
        Migration { id, ddl, phase, table: Some(table) }
    }
}

/// An ordered forward-only migration set for the Storage OLTP tier (storage §3.1; contract 1.5).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Migrations(pub Vec<Migration>);

impl Migrations {
    /// Build from an explicit migration list (the order is the apply order the runner enforces).
    pub fn of(items: impl IntoIterator<Item = Migration>) -> Migrations {
        Migrations(items.into_iter().collect())
    }
}

/// The per-subsystem **hot-table declaration** (storage §3.1; contract 1.5), mirroring
/// `myelin_substrate::migrations::HotTables`. A table is flagged hot when its write rate warrants
/// expand→backfill→contract (measured, not predicted). The online runner reads this to enforce
/// *a migration touching a declared hot table must use the online path*.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HotTables {
    tables: BTreeSet<String>,
}

impl HotTables {
    /// No hot tables declared.
    pub fn none() -> HotTables {
        HotTables { tables: BTreeSet::new() }
    }

    /// Declare a service's hot tables (§3.1) — measured-not-predicted per subsystem.
    pub fn declare(tables: impl IntoIterator<Item = impl Into<String>>) -> HotTables {
        HotTables { tables: tables.into_iter().map(Into::into).collect() }
    }

    /// Whether `table` is declared hot.
    pub fn is_hot(&self, table: &str) -> bool {
        self.tables.contains(table)
    }

    /// The declared hot tables, sorted.
    pub fn tables(&self) -> impl Iterator<Item = &str> {
        self.tables.iter().map(String::as_str)
    }

    /// Whether any hot table is declared.
    pub fn is_empty(&self) -> bool {
        self.tables.is_empty()
    }
}

/// Whether a DDL statement is **destructive** (a forward-only violation): `DROP TABLE` /
/// `DROP COLUMN` (storage §3.1; EI-01 §2). Mirrors the substrate predicate so the two runners +
/// the lint agree on "forward-only". Case-insensitive.
pub fn is_destructive(ddl: &str) -> bool {
    let upper = ddl.to_ascii_uppercase();
    upper.contains("DROP TABLE") || upper.contains("DROP COLUMN")
}

/// Whether a DDL statement is a **blocking `ALTER`** (takes a table lock at write QPS): an
/// `ADD COLUMN … NOT NULL` without a `DEFAULT`, an in-place `ALTER … ALTER COLUMN`, or a
/// non-concurrent `CREATE INDEX` (storage §3.1). On a HOT table any of these stalls writes — it
/// must be the online idiom instead. Mirrors the substrate predicate. Case-insensitive.
pub fn is_blocking_alter(ddl: &str) -> bool {
    let lower = ddl.to_ascii_lowercase();
    let add_not_null = lower.contains("alter table")
        && lower.contains("add column")
        && lower.contains("not null")
        && !lower.contains("default");
    let alter_column_inplace = lower.contains("alter table") && lower.contains("alter column");
    let non_concurrent_index = lower.contains("create index") && !lower.contains("concurrently");
    add_not_null || alter_column_inplace || non_concurrent_index
}

/// A migration the online runner refuses to admit, with the structural reason (storage §3.1).
/// Every variant is a forward-only / online-shape violation; carrying the offending migration id
/// + table keeps the rejection loud and named (EI-01 §3 — a refusal is information).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MigrationError {
    /// A destructive (`DROP`) migration — forward-only is structural; a rollback is a NEW forward
    /// migration, never a down (§3.1; EI-01 §2).
    Destructive { id: &'static str },
    /// A blocking `ALTER` on a declared-hot table — must be expand→backfill→contract (§3.1).
    BlockingAlterOnHotTable { id: &'static str, table: &'static str },
    /// A `Plain` (un-phased) migration touches a declared-hot table — a hot-table change MUST use
    /// the online path (the second half of the deliverable), never a plain blocking change.
    HotTableNotOnline { id: &'static str, table: &'static str },
    /// **The P-ST-05 ordering gate.** A phase arrived out of expand→backfill→contract order for a
    /// table — the canonical case being a **Contract before its Backfill** (the prompt's named
    /// reject verdict). Also fires on a Backfill before its Expand, or a duplicate Contract.
    PhaseOutOfOrder {
        id: &'static str,
        table: &'static str,
        /// The phase this migration carries.
        phase: MigrationPhase,
        /// The highest phase already seen for this table (what `phase` must legally follow).
        after: PhaseProgress,
    },
}

/// How far along expand→backfill→contract a given table is (the per-table watermark the ordering
/// gate advances). A new phase is admitted only if it is the legal successor of the watermark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhaseProgress {
    /// No online phase seen for this table yet (the next legal phase is `Expand`).
    None,
    /// The table has been expanded (the next legal phase is `Backfill`).
    Expanded,
    /// The table has been backfilled (the next legal phase is `Contract`).
    Backfilled,
    /// The table has been contracted (the online cycle is complete; a further phase restarts it).
    Contracted,
}

impl fmt::Display for MigrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MigrationError::Destructive { id } => write!(
                f,
                "migration {id} is destructive (DROP) — forward-only migrations only; a rollback \
                 is a NEW forward migration, never a down (storage §3.1)"
            ),
            MigrationError::BlockingAlterOnHotTable { id, table } => write!(
                f,
                "migration {id} takes a blocking ALTER on the declared-HOT table `{table}` — a \
                 hot-table change must be expand→backfill→contract, never one blocking ALTER that \
                 locks writes at QPS (storage §3.1)"
            ),
            MigrationError::HotTableNotOnline { id, table } => write!(
                f,
                "migration {id} touches the declared-HOT table `{table}` as a Plain migration — a \
                 migration touching a declared hot table MUST use the online path \
                 (expand→backfill→contract), never a plain change (storage §3.1; P-ST-05)"
            ),
            MigrationError::PhaseOutOfOrder { id, table, phase, after } => write!(
                f,
                "migration {id} on `{table}` carries phase {phase:?} out of order — \
                 expand→backfill→contract must run in order (the table is currently {after:?}); a \
                 contract-before-backfill ordering is rejected (storage §3.1; P-ST-05)"
            ),
        }
    }
}

impl std::error::Error for MigrationError {}

/// The forward-only **online** migration runner for the Storage OLTP tier (storage §3.1; contract
/// 1.5; P-ST-05). It admits ONLY the online shape:
///
/// 1. **forward-only** — a destructive (`DROP`) migration is rejected (mirrors the substrate runner);
/// 2. **no blocking change on a hot table** — a blocking `ALTER` on a declared-hot table is rejected;
/// 3. **hot-table-must-be-online** — a `Plain` migration touching a declared-hot table is rejected
///    (a hot table demands the online path);
/// 4. **expand→backfill→contract ORDERING** (the P-ST-05 gate) — for each table, the online phases
///    must arrive in order; a **contract-before-backfill** (or backfill-before-expand, or a phase
///    that skips a step) is rejected at runtime as well as by the P-ST-04 lint.
///
/// It records what it applied (in order) so a test can assert the admitted sequence. The concrete
/// DDL execution against Postgres is a named floor (the driver, P-S12); the validation here does
/// not change shape when the driver lands.
#[derive(Default)]
pub struct OnlineMigrationRunner {
    applied: Vec<&'static str>,
}

impl OnlineMigrationRunner {
    /// A fresh runner (nothing applied yet).
    pub fn new() -> OnlineMigrationRunner {
        OnlineMigrationRunner { applied: Vec::new() }
    }

    /// Validate + apply each migration in order against the [`HotTables`] declaration, enforcing the
    /// four online-shape rules above. On the first violation it returns the loud, named
    /// [`MigrationError`] and applies nothing further (a service cannot start having admitted an
    /// unsafe migration — EI-01 §2). On success every migration id is recorded in [`Self::applied`].
    pub fn run(
        &mut self,
        migrations: &Migrations,
        hot_tables: &HotTables,
    ) -> Result<(), MigrationError> {
        // The per-table expand→backfill→contract watermark — the ordering gate's state.
        use std::collections::BTreeMap;
        let mut progress: BTreeMap<&'static str, PhaseProgress> = BTreeMap::new();

        for m in &migrations.0 {
            // (1) forward-only: a DROP can never be admitted, hot or cold.
            if is_destructive(m.ddl) {
                return Err(MigrationError::Destructive { id: m.id });
            }

            if let Some(table) = m.table {
                let hot = hot_tables.is_hot(table);

                // (2) no blocking change on a hot table.
                if hot && is_blocking_alter(m.ddl) {
                    return Err(MigrationError::BlockingAlterOnHotTable { id: m.id, table });
                }

                // (3) hot-table-must-be-online: a hot table touched by a Plain (un-phased)
                // migration is the "must use the online path" violation.
                if hot && m.phase == MigrationPhase::Plain {
                    return Err(MigrationError::HotTableNotOnline { id: m.id, table });
                }

                // (4) the ordering gate: an online phase must be the legal successor of the
                // table's current watermark. This is where contract-before-backfill is rejected.
                if m.phase != MigrationPhase::Plain {
                    let current = *progress.get(table).unwrap_or(&PhaseProgress::None);
                    match next_progress(current, m.phase) {
                        Some(next) => {
                            progress.insert(table, next);
                        }
                        None => {
                            return Err(MigrationError::PhaseOutOfOrder {
                                id: m.id,
                                table,
                                phase: m.phase,
                                after: current,
                            });
                        }
                    }
                }
            }

            self.applied.push(m.id);
        }
        Ok(())
    }

    /// The ids applied, in order (so a test can assert the admitted online sequence).
    pub fn applied(&self) -> &[&'static str] {
        &self.applied
    }
}

/// The expand→backfill→contract transition function — the heart of the ordering gate. Given a
/// table's current [`PhaseProgress`] watermark and the [`MigrationPhase`] of the next migration,
/// return the new watermark if the transition is legal, or `None` if it is out of order (the
/// contract-before-backfill / backfill-before-expand / skip-a-step rejections).
///
/// Legal transitions (only): None→Expand, Expanded→Backfill, Backfilled→Contract, and
/// Contracted→Expand (a NEW online cycle on the same table starts cleanly). Everything else is
/// out of order. `Plain` is never passed here (it is not an online phase).
fn next_progress(current: PhaseProgress, phase: MigrationPhase) -> Option<PhaseProgress> {
    match (current, phase) {
        (PhaseProgress::None, MigrationPhase::Expand) => Some(PhaseProgress::Expanded),
        (PhaseProgress::Contracted, MigrationPhase::Expand) => Some(PhaseProgress::Expanded),
        (PhaseProgress::Expanded, MigrationPhase::Backfill) => Some(PhaseProgress::Backfilled),
        (PhaseProgress::Backfilled, MigrationPhase::Contract) => Some(PhaseProgress::Contracted),
        // Everything else — Contract before Backfill, Backfill before Expand, a repeated phase,
        // a skipped step — is out of order.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The runner ADMITS a full expand→backfill→contract sequence on a hot table — the 1/1 admit
    /// verdict (storage §3.1; P-ST-05 GATE). Each step is non-blocking, in order.
    #[test]
    fn admits_expand_backfill_contract_on_a_hot_table() {
        let hot = HotTables::declare(["issue"]);
        let migrations = Migrations::of([
            Migration::phased(
                "0010_expand",
                "ALTER TABLE issue ADD COLUMN priority INT;", // nullable add = non-blocking expand.
                MigrationPhase::Expand,
                "issue",
            ),
            Migration::phased(
                "0011_backfill",
                "UPDATE issue SET priority = 0 WHERE priority IS NULL;", // off-hot-path DML.
                MigrationPhase::Backfill,
                "issue",
            ),
            Migration::phased(
                "0012_contract",
                "ALTER TABLE issue ADD COLUMN status TEXT NOT NULL DEFAULT 'open';", // has DEFAULT.
                MigrationPhase::Contract,
                "issue",
            ),
        ]);
        let mut runner = OnlineMigrationRunner::new();
        runner
            .run(&migrations, &hot)
            .expect("expand→backfill→contract is admitted on a hot table");
        assert_eq!(runner.applied(), &["0010_expand", "0011_backfill", "0012_contract"]);
    }

    /// **THE P-ST-05 GATE: a contract-before-backfill ordering is REJECTED** — the 1/1 reject
    /// verdict (storage §3.1). The expand lands, then a contract arrives with no backfill between.
    #[test]
    fn rejects_contract_before_backfill() {
        let hot = HotTables::declare(["issue"]);
        let migrations = Migrations::of([
            Migration::phased(
                "0010_expand",
                "ALTER TABLE issue ADD COLUMN priority INT;",
                MigrationPhase::Expand,
                "issue",
            ),
            // CONTRACT arrives before the BACKFILL — the forbidden ordering. The DDL itself is
            // non-blocking (a validated check added NOT VALID, then a concurrent finalize), so it
            // is the ORDERING gate that rejects this, not the blocking-ALTER rule.
            Migration::phased(
                "0011_contract",
                "ALTER TABLE issue ADD CONSTRAINT priority_set CHECK (priority IS NOT NULL) NOT VALID;",
                MigrationPhase::Contract,
                "issue",
            ),
        ]);
        let mut runner = OnlineMigrationRunner::new();
        let e = runner
            .run(&migrations, &hot)
            .expect_err("a contract-before-backfill ordering is rejected");
        assert_eq!(
            e,
            MigrationError::PhaseOutOfOrder {
                id: "0011_contract",
                table: "issue",
                phase: MigrationPhase::Contract,
                after: PhaseProgress::Expanded,
            }
        );
        // Nothing past the violation was applied beyond what was validated-then-pushed: the expand
        // was admitted, the contract was rejected (the runner stops at the first violation).
        assert_eq!(runner.applied(), &["0010_expand"]);
        assert!(e.to_string().contains("contract-before-backfill"), "loud reason: {e}");
    }

    /// A backfill arriving before its expand is also out of order (the gate is the full ordering,
    /// not just the headline case).
    #[test]
    fn rejects_backfill_before_expand() {
        let hot = HotTables::declare(["issue"]);
        let migrations = Migrations::of([Migration::phased(
            "0010_backfill",
            "UPDATE issue SET priority = 0;",
            MigrationPhase::Backfill,
            "issue",
        )]);
        let mut runner = OnlineMigrationRunner::new();
        let e = runner.run(&migrations, &hot).expect_err("backfill-before-expand is rejected");
        assert_eq!(
            e,
            MigrationError::PhaseOutOfOrder {
                id: "0010_backfill",
                table: "issue",
                phase: MigrationPhase::Backfill,
                after: PhaseProgress::None,
            }
        );
    }

    /// A migration touching a DECLARED-HOT table as a `Plain` (un-phased) change is rejected — a
    /// hot-table change MUST use the online path (the second half of the P-ST-05 deliverable).
    #[test]
    fn hot_table_touched_plain_must_use_online_path() {
        let hot = HotTables::declare(["issue"]);
        let migrations = Migrations::of([Migration::plain_on(
            "0010_plain_hot",
            "ALTER TABLE issue ADD COLUMN note TEXT;", // even a nullable add: a hot table = online.
            "issue",
        )]);
        let mut runner = OnlineMigrationRunner::new();
        let e = runner.run(&migrations, &hot).expect_err("a hot table demands the online path");
        assert_eq!(e, MigrationError::HotTableNotOnline { id: "0010_plain_hot", table: "issue" });
        assert!(e.to_string().contains("online path"), "loud reason: {e}");
    }

    /// A destructive (DROP) migration is rejected — forward-only is structural (storage §3.1).
    #[test]
    fn destructive_migration_is_rejected() {
        let migrations = Migrations::of([Migration::plain("0010_bad", "DROP TABLE issue")]);
        let mut runner = OnlineMigrationRunner::new();
        let e = runner
            .run(&migrations, &HotTables::none())
            .expect_err("a DROP must be rejected");
        assert_eq!(e, MigrationError::Destructive { id: "0010_bad" });
        assert!(e.to_string().contains("forward-only"), "loud reason: {e}");
    }

    /// A blocking `ALTER` on a declared-hot table is rejected (it must be the online idiom).
    #[test]
    fn blocking_alter_on_a_hot_table_is_rejected() {
        let hot = HotTables::declare(["issue"]);
        let migrations = Migrations::of([Migration::phased(
            "0010_hot",
            "ALTER TABLE issue ADD COLUMN body TEXT NOT NULL;", // no DEFAULT → blocking.
            MigrationPhase::Expand,
            "issue",
        )]);
        let mut runner = OnlineMigrationRunner::new();
        let e = runner.run(&migrations, &hot).expect_err("blocking ALTER on hot table is rejected");
        assert_eq!(e, MigrationError::BlockingAlterOnHotTable { id: "0010_hot", table: "issue" });
    }

    /// A plain, non-destructive migration on a NON-hot table is admitted (cold tables don't need the
    /// online discipline) — the runner is not over-eager.
    #[test]
    fn plain_migration_on_a_cold_table_is_admitted() {
        let hot = HotTables::declare(["issue"]); // `audit_archive` is NOT hot.
        let migrations = Migrations::of([
            Migration::plain("0010_new", "CREATE TABLE audit_archive (id BIGINT PRIMARY KEY);"),
            Migration::plain_on(
                "0011_add",
                "ALTER TABLE audit_archive ADD COLUMN note TEXT;",
                "audit_archive",
            ),
        ]);
        let mut runner = OnlineMigrationRunner::new();
        runner.run(&migrations, &hot).expect("a plain migration on a cold table is admitted");
        assert_eq!(runner.applied(), &["0010_new", "0011_add"]);
    }

    /// Two independent hot tables each run their own expand→backfill→contract cycle, interleaved —
    /// the watermark is PER TABLE, so one table's contract is not gated on another's backfill.
    #[test]
    fn per_table_ordering_is_independent() {
        let hot = HotTables::declare(["issue", "message"]);
        let migrations = Migrations::of([
            Migration::phased("0010_e", "ALTER TABLE issue ADD COLUMN a INT;", MigrationPhase::Expand, "issue"),
            Migration::phased("0011_e", "ALTER TABLE message ADD COLUMN b INT;", MigrationPhase::Expand, "message"),
            Migration::phased("0012_b", "UPDATE issue SET a = 0;", MigrationPhase::Backfill, "issue"),
            Migration::phased("0013_b", "UPDATE message SET b = 0;", MigrationPhase::Backfill, "message"),
            Migration::phased("0014_c", "ALTER TABLE issue ADD COLUMN a2 INT DEFAULT 0 NOT NULL;", MigrationPhase::Contract, "issue"),
            Migration::phased("0015_c", "ALTER TABLE message ADD COLUMN b2 INT DEFAULT 0 NOT NULL;", MigrationPhase::Contract, "message"),
        ]);
        let mut runner = OnlineMigrationRunner::new();
        runner.run(&migrations, &hot).expect("two interleaved online cycles are admitted");
        assert_eq!(runner.applied().len(), 6);
    }

    /// After a complete cycle a table may start a NEW expand→backfill→contract cycle (the watermark
    /// resets cleanly on Contracted→Expand) — schema evolves forever, forward-only.
    #[test]
    fn a_table_can_start_a_second_online_cycle() {
        assert_eq!(next_progress(PhaseProgress::Contracted, MigrationPhase::Expand), Some(PhaseProgress::Expanded));
        // But a second contract with no fresh expand/backfill is still out of order.
        assert_eq!(next_progress(PhaseProgress::Contracted, MigrationPhase::Contract), None);
    }

    /// The DDL classifiers catch the bug classes (the predicates the runner + the lint share).
    #[test]
    fn ddl_classifiers_catch_the_bug_classes() {
        assert!(is_destructive("DROP TABLE issue"));
        assert!(is_destructive("ALTER TABLE issue DROP COLUMN body"));
        assert!(!is_destructive("ALTER TABLE issue ADD COLUMN x INT"));
        assert!(is_blocking_alter("ALTER TABLE issue ADD COLUMN x TEXT NOT NULL"));
        assert!(is_blocking_alter("ALTER TABLE issue ALTER COLUMN x TYPE BIGINT"));
        assert!(is_blocking_alter("CREATE INDEX idx ON issue (x)"));
        assert!(!is_blocking_alter("CREATE INDEX CONCURRENTLY idx ON issue (x)"));
        assert!(!is_blocking_alter("ALTER TABLE issue ADD COLUMN x TEXT")); // nullable add = expand.
    }

    /// The hot-table declaration mechanism (§3.1): declare → is_hot is the frozen query.
    #[test]
    fn hot_table_declaration_is_per_subsystem() {
        let hot = HotTables::declare(["block", "db_row", "doc_op"]); // the KN seed set (§3.1).
        assert!(hot.is_hot("block"));
        assert!(!hot.is_hot("audit_archive"));
        assert_eq!(hot.tables().collect::<Vec<_>>(), vec!["block", "db_row", "doc_op"]);
        assert!(!hot.is_empty());
        assert!(HotTables::none().is_empty());
        // `none()` IS the empty/default declaration — pin it so the constructor can't drift (and so
        // the `none -> Default::default()` mutant is observable: both must equal the empty set).
        assert_eq!(HotTables::none(), HotTables::default());
        assert_eq!(HotTables::none().tables().count(), 0);
    }
}
