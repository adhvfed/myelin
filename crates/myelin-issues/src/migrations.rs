//! **The complete forward-only data-model migrations for the Issue Tracker spine** (ISS-P05 /
//! P-371; contract 1.5 forward-only online migrations + the hot-table flags; 11.1 OLTP + RLS;
//! 12.1 the `(tenant, region)` partition key; 2.3 the outbox table; 2.5 the consumer_dedup ledger).
//!
//! **Owning architecture doc (byte-authoritative):**
//! `planning/04-subsystem-architectures/issue-tracker/architecture/01-tech-and-data-model.md`
//! §2 (the `issue` spine — typed core + JSONB tail + the `(tenant, region)` partition key + the
//! lifecycle/GDPR columns), §3 (the governance `scheme`/`scheme_assignment` tables), §4 (the
//! `issue_relation` TE-7 source-of-truth typed table), §5 (the `issue_change_log`, `cycle`,
//! `cycle_membership`, `milestone` tables), §7 (the `prefix_counter` Hi/Lo allocator), §8.1 (the
//! stateful-component register naming `issue` / `issue_relation` / `issue_change_log` HOT). Plus the
//! frozen platform `consumer_dedup` (2.5) + `outbox` (2.3) tables every subsystem co-locates in its
//! own service DB.
//!
//! ## What ISS-P05 ships here — the table SHAPES, forward-only, RLS-ready (NOT the behaviour)
//! Every table below is created exactly as frozen in arch 01 §2–§8, as a **forward-only** migration
//! (contract 1.5; no DROP, no down/rollback) expressed through the substrate framework
//! ([`myelin_substrate::Migration`] / [`Migrations`]) so the boot-time RUNNER applies it AND the
//! `forward-only-migration` lint reads it at source-scan. Each table is:
//! - **`(tenant_id, region)`-first** (arch 01 §2: `tenant`/`region` are the leading columns / the
//!   partition prefix, contract 12.1) — the `tenant-predicate` lint target (every key is
//!   tenant-first; there is no cross-tenant query path);
//! - **RLS-enforced** via the platform-wide `myelin_make_tenant_scoped(...)` convention
//!   (`scripts/pg-init/00-rls-conventions.sql`) — FORCE row-level security + the `(tenant_id,
//!   region)` isolation policy. Issues does NOT fork the RLS policy (EI-01 §7 coherence — one helper,
//!   the same one CI / Refs / Knowledge use).
//!
//! ## Reconciliation: the §2 column name vs the platform RLS convention (documented deviation)
//! Architecture §2 names the tenant partition column `tenant uuid`. The platform-wide RLS helper
//! `myelin_make_tenant_scoped` (the ONE dev/prod RLS convention every tenant table uses, storage
//! §3.1 / contract 11.1) binds its `(tenant_id, region)` isolation policy to a `tenant_id text` +
//! `region text` pair. To keep ONE RLS convention across every subsystem, these migrations name the
//! columns **`tenant_id text` + `region text`** (the convention's exact names) while preserving §2's
//! intent verbatim: `tenant_id`/`region` are the FIRST columns / partition prefix and the RLS
//! isolation key. The `uuid` vs `text` choice follows the platform convention (the tenant token is an
//! opaque string at this layer — `myelin_tenancy::TenantId(String)`); a tenant id is a stable opaque
//! token, never PII. This is the same deliberate, documented deviation `myelin-ci-controlplane` /
//! `myelin-refs-service` / `myelin-knowledge` record (EI-01 §1, code-wins-over-docs: the convention
//! wins over the literal column name so the RLS floor is the SAME one Postgres enforces everywhere).
//!
//! The fixed value-sets (`state_category`, `scheme_kind`, the relation `rel`) are enforced by
//! `CHECK` constraints (the frozen vocabularies §2/§3/§4) rather than Postgres `ENUM` types so a
//! forward-only vocabulary EXTENSION is a non-blocking `CHECK` add, never an enum-rewrite
//! (forward-only, §9). The one INVARIANT category set (`state_category`) is the frozen FOUR
//! (`unstarted`/`started`/`completed`/`cancelled`, sketch 02) — it is closed by design (the
//! cross-project reporting invariant), so it stays a CHECK on the fixed four.
//!
//! ## The hot tables (arch 01 §8.1 — the write-QPS tables)
//! `issue`, `issue_relation`, `issue_change_log` are declared HOT
//! ([`issues_hot_tables`], contract 1.5 / §8.1 "Hot tables flagged"): they carry the board/field-edit
//! write churn, the typed-edge writes, and the per-issue change-log append volume. The hot-table flag
//! means the migration runner refuses a blocking `ALTER` on them at boot, and the
//! `forward-only-migration` lint reads the same declaration at source-scan. The CREATE-TABLE
//! migrations themselves are `Plain` (a create on an empty table takes no meaningful lock; the
//! expand→backfill→contract discipline applies to LATER ALTERs on the populated hot tables, declared
//! by the behaviour bands as the write rate warrants — §9.4).
//!
//! ## Floors named (VISION §3 / prompt DoD) — the per-table BEHAVIOUR follow-ons
//! **This is the SCHEMA ONLY — empty tables are not a working subsystem.** The per-table behaviour
//! lands in its own prompt and is named here:
//! - **Storage = PG-hybrid sharded by tenant** (the typed-core + JSONB tail + the GIN/generated-index
//!   projection). Distributed-SQL (CockroachDB/Yugabyte) is the **named, measured follow-on (R-6,
//!   ISS-P32 / M5)** — only if a single tenant's shard is *measured* to outgrow PG (arch 01 §1.2,
//!   EI-02 §8); PG-sharded is the floor.
//! - the **silent-data-loss-safe write path** (validate → check → mutate → `OutboxTx::emit` in one
//!   tx over the `issue` / `issue_relation` / `issue_change_log` + `outbox` tables) — **ISS-P06**
//!   (P-372);
//! - the **pseudonymous-by-default identity columns + per-subject-DEK free-text + the holder
//!   registration body** (the `pii_key_ref` columns are present here as schema; the DEK wiring +
//!   the full holder ops) — **ISS-P07** (P-373);
//! - the **Hi/Lo human-key allocation** over `prefix_counter` (the `<PROJECTKEY>-<seqno>` block
//!   reserve) — **ISS-P08** (P-374);
//! - the **scheme-precedence algebra + the flexible-field model** over `scheme` /
//!   `scheme_assignment` — **ISS-P11** (P-377);
//! - the **time-axis behaviour** (cycles/sprints + milestones rollup/burndown) over `cycle` /
//!   `cycle_membership` / `milestone` — **ISS-P18+** (P-386).
//!
//! Nothing below writes a row; this migration set creates the tables forward-only + RLS-on so the
//! behaviour bands have their targets.
//!
//! The live-DB forward-only apply (against the dev-stack Postgres) is proven in
//! `tests/integration_iss_p05_spine_schema.rs` (the `integration` cargo feature); the default
//! `cargo build`/`cargo test --workspace` stay DB-free.

