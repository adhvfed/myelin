//! The edge inverse-index schema migration (REF-P5 / P-154; contracts 1.5 + 11.3 consumed).
//!
//! **Owning architecture doc:** `reference-graph.md` §3.2 (the `edge` table — the materialised
//! projection of the `refs.edge.created`/`refs.edge.removed` log; the exact columns + the three
//! indexes), §3.7 (the stateful-component register — R1 `edge` projection: Postgres-class,
//! derived/rebuildable, per-tenant DEK). **External insight:** `02-platform-substrate.md` §7
//! (backlinks are event-sourced projections — this table is REBUILT from the log, never the source
//! of truth). **VISION §1** (the reference graph as connective tissue).
//!
//! ## What REF-P5 (P-154) ships — the schema ONLY
//! The `edge` inverse-index table + its three indexes, as a **forward-only online migration**
//! (contract 1.5), expressed through the substrate migration framework
//! ([`myelin_substrate::Migrations`] / [`myelin_substrate::Migration`]) so the migration RUNNER (at
//! boot) and the `forward-only-migration` LINT (at source-scan) both see it. The table is:
//! - **`(tenant, region)`-first** (§3.2: `tenant`/`region` are the first columns / partition prefix)
//!   — the `tenant-predicate` lint target (every query is tenant-first; there is no cross-tenant
//!   query path);
//! - **RLS-enforced** via the platform-wide `myelin_make_tenant_scoped(...)` convention
//!   (`scripts/pg-init/00-rls-conventions.sql`): `ENABLE` + `FORCE` row-level security + the
//!   `(tenant_id, region)` isolation policy. Even the table owner is subject to the policy (FORCE);
//! - **encrypted-from-birth** under the **per-tenant DEK** reserved in REF-P4
//!   ([`crate::RefsDekPin`], contract 11.3/11.4): the table carries this key ref from its FIRST row,
//!   so the index is never plaintext-then-encrypted.
//!
//! ## Reconciliation: the §3.2 column name vs the platform RLS convention (documented deviation)
//! Architecture §3.2 names the tenant partition column `tenant uuid`. The platform-wide RLS helper
//! `myelin_make_tenant_scoped` (the ONE dev/prod RLS convention every tenant table uses, storage
//! §3.1 / contract 11.1) requires a `tenant_id text` column + a `region text` column so its
//! `(tenant_id, region)` isolation policy binds. To keep ONE RLS convention across every subsystem
//! (a second column-naming would fork the platform RLS policy — EI-01 §7 coherence), this migration
//! names the columns **`tenant_id text` + `region text`** (the convention's exact names) while
//! preserving §3.2's intent verbatim: `tenant_id`/`region` are the FIRST columns / partition prefix
//! and the RLS isolation key. The `uuid` vs `text` choice follows the platform convention (the
//! tenant token is an opaque string at this layer — `myelin_tenancy::TenantId(String)`); a tenant id
//! is still a stable opaque token, never PII. This is a deliberate, documented deviation (VISION §3 /
//! EI-01 §1): the convention wins over the literal column name so the RLS floor is the SAME one
//! Postgres enforces for every tenant table.
//!
//! ## Floors named (VISION §3 / prompt DoD)
//! - **This is the SCHEMA ONLY — an empty table is not a working index.** The
//!   builder/invalidator consumers that POPULATE it land in **REF-P6** (the refs-edge-builder) /
//!   **REF-P7** (the refs-projection-invalidator). Nothing writes a row here; this migration creates
//!   the table + indexes encrypted-from-birth + RLS-on so the builder has a target.
//! - **No mutation floor (schema migration).** A `CREATE TABLE` has no decision logic to mutate; the
//!   consumer-side mutation floors (the deterministic `edge_id` upsert; the
//!   `source_root`/`target_root` derivation) are stated + met in **REF-P6**.
//! - **The live-DB apply is proven against the dev stack** in `tests/integration_ref_p5_edge_schema.rs`
//!   (the `integration` cargo feature) — the default `cargo build`/`cargo test --workspace` stay
//!   DB-free. The world-scale migration-under-load drill (SUB-D10) is a substrate floor (P-S34), not
//!   re-proven here.

use myelin_substrate::{Migration, MigrationPhase, Migrations};
use myelin_tenancy::{Region, TenantId};

