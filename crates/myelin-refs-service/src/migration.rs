use myelin_substrate::{Migration, MigrationPhase, Migrations};
use myelin_tenancy::{Region, TenantId};

use crate::dek::RefsDekPin;

pub const EDGE_TABLE: &str = "edge";

pub const EDGE_MIGRATION_ID: &str = "refs_0001_edge";
pub const EDGE_INBOUND_KEYSET_MIGRATION_ID: &str = "refs_0002_inbound_keyset";

pub const EDGE_INBOUND_INDEX: &str = "edge_inbound";
pub const EDGE_INBOUND_KEYSET_INDEX: &str = "edge_inbound_keyset";
pub const EDGE_OUTBOUND_INDEX: &str = "edge_outbound";
pub const EDGE_BY_REL_INDEX: &str = "edge_by_rel";

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

pub const CREATE_EDGE_INBOUND_KEYSET_INDEX_DDL: &str =
    "CREATE INDEX CONCURRENTLY IF NOT EXISTS edge_inbound_keyset \
     ON edge (tenant_id, region, target_root, edge_id) WHERE NOT tombstoned";

pub const MAKE_EDGE_TENANT_SCOPED_DDL: &str = "SELECT myelin_make_tenant_scoped('edge')";

pub fn edge_table_migrations() -> Migrations {
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
    let ddl: &'static str = Box::leak(ddl.into_boxed_str());
    Migrations::of([
        Migration::phased(EDGE_MIGRATION_ID, ddl, MigrationPhase::Plain, EDGE_TABLE),
        Migration::phased(
            EDGE_INBOUND_KEYSET_MIGRATION_ID,
            CREATE_EDGE_INBOUND_KEYSET_INDEX_DDL,
            MigrationPhase::Expand,
            EDGE_TABLE,
        ),
    ])
}

pub fn edge_table_dek_ref(
    dek: &RefsDekPin,
    tenant: &TenantId,
    region: &Region,
) -> Result<String, myelin_storage::KmsError> {
    Ok(dek.reserve(tenant, region)?.to_uri())
}

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
        assert!(
            ddl.contains("PRIMARY KEY (tenant_id, edge_id)"),
            "the primary key is tenant-first (tenant_id, edge_id) - §3.2"
        );
        assert!(
            ddl.contains("UNIQUE (tenant_id, source, target, rel)"),
            "the UNIQUE (tenant_id, source, target, rel) idempotency key is present - §3.2"
        );
    }

    #[test]
    fn the_three_indexes_carry_their_where_predicates() {
        let by_name = |n: &str| {
            CREATE_EDGE_INDEXES_DDL
                .iter()
                .find(|(name, _)| *name == n)
                .map(|(_, ddl)| *ddl)
                .unwrap()
        };
        let inbound = by_name(EDGE_INBOUND_INDEX);
        assert!(
            inbound.contains("(tenant_id, target_root)"),
            "edge_inbound keys (tenant_id, target_root)"
        );
        assert!(
            inbound.contains("WHERE NOT tombstoned"),
            "edge_inbound is live-edges-only (§3.2)"
        );
        let outbound = by_name(EDGE_OUTBOUND_INDEX);
        assert!(
            outbound.contains("(tenant_id, source_root)"),
            "edge_outbound keys (tenant_id, source_root)"
        );
        assert!(
            !outbound.contains("WHERE"),
            "edge_outbound has no partial predicate (§3.2)"
        );
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

    #[test]
    fn every_index_is_tenant_first() {
        for (name, ddl) in CREATE_EDGE_INDEXES_DDL {
            assert!(
                ddl.contains("(tenant_id,"),
                "index `{name}` must be tenant-first (no cross-tenant query path): {ddl}"
            );
        }
        assert!(CREATE_EDGE_INBOUND_KEYSET_INDEX_DDL
            .contains("ON edge (tenant_id, region, target_root, edge_id)"));
    }

    #[test]
    fn the_edge_migration_is_forward_only() {
        let migrations = edge_table_migrations();
        assert_eq!(migrations.0.len(), 2);
        let m = &migrations.0[0];
        assert_eq!(m.id, EDGE_MIGRATION_ID);
        assert_eq!(m.table, Some(EDGE_TABLE));
        assert_eq!(
            m.phase,
            MigrationPhase::Plain,
            "a CREATE TABLE is a plain forward migration"
        );
        assert!(
            edge_ddl_is_forward_only(m.ddl),
            "the edge migration is forward-only (no DROP)"
        );
        assert!(
            !m.ddl.to_ascii_uppercase().contains("DROP"),
            "no DROP in the edge migration"
        );
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
        let keyset = &migrations.0[1];
        assert_eq!(keyset.id, EDGE_INBOUND_KEYSET_MIGRATION_ID);
        assert_eq!(keyset.table, Some(EDGE_TABLE));
        assert_eq!(keyset.phase, MigrationPhase::Expand);
        assert_eq!(keyset.ddl, CREATE_EDGE_INBOUND_KEYSET_INDEX_DDL);
        assert!(keyset.ddl.contains("CREATE INDEX CONCURRENTLY"));
        assert!(edge_ddl_is_forward_only(keyset.ddl));
    }

    #[test]
    fn the_runner_admits_the_edge_migration() {
        use myelin_substrate::{HotTables, MigrationRunner};
        let migrations = edge_table_migrations();
        let mut runner = MigrationRunner::new();
        runner
            .run(&migrations, &HotTables::none())
            .expect("the edge schema migration applies forward-only");
        assert_eq!(
            runner.applied(),
            &[EDGE_MIGRATION_ID, EDGE_INBOUND_KEYSET_MIGRATION_ID],
            "the runner applied the edge schema and its online keyset index"
        );
    }

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

    #[test]
    fn the_edge_table_is_rls_on_and_tenant_region_partitioned() {
        assert_eq!(
            MAKE_EDGE_TENANT_SCOPED_DDL,
            "SELECT myelin_make_tenant_scoped('edge')"
        );
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

    #[test]
    fn the_edge_table_is_encrypted_from_birth_under_the_per_tenant_dek() {
        assert!(
            CREATE_EDGE_TABLE_DDL.contains("dek_ref"),
            "the edge table carries the per-row DEK ref"
        );
        let dek = RefsDekPin::new(Arc::new(KmsEngine::new()));
        let key_ref =
            edge_table_dek_ref(&dek, &t(), &r()).expect("reserve the edge table per-tenant DEK");
        assert_eq!(
            key_ref, "kms://acme/0/tenant",
            "the encrypted-from-birth per-tenant DEK ref (§3.7)"
        );
        let direct = dek.reserve(&t(), &r()).expect("reserve directly").to_uri();
        assert_eq!(
            key_ref, direct,
            "the edge table keys on the REF-P4 per-tenant DEK (one hierarchy)"
        );
    }
}