use myelin_events::OUTBOX_MIGRATION;
use myelin_substrate::{HotTables, Migration, Migrations};

/// The Issue-Tracker spine table names (arch 01 §2–§8). PII-free opaque identifiers. The order is
/// the foreign-key dependency order the runner applies in (`issue` before `issue_relation`, which
/// FK-references it).
pub const ISSUE_TABLE: &str = "issue";
pub const ISSUE_RELATION_TABLE: &str = "issue_relation";
pub const ISSUE_CHANGE_LOG_TABLE: &str = "issue_change_log";
pub const SCHEME_TABLE: &str = "scheme";
pub const SCHEME_ASSIGNMENT_TABLE: &str = "scheme_assignment";
pub const CYCLE_TABLE: &str = "cycle";
pub const CYCLE_MEMBERSHIP_TABLE: &str = "cycle_membership";
pub const MILESTONE_TABLE: &str = "milestone";
pub const PREFIX_COUNTER_TABLE: &str = "prefix_counter";
pub const CONSUMER_DEDUP_TABLE: &str = "consumer_dedup";
pub const OUTBOX_TABLE: &str = "outbox";

/// The `issue` hot-board / list scan index names (arch 01 §2 — the index-range hot path the
/// board/roadmap/assignee/cycle reads ride). The behaviour (the scan) is the read path; the SHAPES
/// land here so the board has its indexes.
pub const ISSUE_BOARD_INDEX: &str = "issue_board";
pub const ISSUE_ROADMAP_INDEX: &str = "issue_roadmap";
pub const ISSUE_ASSIGNEE_INDEX: &str = "issue_assignee";
pub const ISSUE_PARENT_INDEX: &str = "issue_parent";
pub const ISSUE_CYCLE_INDEX: &str = "issue_cycle";
pub const ISSUE_PROPS_GIN_INDEX: &str = "issue_props_gin";

// ============================================================================================
// The forward-only CREATE-TABLE DDL constants (arch 01 §2–§8, verbatim intent; tenant_id/region
// named to the RLS convention — see the module deviation note). Held as `&str` so the DDL is NOT
// mistaken for live Rust by the lints (`blank_string_literals` blanks literal contents), while the
// migration framework still carries the real DDL to the boot runner / the live integration test.
// ============================================================================================

/// `issue` (arch 01 §2) — the central spine: the typed core (always-present, indexed columns — the
/// board/list/report hot path) + the JSONB property-bag tail (`props`/`props_nodes`, zero-DDL custom
/// fields) + the lifecycle/GDPR columns (`deleted_at`/`contains_personal_data`/`restricted`/
/// `pii_key_ref`, recon §X-7). `assignee`/`reporter` are PSEUDONYM principal ids (4.8, erasure-safe);
/// `pii_key_ref` is the per-subject DEK ref (GD-4, wired in ISS-P07). HOT (board/field edits).
pub const CREATE_ISSUE_DDL: &str = "\
CREATE TABLE IF NOT EXISTS issue (
  tenant_id              text        NOT NULL,
  region                 text        NOT NULL,
  id                     uuid        NOT NULL,
  key                    text        NOT NULL,
  prefix                 text        NOT NULL,
  type_id                uuid        NOT NULL,
  type_rank              smallint    NOT NULL,
  state                  text        NOT NULL,
  state_category         text        NOT NULL CHECK (state_category IN ('unstarted','started','completed','cancelled')),
  priority               smallint    NOT NULL DEFAULT 0,
  assignee               uuid,
  reporter               uuid        NOT NULL,
  parent_id              uuid,
  project_id             uuid        NOT NULL,
  cycle_id               uuid,
  rank                   text        NOT NULL,
  title                  text        NOT NULL,
  body_block             uuid,
  props                  jsonb       NOT NULL DEFAULT '{}',
  props_nodes            jsonb       NOT NULL DEFAULT '[]',
  created_at             timestamptz NOT NULL DEFAULT now(),
  updated_at             timestamptz NOT NULL DEFAULT now(),
  state_changed_at       timestamptz NOT NULL DEFAULT now(),
  archived               boolean     NOT NULL DEFAULT false,
  deleted_at             timestamptz,
  contains_personal_data boolean     NOT NULL DEFAULT false,
  data_role              text        NOT NULL DEFAULT 'tenant-content',
  restricted             boolean     NOT NULL DEFAULT false,
  pii_key_ref            text,
  version                bigint       NOT NULL,
  PRIMARY KEY (tenant_id, id),
  UNIQUE (tenant_id, key)
)";