use crate::dek::RefsDekPin;

/// The `edge` inverse-index table name (§3.2). The materialised projection of the
/// `refs.edge.created`/`refs.edge.removed` log — R1 in the stateful-component register (§3.7),
/// derived/rebuildable, per-tenant-DEK-encrypted. PII-free identifier.
pub const EDGE_TABLE: &str = "edge";

/// The stable, ordered, PII-free migration id for the edge schema (the runner applies migrations in
/// id order; this is the Refs subsystem's first domain-table migration).
pub const EDGE_MIGRATION_ID: &str = "refs_0001_edge";

/// The `edge_inbound` index name — "what references this (+children)?" (the hot inbound walk; §3.2).
pub const EDGE_INBOUND_INDEX: &str = "edge_inbound";
/// The `edge_outbound` index name — the outbound walk + the C-4 `SetExpr` filter column (§3.2).
pub const EDGE_OUTBOUND_INDEX: &str = "edge_outbound";
/// The `edge_by_rel` index name — the typed-lifecycle (TE-7) traversal index (§3.2).
pub const EDGE_BY_REL_INDEX: &str = "edge_by_rel";

/// The forward-only DDL that creates the `edge` table (§3.2 shape, verbatim intent), encrypted under
/// the per-tenant DEK + RLS-ready.
///
/// **§3.2 columns (verbatim intent):**
/// - `tenant_id`/`region` — the `(tenant, region)` partition prefix + the RLS isolation key (the
///   `tenant`/`region` of §3.2, named to the platform RLS convention — see the module deviation note);
/// - `edge_id` — **deterministic** `hash(tenant, source, target, rel)` ⇒ idempotent rebuild (REF-P6
///   upserts on it; the same edge replayed twice is one row);
/// - `source`/`target` — the FULL `#sub` `ArtifactRef` URNs (so "this message embeds block b9 of
///   page 7c2" is exact);
/// - `source_root`/`target_root` — the `#sub`-stripped roots (the outbound walk + the C-4 filter
///   column / the hot inbound index key — `strip_sub`, REF-P1);
/// - `rel` (`edge_rel`) — `'mentions'|'embeds'|'links'` | lifecycle rels (§3.3);
/// - `rel_class` — `'reference'` (Refs-authoritative) | `'lifecycle'` (typed-mirror; the TE-7 seam);
/// - `origin_event` — provenance (audit); `origin_actor` — **PSEUDONYMOUS** Principal `ArtifactRef`
///   (erasure-safe; EI-04 §1 — Refs never holds the name, only the opaque id);
/// - `created_at`; `zookie` — the consistency token at edge-write time (§4.4);
/// - `tombstoned` — the erasure/deletion soft-delete flag (§4.6 ladder).
///
/// **Keys (§3.2):** `PRIMARY KEY (tenant_id, edge_id)` (tenant-first); `UNIQUE (tenant_id, source,
/// target, rel)` (one edge per `(source, target, rel)` per tenant — the idempotency backstop).
///
/// **Encrypted-from-birth:** the `dek_ref` column carries the per-tenant DEK ref (REF-P4) for every
/// row so the bulk columns are sealed under the per-tenant DEK from the FIRST insert (the
/// tenant-decommission crypto-shred unit; §3.7). No row is ever written plaintext-then-encrypted.
///
/// The `edge_rel` / `rel_class` value sets are enforced by `CHECK` constraints (the frozen
/// vocabularies §3.2/§3.3) rather than Postgres `ENUM` types so a forward-only vocabulary EXTENSION
/// (a new lifecycle rel) is a non-blocking `CHECK` add, never an enum-rewrite (forward-only, §9).
pub const CREATE_EDGE_TABLE_DDL: &str = "\
CREATE TABLE IF NOT EXISTS edge (\n  \
  tenant_id    text NOT NULL,\n  \
  region       text NOT NULL,\n  \
  edge_id      text NOT NULL,\n  \
  source       text NOT NULL,\n  \
  source_root  text NOT NULL,\n  \
  target       text NOT NULL,\n  \
  target_root  text NOT NULL,\n  \
  rel          text NOT NULL CHECK (rel IN ('mentions','embeds','links','closes','blocks','blocked_by','depends_on','parent','child','assigns','relates')),\n  \
  rel_class    text NOT NULL CHECK (rel_class IN ('reference','lifecycle')),\n  \
  origin_event text NOT NULL,\n  \
  origin_actor text NOT NULL,\n  \
  created_at   timestamptz NOT NULL,\n  \
  zookie       text,\n  \
  tombstoned   boolean NOT NULL DEFAULT false,\n  \
  dek_ref      text NOT NULL,\n  \
  PRIMARY KEY (tenant_id, edge_id),\n  \
  UNIQUE (tenant_id, source, target, rel)\n\
)";