/// The `issue` hot-path indexes (arch 01 §2 — the board/roadmap/assignee/parent/cycle scans + the
/// custom-field GIN). Built `CONCURRENTLY` on the declared-HOT `issue` table so a re-apply against
/// live write traffic stays non-blocking (the expand-phase discipline, §9.4); the partial predicates
/// (`WHERE deleted_at IS NULL`, etc.) are the soft-delete-aware hot path. Each is tenant-first.
pub const CREATE_ISSUE_INDEXES_DDL: &[(&str, &str)] = &[
    (
        ISSUE_BOARD_INDEX,
        "CREATE INDEX CONCURRENTLY IF NOT EXISTS issue_board ON issue (tenant_id, project_id, state_category, rank) WHERE deleted_at IS NULL",
    ),
    (
        ISSUE_ROADMAP_INDEX,
        "CREATE INDEX CONCURRENTLY IF NOT EXISTS issue_roadmap ON issue (tenant_id, project_id, type_rank, rank) WHERE type_rank >= 2 AND deleted_at IS NULL",
    ),
    (
        ISSUE_ASSIGNEE_INDEX,
        "CREATE INDEX CONCURRENTLY IF NOT EXISTS issue_assignee ON issue (tenant_id, assignee, state_category) WHERE deleted_at IS NULL",
    ),
    (
        ISSUE_PARENT_INDEX,
        "CREATE INDEX CONCURRENTLY IF NOT EXISTS issue_parent ON issue (tenant_id, parent_id)",
    ),
    (
        ISSUE_CYCLE_INDEX,
        "CREATE INDEX CONCURRENTLY IF NOT EXISTS issue_cycle ON issue (tenant_id, cycle_id, state_category) WHERE cycle_id IS NOT NULL",
    ),
    (
        ISSUE_PROPS_GIN_INDEX,
        "CREATE INDEX CONCURRENTLY IF NOT EXISTS issue_props_gin ON issue USING gin (props jsonb_path_ops)",
    ),
];

/// Forward-only nullable-column expand that makes the live issue store's free-text posture
/// executable. The legacy `title` column remains as a non-PII sentinel during the expand phase;
/// new writes put only ciphertext + nonce + the existing `pii_key_ref` in the row. A later measured
/// backfill/contract migration may remove the sentinel column after every older row is converted.
/// All three additions are nullable and carry no default, so this is an online metadata-only expand
/// on the declared-HOT `issue` table.
pub const EXPAND_ISSUE_DURABLE_STORE_DDL: &str = "\
ALTER TABLE issue ADD COLUMN IF NOT EXISTS title_nonce bytea;
ALTER TABLE issue ADD COLUMN IF NOT EXISTS title_ciphertext bytea;
ALTER TABLE issue ADD COLUMN IF NOT EXISTS created_by_principal text;";

/// `issue_relation` (arch 01 §4 — the TE-7 source of truth, contract 5.5). We write the FORWARD edge
/// transactionally + emit ONE typed event; Refs materialises both directions. The FK constrains only
/// the `src_issue` end (the far `dst_ref` may be cross-subsystem). HOT (typed-edge writes). The
/// `rel` vocabulary is a CHECK (forward-only vocabulary extension), not an enum.
pub const CREATE_ISSUE_RELATION_DDL: &str = "\
CREATE TABLE IF NOT EXISTS issue_relation (
  tenant_id   text        NOT NULL,
  region      text        NOT NULL,
  relation_id uuid        NOT NULL,
  src_issue   uuid        NOT NULL,
  dst_ref     text        NOT NULL,
  rel         text        NOT NULL CHECK (rel IN ('parent','blocks','blocked_by','closes','depends_on','relates')),
  created_by  uuid        NOT NULL,
  created_at  timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (tenant_id, relation_id),
  UNIQUE (tenant_id, src_issue, dst_ref, rel),
  FOREIGN KEY (tenant_id, src_issue) REFERENCES issue(tenant_id, id) ON DELETE CASCADE
)";

/// The two `issue_relation` traversal indexes (arch 01 §4): `issue_rel_src` (the forward walk) +
/// `issue_rel_dst` ("what blocks me" — the close-guard read, sketch 05A). Built `CONCURRENTLY` on
/// the declared-HOT table (non-blocking against live writes).
pub const CREATE_ISSUE_RELATION_INDEXES_DDL: &[(&str, &str)] = &[
    (
        "issue_rel_src",
        "CREATE INDEX CONCURRENTLY IF NOT EXISTS issue_rel_src ON issue_relation (tenant_id, src_issue, rel)",
    ),
    (
        "issue_rel_dst",
        "CREATE INDEX CONCURRENTLY IF NOT EXISTS issue_rel_dst ON issue_relation (tenant_id, dst_ref, rel)",
    ),
];

/// `issue_change_log` (arch 01 §5) — every state change recorded; the audit + GDPR + activity-feed
/// basis (per-issue monotonic `seq`, the issue is the aggregate). `actor` is a PSEUDONYM (human OR
/// agent — same shape); `pii_key_ref` is the per-subject DEK if a delta carries free-text PII (GD-4).
/// HOT (per-issue append volume).
pub const CREATE_ISSUE_CHANGE_LOG_DDL: &str = "\
CREATE TABLE IF NOT EXISTS issue_change_log (
  tenant_id    text        NOT NULL,
  region       text        NOT NULL,
  issue_id     uuid        NOT NULL,
  seq          bigint      NOT NULL,
  actor        uuid        NOT NULL,
  actor_kind   text        NOT NULL CHECK (actor_kind IN ('human','agent','service')),
  on_behalf_of uuid,
  change       jsonb       NOT NULL,
  pii_key_ref  text,
  at           timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (tenant_id, issue_id, seq)
)";