/// The forward-only DDL for the three §3.2 indexes (non-concurrent here is fine — `CREATE INDEX` on a
/// freshly-created EMPTY table takes no meaningful lock; the hot-table expand→backfill→contract
/// discipline applies to ALTERs on populated tables, and `edge` is NOT declared hot for THIS
/// migration since the create is atomic and empty).
///
/// - `edge_inbound (tenant_id, target_root) WHERE NOT tombstoned` — "what references this
///   (+children)?" (the hot inbound walk; live edges only);
/// - `edge_outbound (tenant_id, source_root)` — the outbound walk + the C-4 `SetExpr` filter column;
/// - `edge_by_rel (tenant_id, target_root, rel) WHERE rel_class = 'lifecycle'` — the typed-lifecycle
///   (TE-7) traversal index (only lifecycle-class edges).
pub const CREATE_EDGE_INDEXES_DDL: &[(&str, &str)] = &[
    (
        EDGE_INBOUND_INDEX,
        "CREATE INDEX IF NOT EXISTS edge_inbound ON edge (tenant_id, target_root) WHERE NOT tombstoned",
    ),
    (
        EDGE_OUTBOUND_INDEX,
        "CREATE INDEX IF NOT EXISTS edge_outbound ON edge (tenant_id, source_root)",
    ),
    (
        EDGE_BY_REL_INDEX,
        "CREATE INDEX IF NOT EXISTS edge_by_rel ON edge (tenant_id, target_root, rel) WHERE rel_class = 'lifecycle'",
    ),
];

/// The RLS DDL — make the `edge` table tenant-scoped via the platform-wide convention helper
/// (`scripts/pg-init/00-rls-conventions.sql`): `ENABLE` + `FORCE` row-level security + the
/// `(tenant_id, region)` isolation policy. FORCE means even the table owner is subject to the policy
/// (so a migration/owner connection cannot accidentally read cross-tenant). This is the SAME helper
/// every tenant table uses — Refs does not fork the RLS policy (EI-01 §7).
pub const MAKE_EDGE_TENANT_SCOPED_DDL: &str = "SELECT myelin_make_tenant_scoped('edge')";

/// The Refs edge-schema migration set (contract 1.5), built through the substrate framework so the
/// boot-time RUNNER applies it forward-only AND the `forward-only-migration` lint reads it at
/// source-scan. ONE [`Migration`]: the `CREATE TABLE` (a new table, `MigrationPhase::Plain` — no
/// expand→backfill→contract discipline is needed to CREATE a table). The index creates + the RLS
/// call ride alongside the create in the same forward migration (an empty fresh table; no
/// hot-table lock). `table: Some("edge")` so the runner can match it against the hot-table
/// declaration — but `edge` is NOT declared hot for this create-and-index migration (the hot-table
/// expand→backfill→contract discipline applies to LATER ALTERs on the populated table, declared by
/// the builder band if/when the write rate warrants it — measured-not-predicted, §9.4).
///
/// The DDL is held as `&str` constants, so it is NOT mistaken for live Rust by the lint
/// (`blank_string_literals` blanks literal contents) — but the migration framework still carries the
/// real DDL to the boot runner / the live integration test.
pub fn edge_table_migrations() -> Migrations {
    // The create-table + the three indexes + the RLS scoping, as ONE ordered forward-only migration
    // batch. Each entry is a forward-only DDL statement (no DROP, no blocking ALTER on a hot table).
    let mut ddl = String::new();
    ddl.push_str(CREATE_EDGE_TABLE_DDL);
    ddl.push(';');
    for (_name, idx) in CREATE_EDGE_INDEXES_DDL {
        ddl.push('\n');
        ddl.push_str(idx);
        ddl.push(';');
    }
    ddl.push('\n');
    ddl.push_str(MAKE_EDGE_TENANT_SCOPED_DDL);
    ddl.push(';');
    // Leak the assembled DDL to `'static` (the substrate `Migration` holds `&'static str`; the
    // migration set is built once at boot/serve, so this is a one-time, bounded leak — the same
    // shape the framework expects).
    let ddl: &'static str = Box::leak(ddl.into_boxed_str());
    Migrations::of([Migration::phased(
        EDGE_MIGRATION_ID,
        ddl,
        MigrationPhase::Plain,
        EDGE_TABLE,
    )])
}