/// `scheme` (arch 01 §3) — the layered governance scheme objects, INTERPRETED at runtime so turning
/// governance on is a config write, never a data migration. `body` is the kind-specific JSONB
/// definition; `kind` is the frozen five-kind vocabulary (CHECK, not enum — forward-only extension).
pub const CREATE_SCHEME_DDL: &str = "\
CREATE TABLE IF NOT EXISTS scheme (
  tenant_id text   NOT NULL,
  region    text   NOT NULL,
  scheme_id uuid   NOT NULL,
  kind      text   NOT NULL CHECK (kind IN ('workflow','field','permission','sla','type')),
  name      text   NOT NULL,
  body      jsonb  NOT NULL,
  version   bigint NOT NULL,
  PRIMARY KEY (tenant_id, scheme_id)
)";

/// `scheme_assignment` (arch 01 §3) — the `(type × project × team)` assignment the precedence algebra
/// (ISS-P11) resolves. NULL = "any" at that axis (the org-default fallback). The PK uses sentinel
/// `COALESCE`d nils so a NULL axis is a real, uniquely-addressable assignment slot. `kind` is the
/// same frozen five-kind CHECK.
pub const CREATE_SCHEME_ASSIGNMENT_DDL: &str = "\
CREATE TABLE IF NOT EXISTS scheme_assignment (
  tenant_id  text NOT NULL,
  region     text NOT NULL,
  scheme_id  uuid NOT NULL,
  kind       text NOT NULL CHECK (kind IN ('workflow','field','permission','sla','type')),
  type_id    uuid,
  project_id uuid,
  team_id    uuid,
  PRIMARY KEY (tenant_id, kind,
    COALESCE(type_id,    '00000000-0000-0000-0000-000000000000'::uuid),
    COALESCE(project_id, '00000000-0000-0000-0000-000000000000'::uuid),
    COALESCE(team_id,    '00000000-0000-0000-0000-000000000000'::uuid))
)";

/// `cycle` (arch 01 §5 — the TIME AXIS) — a cycle is NOT an issue (no workflow state/assignee);
/// membership is a relation (`cycle_membership`), not containment. `state` is the lifecycle of the
/// cycle itself.
pub const CREATE_CYCLE_DDL: &str = "\
CREATE TABLE IF NOT EXISTS cycle (
  tenant_id  text        NOT NULL,
  region     text        NOT NULL,
  cycle_id   uuid        NOT NULL,
  project_id uuid        NOT NULL,
  name       text        NOT NULL,
  starts_at  timestamptz NOT NULL,
  ends_at    timestamptz NOT NULL,
  capacity   numeric,
  state      text        NOT NULL CHECK (state IN ('planned','active','completed')),
  PRIMARY KEY (tenant_id, cycle_id)
)";

/// `cycle_membership` (arch 01 §5) — the issue↔cycle relation (NOT containment), with carry-over
/// provenance (flow A3). The `issue` row carries a denormalised `cycle_id` cache; THIS is the truth.
pub const CREATE_CYCLE_MEMBERSHIP_DDL: &str = "\
CREATE TABLE IF NOT EXISTS cycle_membership (
  tenant_id         text        NOT NULL,
  region            text        NOT NULL,
  cycle_id          uuid        NOT NULL,
  issue_id          uuid        NOT NULL,
  added_at          timestamptz NOT NULL DEFAULT now(),
  carried_over_from uuid,
  PRIMARY KEY (tenant_id, cycle_id, issue_id)
)";

/// `milestone` (arch 01 §5) — versions/releases (the import target; sketch 09). `target_date` is the
/// planned date; `released_at` flips when shipped.
pub const CREATE_MILESTONE_DDL: &str = "\
CREATE TABLE IF NOT EXISTS milestone (
  tenant_id    text        NOT NULL,
  region       text        NOT NULL,
  milestone_id uuid        NOT NULL,
  project_id   uuid        NOT NULL,
  name         text        NOT NULL,
  target_date  date,
  released_at  timestamptz,
  PRIMARY KEY (tenant_id, milestone_id)
)";

/// `prefix_counter` (arch 01 §7 — the Hi/Lo allocator) — the durable high-water mark advanced by a
/// block at reserve (the `<PROJECTKEY>-<seqno>` human key, ISS-P08). `block_size` is adaptive (small
/// for cold prefixes, large for hot). Per-prefix isolation (a busy `ENG` doesn't slow `OPS`).
pub const CREATE_PREFIX_COUNTER_DDL: &str = "\
CREATE TABLE IF NOT EXISTS prefix_counter (
  tenant_id  text   NOT NULL,
  region     text   NOT NULL,
  prefix     text   NOT NULL,
  high_water bigint NOT NULL,
  block_size int    NOT NULL DEFAULT 50,
  PRIMARY KEY (tenant_id, prefix)
)";

/// `consumer_dedup` (the platform consumer template's exactly-once-effect ledger, contract 2.5) —
/// `(tenant_id, region)`-first with the `(consumer, event_id)` dedup PK. The bus consumers (rollup /
/// SLA / trigger / feeder) dedup on this so one delivery = one effect under at-least-once redelivery.
/// The dedup LOGIC lands with the consumer bands; the SHAPE is the platform-frozen one (no fork).
pub const CREATE_CONSUMER_DEDUP_DDL: &str = "\
CREATE TABLE IF NOT EXISTS consumer_dedup (
  tenant_id text NOT NULL,
  region    text NOT NULL,
  consumer  text NOT NULL,
  event_id  text NOT NULL,
  PRIMARY KEY (consumer, event_id)
)";

/// The RLS scoping DDL for an Issue-Tracker table — the platform-wide `myelin_make_tenant_scoped`
/// convention (FORCE row-level security + the `(tenant_id, region)` isolation policy). Issues does
/// NOT fork the RLS policy; it calls the ONE helper every tenant table uses (EI-01 §7).
pub fn make_tenant_scoped_ddl(table: &str) -> String {
    format!("SELECT myelin_make_tenant_scoped('{table}')")
}

/// Every Issue-Tracker CREATE-TABLE DDL paired with its table name + a stable migration id, in
/// FK-dependency order (`issue` before `issue_relation`). The `issue` create rides with its six
/// hot-path indexes appended (an empty fresh table — no hot-table lock; the create is atomic); the
/// `issue_relation` create rides with its two traversal indexes. The `outbox` table is the frozen
/// platform DDL ([`myelin_events::OUTBOX_MIGRATION`], contract 2.3) — Issues co-locates it in its own
/// service DB (the same-tx co-commit anchor), it does NOT author a second outbox shape (EI-01 §7).
/// One ordered list so [`issues_migrations`] builds the [`Migrations`] set + the
/// `forward-only-migration` lint reads the same DDL.
fn create_statements() -> Vec<(&'static str, &'static str, String)> {
    vec![
        ("iss_0001_issue", ISSUE_TABLE, CREATE_ISSUE_DDL.to_string()),
        (
            "iss_0002_issue_relation",
            ISSUE_RELATION_TABLE,
            CREATE_ISSUE_RELATION_DDL.to_string(),
        ),
        (
            "iss_0003_issue_change_log",
            ISSUE_CHANGE_LOG_TABLE,
            CREATE_ISSUE_CHANGE_LOG_DDL.to_string(),
        ),
        (
            "iss_0004_scheme",
            SCHEME_TABLE,
            CREATE_SCHEME_DDL.to_string(),
        ),
        (
            "iss_0005_scheme_assignment",
            SCHEME_ASSIGNMENT_TABLE,
            CREATE_SCHEME_ASSIGNMENT_DDL.to_string(),
        ),
        ("iss_0006_cycle", CYCLE_TABLE, CREATE_CYCLE_DDL.to_string()),
        (
            "iss_0007_cycle_membership",
            CYCLE_MEMBERSHIP_TABLE,
            CREATE_CYCLE_MEMBERSHIP_DDL.to_string(),
        ),
        (
            "iss_0008_milestone",
            MILESTONE_TABLE,
            CREATE_MILESTONE_DDL.to_string(),
        ),
        (
            "iss_0009_prefix_counter",
            PREFIX_COUNTER_TABLE,
            CREATE_PREFIX_COUNTER_DDL.to_string(),
        ),
        (
            "iss_0010_consumer_dedup",
            CONSUMER_DEDUP_TABLE,
            CREATE_CONSUMER_DEDUP_DDL.to_string(),
        ),
        (
            "iss_0011_outbox",
            OUTBOX_TABLE,
            OUTBOX_MIGRATION.to_string(),
        ),
    ]
}

/// **The complete Issue-Tracker forward-only migration set** (contract 1.5 / 11.1 / 2.3 / 2.5; arch
/// 01 §2–§8). One [`Migration`] per table (`Plain` — a CREATE on an empty table is a plain forward
/// migration; no expand→backfill→contract is needed to CREATE), in FK-dependency order, each
/// carrying its CREATE-TABLE DDL + the platform RLS scoping (and, for `issue`/`issue_relation`, their
/// hot-path indexes). The runner applies them forward-only at boot; the `forward-only-migration` lint
/// reads the same DDL.
///
/// The frozen platform `outbox` table ([`OUTBOX_MIGRATION`], 2.3) carries no `tenant_id`/`region`
/// columns (it is the cross-seam cursor keyed on `(aggregate, seq)`); it is co-located + made
/// tenant-scoped is NOT applicable (the relay reads it cross-tenant within the cell), so it is the
/// ONE table that rides without the `myelin_make_tenant_scoped` call — exactly as the platform
/// co-location migration set frames it (`myelin_storage::coloc`).
pub fn issues_migrations() -> Migrations {
    let mut migrations = Vec::new();
    for (id, table, create) in create_statements() {
        let mut ddl = create;
        if !ddl.trim_end().ends_with(';') {
            ddl.push(';');
        }
        // The outbox is the platform cross-seam cursor (keyed on (aggregate, seq), no tenant
        // partition columns — it is drained by the relay within the cell); it is the ONE table that
        // does NOT take the tenant-scoped RLS policy. Every Issues DOMAIN table is RLS-scoped.
        if table != OUTBOX_TABLE {
            ddl.push('\n');
            ddl.push_str(&make_tenant_scoped_ddl(table));
            ddl.push(';');
        }
        // The substrate `Migration` holds `&'static str`; the set is built once at boot/serve, so
        // this is a one-time, bounded leak — the same shape the framework + the CI/refs shells expect.
        let ddl: &'static str = Box::leak(ddl.into_boxed_str());
        migrations.push(Migration::plain_on(id, ddl, table));
    }
    // Each concurrent index is its OWN migration/query. PostgreSQL rejects
    // `CREATE INDEX CONCURRENTLY` when it shares a simple-query message with other statements,
    // because the message becomes an implicit transaction block. Keeping each index separate is
    // both boot-applicable and non-blocking on the declared-hot tables.
    for (name, ddl) in CREATE_ISSUE_INDEXES_DDL {
        let id = Box::leak(format!("iss_0012_{name}").into_boxed_str());
        migrations.push(Migration::plain_on(id, ddl, ISSUE_TABLE));
    }
    for (name, ddl) in CREATE_ISSUE_RELATION_INDEXES_DDL {
        let id = Box::leak(format!("iss_0013_{name}").into_boxed_str());
        migrations.push(Migration::plain_on(id, ddl, ISSUE_RELATION_TABLE));
    }
    migrations.push(Migration::plain_on(
        "iss_0014_issue_durable_store_expand",
        EXPAND_ISSUE_DURABLE_STORE_DDL,
        ISSUE_TABLE,
    ));
    Migrations::of(migrations)
}