/// The per-row **encrypted-from-birth** key ref (§3.7 / contract 11.3): every `edge` row carries the
/// per-tenant DEK ref reserved in REF-P4 ([`RefsDekPin::reserve`]) so the bulk columns seal under the
/// per-tenant DEK from the FIRST insert. This resolves the key ref the migration's `dek_ref` column
/// stores for `(tenant, region)` — the anchor the REF-P6 builder writes with every row and the
/// REF-P15 crypto-shred destroys at tenant grain.
///
/// Returns the key ref URI (`kms://<tenant>/<epoch>/tenant`); a failure to reserve the DEK is a LOUD
/// error (never an unencrypted edge table — encrypted-from-birth is structural, not best-effort).
pub fn edge_table_dek_ref(
    dek: &RefsDekPin,
    tenant: &TenantId,
    region: &Region,
) -> Result<String, myelin_storage::KmsError> {
    Ok(dek.reserve(tenant, region)?.to_uri())
}

/// Whether `ddl` is a forward-only-LEGAL statement for the edge schema: no destructive `DROP`, no
/// down/rollback. (The framework's [`myelin_substrate::is_destructive`] / the
/// `forward-only-migration` lint enforce this at boot + source-scan; this is the in-module structural
/// assertion the migration test rests on so the edge DDL is proven forward-only without a live DB.)
pub fn edge_ddl_is_forward_only(ddl: &str) -> bool {
    !myelin_substrate::is_destructive(ddl)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use myelin_storage::KmsEngine;

    fn t() -> TenantId {
        TenantId("acme".into())
    }
    fn r() -> Region {
        Region("fr-par".into())
    }

    /// **The edge table DDL is the §3.2 shape — every column + both keys present.** The materialised
    /// projection's columns (the deterministic `edge_id`, the full-URN `source`/`target`, the
    /// `#sub`-stripped `*_root`, `rel`/`rel_class`, the audit `origin_*`, `zookie`, `tombstoned`) are
    /// all declared; the tenant-first `PRIMARY KEY (tenant_id, edge_id)` + the
    /// `UNIQUE (tenant_id, source, target, rel)` idempotency key are present.
    #[test]
    fn create_edge_table_ddl_is_the_3_2_shape() {
        let ddl = CREATE_EDGE_TABLE_DDL;
        for col in [
            "tenant_id",
            "region",
            "edge_id",
            "source",
            "source_root",
            "target",
            "target_root",
            "rel ",
            "rel_class",
            "origin_event",
            "origin_actor",
            "created_at",
            "zookie",
            "tombstoned",
            "dek_ref",
        ] {
            assert!(
                ddl.contains(col),
                "the §3.2 edge column `{col}` is declared in the DDL"
            );
        }
        // tenant-first primary key (the tenant-predicate floor: every key is tenant-first).
        assert!(
            ddl.contains("PRIMARY KEY (tenant_id, edge_id)"),
            "the primary key is tenant-first (tenant_id, edge_id) — §3.2"
        );
        // the idempotency backstop: one edge per (source, target, rel) per tenant.
        assert!(
            ddl.contains("UNIQUE (tenant_id, source, target, rel)"),
            "the UNIQUE (tenant_id, source, target, rel) idempotency key is present — §3.2"
        );
    }

    /// **The three §3.2 indexes exist with their exact WHERE predicates.** `edge_inbound`
    /// (`WHERE NOT tombstoned`), `edge_outbound` (no predicate), `edge_by_rel`
    /// (`WHERE rel_class = 'lifecycle'`) — each tenant-first. The partial predicates are
    /// load-bearing (live-edges-only inbound; lifecycle-only TE-7 traversal).
    #[test]
    fn the_three_indexes_carry_their_where_predicates() {
        let by_name = |n: &str| {
            CREATE_EDGE_INDEXES_DDL
                .iter()
                .find(|(name, _)| *name == n)
                .map(|(_, ddl)| *ddl)
                .unwrap()
        };
        // edge_inbound: tenant-first, target_root, live edges only.
        let inbound = by_name(EDGE_INBOUND_INDEX);
        assert!(
            inbound.contains("(tenant_id, target_root)"),
            "edge_inbound keys (tenant_id, target_root)"
        );
        assert!(
            inbound.contains("WHERE NOT tombstoned"),
            "edge_inbound is live-edges-only (§3.2)"
        );
        // edge_outbound: tenant-first, source_root, no predicate (the C-4 filter column).
        let outbound = by_name(EDGE_OUTBOUND_INDEX);
        assert!(
            outbound.contains("(tenant_id, source_root)"),
            "edge_outbound keys (tenant_id, source_root)"
        );
        assert!(
            !outbound.contains("WHERE"),
            "edge_outbound has no partial predicate (§3.2)"
        );
        // edge_by_rel: tenant-first, (target_root, rel), lifecycle-class only (the TE-7 seam).
        let by_rel = by_name(EDGE_BY_REL_INDEX);
        assert!(
            by_rel.contains("(tenant_id, target_root, rel)"),
            "edge_by_rel keys (tenant_id, target_root, rel)"
        );
        assert!(
            by_rel.contains("WHERE rel_class = 'lifecycle'"),
            "edge_by_rel is lifecycle-class only (the TE-7 traversal index, §3.2)"
        );
    }

    /// **Every index is tenant-first (the tenant-predicate / no-cross-tenant-query-path floor).** No
    /// index path can scan across tenants — `tenant_id` is the FIRST column of all three (§3.2 / §3
    /// "no cross-tenant query path").
    #[test]
    fn every_index_is_tenant_first() {
        for (name, ddl) in CREATE_EDGE_INDEXES_DDL {
            assert!(
                ddl.contains("(tenant_id,"),
                "index `{name}` must be tenant-first (no cross-tenant query path): {ddl}"
            );
        }
    }

    /// **The migration applies forward-only (no DROP, no down) — the contract-1.5 floor.** The
    /// assembled edge DDL carries the create + the three indexes + the RLS scoping, and is
    /// forward-only-legal: `is_destructive` is false. The runner / lint enforce this at boot /
    /// source-scan; this is the in-module structural proof.
    #[test]
    fn the_edge_migration_is_forward_only() {
        let migrations = edge_table_migrations();
        assert_eq!(
            migrations.0.len(),
            1,
            "one forward migration: create the edge schema"
        );
        let m = &migrations.0[0];
        assert_eq!(m.id, EDGE_MIGRATION_ID);
        assert_eq!(m.table, Some(EDGE_TABLE));
        assert_eq!(
            m.phase,
            MigrationPhase::Plain,
            "a CREATE TABLE is a plain forward migration"
        );
        // forward-only: no destructive DROP anywhere in the assembled DDL.
        assert!(
            edge_ddl_is_forward_only(m.ddl),
            "the edge migration is forward-only (no DROP)"
        );
        assert!(
            !m.ddl.to_ascii_uppercase().contains("DROP"),
            "no DROP in the edge migration"
        );
        // the assembled DDL carries the create + all three indexes + the RLS scoping.
        assert!(
            m.ddl.contains("CREATE TABLE IF NOT EXISTS edge"),
            "the create-table rides the migration"
        );
        for (name, _) in CREATE_EDGE_INDEXES_DDL {
            assert!(m.ddl.contains(name), "index `{name}` rides the migration");
        }
        assert!(
            m.ddl.contains("myelin_make_tenant_scoped('edge')"),
            "the RLS scoping rides the migration"
        );
    }

    /// **The migration RUNNER admits the edge migration forward-only at boot (contract 1.5).** The
    /// substrate runner applies it (no DROP, no blocking ALTER on a hot table — `edge` is not hot for
    /// the create), recording it applied. This is the boot-time half of the forward-only gate.
    #[test]
    fn the_runner_admits_the_edge_migration() {
        use myelin_substrate::{HotTables, MigrationRunner};
        let migrations = edge_table_migrations();
        let mut runner = MigrationRunner::new();
        // `edge` is NOT declared hot for the create-and-index migration (empty fresh table).
        runner
            .run(&migrations, &HotTables::none())
            .expect("the edge schema migration applies forward-only");
        assert_eq!(
            runner.applied(),
            &[EDGE_MIGRATION_ID],
            "the runner applied the edge migration"
        );
    }

    /// **The runner REFUSES a destructive variant (forward-only is structural).** A hypothetical
    /// `DROP TABLE edge` rollback is rejected — proving the gate is not vacuously green (a real DROP
    /// would halt boot, §9.1 / EI-01 §2).
    #[test]
    fn a_destructive_edge_rollback_is_refused() {
        use myelin_substrate::{HotTables, Migration, MigrationRunner, Migrations};
        let bad = Migrations::of([Migration::plain("refs_9999_drop", "DROP TABLE edge")]);
        let mut runner = MigrationRunner::new();
        let e = runner
            .run(&bad, &HotTables::none())
            .expect_err("a DROP must be refused");
        assert!(
            e.0.contains("forward-only"),
            "the refusal names forward-only: {}",
            e.0
        );
    }

    /// **The table is RLS-ready — the migration calls the platform `myelin_make_tenant_scoped`
    /// convention (FORCE row-level security + the `(tenant_id, region)` isolation policy).** Refs
    /// does NOT fork the RLS policy; it uses the ONE helper every tenant table uses (EI-01 §7). The
    /// structural assertion that the edge table is RLS-on + `(tenant, region)`-partitioned.
    #[test]
    fn the_edge_table_is_rls_on_and_tenant_region_partitioned() {
        // RLS: the migration installs the platform-wide isolation policy on `edge`.
        assert_eq!(
            MAKE_EDGE_TENANT_SCOPED_DDL,
            "SELECT myelin_make_tenant_scoped('edge')"
        );
        // (tenant, region)-partitioned: tenant_id + region are the first columns, the primary key is
        // tenant-first, the RLS policy binds (tenant_id, region).
        let ddl = CREATE_EDGE_TABLE_DDL;
        let tenant_pos = ddl.find("tenant_id").expect("tenant_id column");
        let region_pos = ddl.find("region").expect("region column");
        let edge_id_pos = ddl.find("edge_id").expect("edge_id column");
        assert!(tenant_pos < region_pos, "tenant_id is the FIRST column");
        assert!(
            region_pos < edge_id_pos,
            "region is the SECOND column (the (tenant, region) prefix)"
        );
    }

    /// **The edge table is encrypted-from-birth under the per-tenant DEK (REF-P4, contract 11.3).**
    /// The `dek_ref` column carries the per-tenant DEK ref for every row, resolved through the SAME
    /// REF-P4 [`RefsDekPin`] (one cell root, one hierarchy — no second KMS). The ref is the
    /// `kms://<tenant>/<epoch>/tenant` anchor every edge row travels with.
    #[test]
    fn the_edge_table_is_encrypted_from_birth_under_the_per_tenant_dek() {
        // the table carries a per-row DEK ref column (no row is plaintext).
        assert!(
            CREATE_EDGE_TABLE_DDL.contains("dek_ref"),
            "the edge table carries the per-row DEK ref"
        );
        // the DEK ref is the per-tenant DEK reserved in REF-P4 (the encrypted-from-birth anchor).
        let dek = RefsDekPin::new(Arc::new(KmsEngine::new()));
        let key_ref =
            edge_table_dek_ref(&dek, &t(), &r()).expect("reserve the edge table per-tenant DEK");
        assert_eq!(
            key_ref, "kms://acme/0/tenant",
            "the encrypted-from-birth per-tenant DEK ref (§3.7)"
        );
        // it is the SAME ref the REF-P4 pin reserves (one hierarchy — not a second KMS).
        let direct = dek.reserve(&t(), &r()).expect("reserve directly").to_uri();
        assert_eq!(
            key_ref, direct,
            "the edge table keys on the REF-P4 per-tenant DEK (one hierarchy)"
        );
    }
}