/// **The Issue-Tracker hot-table declaration** (contract 1.5 / §8.1; arch 01 §8.1 "Hot tables
/// flagged"). `issue` (the board/field-edit write churn), `issue_relation` (the typed-edge writes),
/// `issue_change_log` (the per-issue change-log append volume) are the write-QPS tables. A
/// declared-hot table refuses a blocking `ALTER` at boot (the migration runner) and is read by the
/// `forward-only-migration` lint at source-scan — so a future ALTER on one of them MUST go
/// expand→backfill→contract (§9.4).
pub fn issues_hot_tables() -> HotTables {
    HotTables::declare([ISSUE_TABLE, ISSUE_RELATION_TABLE, ISSUE_CHANGE_LOG_TABLE])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **All eleven Issue-Tracker spine tables are in the forward-only migration set, FK-ordered.**
    /// The complete arch 01 §2–§8 spine lands here; `issue` precedes `issue_relation` (the FK
    /// dependency). This is the prompt's "the issue-spine migrations" gate.
    #[test]
    fn all_eleven_spine_tables_are_present_fk_ordered() {
        let migrations = issues_migrations();
        let create_ids: std::collections::BTreeSet<&str> =
            create_statements().iter().map(|(id, _, _)| *id).collect();
        let tables: Vec<&str> = migrations
            .0
            .iter()
            .filter(|m| create_ids.contains(m.id))
            .map(|m| m.table.unwrap())
            .collect();
        assert_eq!(
            tables,
            vec![
                ISSUE_TABLE,
                ISSUE_RELATION_TABLE,
                ISSUE_CHANGE_LOG_TABLE,
                SCHEME_TABLE,
                SCHEME_ASSIGNMENT_TABLE,
                CYCLE_TABLE,
                CYCLE_MEMBERSHIP_TABLE,
                MILESTONE_TABLE,
                PREFIX_COUNTER_TABLE,
                CONSUMER_DEDUP_TABLE,
                OUTBOX_TABLE,
            ],
            "all 11 spine tables, FK-dependency ordered (issue before issue_relation)"
        );
        let issue_pos = tables.iter().position(|t| *t == ISSUE_TABLE).unwrap();
        let rel_pos = tables
            .iter()
            .position(|t| *t == ISSUE_RELATION_TABLE)
            .unwrap();
        assert!(
            issue_pos < rel_pos,
            "issue is created before issue_relation (the FK target before the FK source)"
        );
    }

    /// **Every Issues DOMAIN table is `(tenant_id, region)`-first with a tenant-first primary key
    /// (contract 12.1 / the tenant-predicate floor) and is RLS-scoped (0 un-scoped tables).** No key
    /// path can scan across tenants — `tenant_id` is the FIRST column on every domain table (arch 01
    /// §2 "no cross-tenant query path"). The platform `outbox` (the cross-seam cursor) is the one
    /// non-tenant-partitioned table (it is keyed on `(aggregate, seq)`); it is explicitly excluded.
    #[test]
    fn every_domain_table_is_tenant_region_first_and_rls_scoped() {
        for (_id, table, ddl) in create_statements() {
            if table == OUTBOX_TABLE {
                continue; // the cross-seam cursor — not tenant-partitioned (keyed on aggregate, seq).
            }
            let tenant_pos = ddl.find("tenant_id").expect("tenant_id column");
            let region_pos = ddl.find("region").expect("region column");
            assert!(
                tenant_pos < region_pos,
                "tenant_id is the FIRST column (before region) on `{table}`: {ddl}"
            );
            // The domain tables are tenant-first in the PRIMARY KEY. `consumer_dedup` is the ONE
            // documented exception: its PK is the platform consumer template's exactly-once
            // `(consumer, event_id)` dedup key (contract 2.5) — `tenant_id`/`region` still LEAD the
            // row (the partition prefix + the RLS isolation columns), but the PK is the dedup anchor,
            // not a re-invented tenant-first key (the same shape `myelin-ci-dispatch` carries).
            if table == CONSUMER_DEDUP_TABLE {
                assert!(
                    ddl.contains("PRIMARY KEY (consumer, event_id)"),
                    "consumer_dedup's PK is the platform (consumer, event_id) dedup key: {ddl}"
                );
            } else {
                assert!(
                    ddl.contains("PRIMARY KEY (tenant_id"),
                    "the primary key is tenant-first on `{table}`: {ddl}"
                );
            }
        }
        // The RLS scoping rides every domain TABLE-CREATE migration (and NOT the outbox). Index and
        // nullable-column expand steps operate on a table whose RLS policy is already installed.
        let create_ids: std::collections::BTreeSet<&str> =
            create_statements().iter().map(|(id, _, _)| *id).collect();
        for m in issues_migrations()
            .0
            .iter()
            .filter(|m| create_ids.contains(m.id))
        {
            if m.table == Some(OUTBOX_TABLE) {
                assert!(
                    !m.ddl.contains("myelin_make_tenant_scoped"),
                    "the cross-seam outbox is not tenant-scoped (it is the cell-wide relay cursor)"
                );
            } else {
                assert!(
                    m.ddl.contains("myelin_make_tenant_scoped"),
                    "domain migration {} installs the platform RLS policy (0 un-scoped tables)",
                    m.id
                );
            }
        }
    }

    /// **The migration set applies forward-only (no DROP, no down) — the contract-1.5 floor.** Every
    /// assembled DDL is forward-only-legal (`is_destructive` is false). The runner / lint enforce
    /// this at boot / source-scan; this is the in-module proof.
    #[test]
    fn the_migration_set_is_forward_only() {
        let migrations = issues_migrations();
        assert_eq!(
            migrations.0.len(),
            20,
            "11 table creates + 8 concurrent indexes + 1 online expand"
        );
        for m in &migrations.0 {
            assert!(
                !myelin_substrate::is_destructive(m.ddl),
                "migration {} is forward-only (no DROP): {}",
                m.id,
                m.ddl
            );
            assert!(
                !m.ddl.to_ascii_uppercase().contains("DROP"),
                "no DROP in migration {}",
                m.id
            );
        }
    }

    /// **The runner admits the whole set forward-only at boot, FK-ordered (contract 1.5).** The
    /// substrate runner applies every migration (no DROP, no blocking ALTER on a hot table — the
    /// CREATEs are Plain) and records them applied in order. This is the boot-time half of the gate
    /// (re-applies idempotently — the CREATEs are all `IF NOT EXISTS`).
    #[test]
    fn the_runner_admits_the_whole_set_idempotently() {
        use myelin_substrate::MigrationRunner;
        let migrations = issues_migrations();
        let hot = issues_hot_tables();
        let mut runner = MigrationRunner::new();
        runner
            .run(&migrations, &hot)
            .expect("the full Issue-Tracker spine applies forward-only");
        assert_eq!(
            runner.applied().len(),
            20,
            "the runner applied every table/index/expand migration"
        );
        assert_eq!(
            runner.applied()[0],
            "iss_0001_issue",
            "issue is applied first (FK order)"
        );

        // RE-APPLY: the same set is admitted again (the CREATEs are IF NOT EXISTS — idempotent).
        let mut runner2 = MigrationRunner::new();
        runner2
            .run(&migrations, &hot)
            .expect("the spine re-applies idempotently");
        assert_eq!(
            runner2.applied().len(),
            20,
            "the re-apply admits every migration again"
        );
    }

    /// **A destructive rollback variant is refused (forward-only is structural, not vacuous).** A
    /// hypothetical `DROP TABLE issue` is rejected by the runner — proving the gate is real (a real
    /// DROP would halt boot, §9.1 / EI-01 §2).
    #[test]
    fn a_destructive_rollback_is_refused() {
        use myelin_substrate::MigrationRunner;
        let bad = Migrations::of([Migration::plain("iss_9999_drop", "DROP TABLE issue")]);
        let mut runner = MigrationRunner::new();
        let e = runner
            .run(&bad, &issues_hot_tables())
            .expect_err("a DROP must be refused");
        assert!(
            e.0.contains("forward-only"),
            "the refusal names forward-only: {}",
            e.0
        );
    }

    /// **The three hot tables are declared (arch 01 §8.1 "Hot tables flagged").** `issue` /
    /// `issue_relation` / `issue_change_log` — the write-QPS tables that refuse a blocking ALTER
    /// (the expand→backfill→contract discipline) at boot. A non-hot table is NOT flagged.
    #[test]
    fn the_three_hot_tables_are_declared() {
        let hot = issues_hot_tables();
        for t in [ISSUE_TABLE, ISSUE_RELATION_TABLE, ISSUE_CHANGE_LOG_TABLE] {
            assert!(hot.is_hot(t), "`{t}` is declared hot (arch 01 §8.1)");
        }
        assert!(
            !hot.is_hot(SCHEME_TABLE),
            "scheme is NOT a hot table (config write rate)"
        );
        assert!(
            !hot.is_hot(MILESTONE_TABLE),
            "milestone is NOT a hot table (low write rate)"
        );
    }

    /// **The `issue` lifecycle/GDPR columns the erasure posture needs are present (recon §X-7).** The
    /// soft-delete (`deleted_at`), the restriction flag (`restricted`), the routing flag
    /// (`contains_personal_data`), the GDPR `data_role`, and the per-subject DEK ref (`pii_key_ref`)
    /// — the columns ISS-P06/P07 flow data through. Pins them so an edit cannot silently drop one.
    #[test]
    fn the_issue_lifecycle_and_gdpr_columns_are_present() {
        for col in [
            "deleted_at",
            "contains_personal_data",
            "data_role",
            "restricted",
            "pii_key_ref",
            "version",
            "props",
            "props_nodes",
        ] {
            assert!(
                CREATE_ISSUE_DDL.contains(col),
                "the issue lifecycle/GDPR/tail column `{col}` is present (recon §X-7 / §2)"
            );
        }
        // The pseudonymous identity fields are uuid (the principal id; the pseudonym map lives in
        // Identity — erasure-safe, §2).
        assert!(
            CREATE_ISSUE_DDL.contains("assignee               uuid")
                && CREATE_ISSUE_DDL.contains("reporter               uuid"),
            "assignee/reporter are pseudonymous principal ids (erasure-safe, 4.8)"
        );
    }

    /// **The frozen value-set vocabularies are CHECK constraints, not enum types (forward-only
    /// vocabulary extension, §9).** The INVARIANT `state_category` four-set (the cross-project
    /// reporting invariant, sketch 02), the relation `rel` six-set, and the `scheme.kind` five-set
    /// are CHECKs so a new value is a non-blocking CHECK add (or, for the closed category set, a
    /// deliberate non-extension), never an enum-rewrite.
    #[test]
    fn the_frozen_vocabularies_are_check_constraints() {
        let squash = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            squash(CREATE_ISSUE_DDL).contains(
                "state_category text NOT NULL CHECK (state_category IN ('unstarted','started','completed','cancelled'))"
            ),
            "issue.state_category is the frozen FOUR invariant categories (sketch 02)"
        );
        assert!(
            squash(CREATE_ISSUE_RELATION_DDL).contains(
                "rel text NOT NULL CHECK (rel IN ('parent','blocks','blocked_by','closes','depends_on','relates'))"
            ),
            "issue_relation.rel is the frozen six-relation vocabulary (§4)"
        );
        assert!(
            squash(CREATE_SCHEME_DDL).contains(
                "kind text NOT NULL CHECK (kind IN ('workflow','field','permission','sla','type'))"
            ),
            "scheme.kind is the frozen five-kind vocabulary (§3)"
        );
    }

    /// **`issue_relation` is the TE-7 source of truth: the FORWARD edge only, FK-anchored on the
    /// `src_issue` end, with the cross-subsystem `dst_ref` free (§4 / contract 5.5).** Refs
    /// materialises the inverse; we never dual-write it. The FK constrains only the source end (the
    /// far end may be a PR / doc / chat thread).
    #[test]
    fn issue_relation_is_the_forward_edge_fk_anchored_on_src() {
        assert!(
            CREATE_ISSUE_RELATION_DDL
                .contains("FOREIGN KEY (tenant_id, src_issue) REFERENCES issue(tenant_id, id) ON DELETE CASCADE"),
            "the FK constrains only the src_issue end (the dst_ref may be cross-subsystem)"
        );
        assert!(
            CREATE_ISSUE_RELATION_DDL.contains("UNIQUE (tenant_id, src_issue, dst_ref, rel)"),
            "the forward edge is unique per (src_issue, dst_ref, rel) — no dual-write"
        );
        assert!(
            CREATE_ISSUE_RELATION_DDL.contains("dst_ref     text"),
            "dst_ref is an ArtifactRef text (may be cross-subsystem, §4)"
        );
    }

    /// **The six `issue` hot-path indexes each ride a separate migration/query, built
    /// `CONCURRENTLY` on the declared-HOT table (arch 01 §2).** PostgreSQL refuses a concurrent
    /// index inside a multi-statement implicit transaction, so separate steps are part of the
    /// production boot contract, not merely cosmetic.
    #[test]
    fn the_six_issue_indexes_ride_the_migration_concurrently() {
        let migrations = issues_migrations();
        for (name, ddl) in CREATE_ISSUE_INDEXES_DDL {
            let matching: Vec<_> = migrations
                .0
                .iter()
                .filter(|m| m.table == Some(ISSUE_TABLE) && m.ddl == *ddl)
                .collect();
            assert_eq!(
                matching.len(),
                1,
                "index `{name}` has exactly one standalone migration"
            );
            assert_eq!(
                matching[0].ddl.matches(';').count(),
                0,
                "one SQL statement per concurrent-index query"
            );
        }
        // Every hot-table index is CONCURRENTLY (non-blocking against the declared-hot table).
        for (name, ddl) in CREATE_ISSUE_INDEXES_DDL {
            assert!(
                ddl.contains("CONCURRENTLY"),
                "issue index `{name}` is built CONCURRENTLY (hot-table expand discipline)"
            );
        }
        // The board index carries its soft-delete-aware predicate.
        let board = CREATE_ISSUE_INDEXES_DDL
            .iter()
            .find(|(n, _)| *n == ISSUE_BOARD_INDEX)
            .map(|(_, d)| *d)
            .unwrap();
        assert!(
            board.contains("(tenant_id, project_id, state_category, rank)")
                && board.contains("WHERE deleted_at IS NULL"),
            "issue_board is the tenant-first board scan over (project, category, rank), live-only"
        );
    }

    /// **The co-located `outbox` table is the frozen platform 2.3 shape (no fork) — the same-tx
    /// co-commit anchor.** Issues co-locates `myelin_events::OUTBOX_MIGRATION` in its own service DB
    /// (the cross-seam cursor); it does NOT author a second outbox shape (EI-01 §7). It rides without
    /// the tenant-scoped RLS call (it is the cell-wide relay cursor keyed on `(aggregate, seq)`).
    #[test]
    fn the_outbox_is_the_frozen_platform_shape_co_located() {
        let outbox = issues_migrations()
            .0
            .into_iter()
            .find(|m| m.table == Some(OUTBOX_TABLE))
            .unwrap();
        assert!(
            outbox.ddl.contains("CREATE TABLE IF NOT EXISTS outbox"),
            "the outbox is the frozen platform 2.3 table"
        );
        assert!(
            outbox.ddl.contains("UNIQUE (aggregate, seq)"),
            "the outbox carries the (aggregate, seq) co-commit ordering key"
        );
        // Byte-identical to the platform DDL (no Issues-local divergence).
        assert!(
            outbox.ddl.starts_with(OUTBOX_MIGRATION),
            "the outbox migration is the frozen myelin_events::OUTBOX_MIGRATION, verbatim"
        );
    }

    /// **The `consumer_dedup` ledger is the platform exactly-once shape (contract 2.5).**
    /// `(tenant_id, region)`-first with the `(consumer, event_id)` dedup PK — the platform consumer
    /// template's idempotency anchor (the bus consumers dedup on it). One shape, no fork (EI-01 §7).
    #[test]
    fn the_consumer_dedup_ledger_is_the_platform_2_5_shape() {
        for col in ["tenant_id", "region", "consumer", "event_id"] {
            assert!(
                CREATE_CONSUMER_DEDUP_DDL.contains(col),
                "the 2.5 column `{col}` is declared"
            );
        }
        assert!(
            CREATE_CONSUMER_DEDUP_DDL.contains("PRIMARY KEY (consumer, event_id)"),
            "the exactly-once dedup key is (consumer, event_id) — the platform consumer template"
        );
    }
}
