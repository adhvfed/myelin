use std::sync::Arc;

use myelin_identity::Principal;
use myelin_storage::blob::BlobStore;
use myelin_storage::oltp::{OltpConfig, OltpError, OltpPool};
use myelin_storage::rls::{ResolvedTenant, TenantQuery, TenantScope, TenantTable};
use myelin_tenancy::{Region, TenantId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KnowledgeTable {
    Block,
    Page,
    DbCollection,
    DbRow,
    DbView,
    DbRelation,
    PageParent,
    DocOp,
    DocSnapshot,
}

impl KnowledgeTable {
    pub fn name(self) -> &'static str {
        match self {
            KnowledgeTable::Block => "block",
            KnowledgeTable::Page => "page",
            KnowledgeTable::DbCollection => "db_collection",
            KnowledgeTable::DbRow => "db_row",
            KnowledgeTable::DbView => "db_view",
            KnowledgeTable::DbRelation => "db_relation",
            KnowledgeTable::PageParent => "page_parent",
            KnowledgeTable::DocOp => "doc_op",
            KnowledgeTable::DocSnapshot => "doc_snapshot",
        }
    }

    pub fn is_hot(self) -> bool {
        matches!(
            self,
            KnowledgeTable::Block | KnowledgeTable::DbRow | KnowledgeTable::DocOp
        )
    }

    fn tenant_table(self) -> TenantTable {
        TenantTable::new(self.name())
    }

    pub const ALL: [KnowledgeTable; 9] = [
        KnowledgeTable::Block,
        KnowledgeTable::Page,
        KnowledgeTable::DbCollection,
        KnowledgeTable::DbRow,
        KnowledgeTable::DbView,
        KnowledgeTable::DbRelation,
        KnowledgeTable::PageParent,
        KnowledgeTable::DocOp,
        KnowledgeTable::DocSnapshot,
    ];
}

pub struct KnowledgeStore {
    pool: OltpPool,
    blobs: Arc<dyn BlobStore + Send + Sync>,
}

impl KnowledgeStore {
    pub fn open(
        config: OltpConfig,
        blobs: Arc<dyn BlobStore + Send + Sync>,
    ) -> Result<KnowledgeStore, OltpError> {
        let pool = OltpPool::open(config)?;
        Ok(KnowledgeStore { pool, blobs })
    }

    pub fn pool(&self) -> &OltpPool {
        &self.pool
    }

    pub fn blobs(&self) -> &(dyn BlobStore + Send + Sync) {
        self.blobs.as_ref()
    }

    pub fn query(&self, scope: TenantScope, table: KnowledgeTable) -> TenantQuery {
        TenantQuery::for_table(scope, table.tenant_table())
    }

    pub fn resolve_tenant(scope: &TenantScope, path_tenant: Option<&TenantId>) -> ResolvedTenant {
        scope.resolve(path_tenant)
    }
}

pub fn knowledge_scope(principal: &Principal, cell_region: Region) -> TenantScope {
    TenantScope::from_verified_token(principal, cell_region)
}

pub fn knowledge_store_migrations() -> myelin_substrate::Migrations {
    use myelin_substrate::{Migration, MigrationPhase};
    myelin_substrate::Migrations::of([
        Migration::plain(
            "0201_page",
            "CREATE TABLE IF NOT EXISTS page (\
               tenant text NOT NULL, region text NOT NULL, page_id text NOT NULL, \
               space_id text NOT NULL, parent_page text, title text NOT NULL, icon text, \
               is_folder boolean NOT NULL DEFAULT false, published boolean NOT NULL DEFAULT false, \
               archived boolean NOT NULL DEFAULT false, acl_zookie text, created_at text NOT NULL, \
               PRIMARY KEY (tenant, page_id))",
        ),
        Migration::phased(
            "0202_block",
            "CREATE TABLE IF NOT EXISTS block (\
               tenant text NOT NULL, region text NOT NULL, page_id text NOT NULL, \
               block_id text NOT NULL, parent_id text, order_key text NOT NULL, \
               block_type text NOT NULL CHECK (block_type IN (\
                 'paragraph','heading','bullet_list','ordered_list','task_list','blockquote',\
                 'code_block','callout','table','divider','image','embed','db_view','toggle',\
                 'sync_block')), \
               props text NOT NULL DEFAULT '{}', inline text NOT NULL DEFAULT '', \
               inline_nodes text NOT NULL DEFAULT '[]', \
               contains_personal_data boolean NOT NULL DEFAULT false, data_role text, \
               pii_key_ref text, created_by text NOT NULL, edited_by text NOT NULL, \
               created_at text NOT NULL, edited_at text NOT NULL, version bigint NOT NULL, \
               PRIMARY KEY (tenant, block_id))",
            MigrationPhase::Expand,
            "block",
        ),
        Migration::phased(
            "0203_block_children_index",
            "CREATE INDEX CONCURRENTLY IF NOT EXISTS block_children \
               ON block (tenant, page_id, parent_id, order_key)",
            MigrationPhase::Expand,
            "block",
        ),
        Migration::plain(
            "0204_db_collection",
            "CREATE TABLE IF NOT EXISTS db_collection (\
               tenant text NOT NULL, region text NOT NULL, db_id text NOT NULL, \
               space_id text NOT NULL, name text NOT NULL, field_defs text NOT NULL, \
               PRIMARY KEY (tenant, db_id))",
        ),
        Migration::phased(
            "0205_db_row",
            "CREATE TABLE IF NOT EXISTS db_row (\
               tenant text NOT NULL, region text NOT NULL, db_id text NOT NULL, \
               row_id text NOT NULL, props text NOT NULL, body_page text, order_key text NOT NULL, \
               version bigint NOT NULL, contains_personal_data boolean NOT NULL DEFAULT false, \
               data_role text, pii_key_ref text, created_at text NOT NULL, \
               PRIMARY KEY (tenant, row_id))",
            MigrationPhase::Expand,
            "db_row",
        ),
        Migration::phased(
            "0206_db_row_props_index",
            "CREATE INDEX CONCURRENTLY IF NOT EXISTS db_row_props_gin ON db_row (tenant, db_id)",
            MigrationPhase::Expand,
            "db_row",
        ),
        Migration::plain(
            "0207_db_view",
            "CREATE TABLE IF NOT EXISTS db_view (\
               tenant text NOT NULL, region text NOT NULL, view_id text NOT NULL, \
               db_id text NOT NULL, spec text NOT NULL, shared boolean NOT NULL DEFAULT true, \
               PRIMARY KEY (tenant, view_id))",
        ),
        Migration::plain(
            "0208_db_relation",
            "CREATE TABLE IF NOT EXISTS db_relation (\
               tenant text NOT NULL, region text NOT NULL, relation_id text NOT NULL, \
               src_row text NOT NULL, dst_ref text NOT NULL, rel text NOT NULL, \
               created_by text NOT NULL, created_at text NOT NULL, \
               PRIMARY KEY (tenant, relation_id))",
        ),
        Migration::plain(
            "0209_page_parent",
            "CREATE TABLE IF NOT EXISTS page_parent (\
               tenant text NOT NULL, region text NOT NULL, page_id text NOT NULL, \
               parent_page text NOT NULL, order_key text NOT NULL, \
               PRIMARY KEY (tenant, page_id))",
        ),
        Migration::phased(
            "0210_doc_op",
            "CREATE TABLE IF NOT EXISTS doc_op (\
               tenant text NOT NULL, region text NOT NULL, page_id text NOT NULL, \
               op_seq bigint NOT NULL, op_id text NOT NULL, actor text NOT NULL, \
               op_kind text NOT NULL, payload text NOT NULL, pii_key_ref text, \
               applied_at text NOT NULL, \
               PRIMARY KEY (tenant, page_id, op_seq))",
            MigrationPhase::Expand,
            "doc_op",
        ),
        Migration::phased(
            "0211_doc_op_resume_index",
            "CREATE INDEX CONCURRENTLY IF NOT EXISTS doc_op_resume \
               ON doc_op (tenant, page_id, op_seq)",
            MigrationPhase::Expand,
            "doc_op",
        ),
        Migration::plain(
            "0212_doc_snapshot",
            "CREATE TABLE IF NOT EXISTS doc_snapshot (\
               tenant text NOT NULL, region text NOT NULL, page_id text NOT NULL, \
               snap_seq bigint NOT NULL, blob_hash text NOT NULL, named_label text, \
               created_at text NOT NULL, \
               PRIMARY KEY (tenant, page_id, snap_seq))",
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalId, PrincipalKind};
    use myelin_substrate::{is_blocking_alter, is_destructive, HotTables, MigrationRunner};

    fn principal(tenant: &str) -> Principal {
        Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        )
    }

    fn cfg() -> OltpConfig {
        OltpConfig {
            max_pool_size: 8,
            statement_timeout_ms: 5_000,
            per_tenant_in_flight_cap: 4,
        }
    }

    #[test]
    fn store_opens_with_pool_and_blobs() {
        let store = KnowledgeStore::open(cfg(), Arc::new(myelin_storage::blob::FsBlobStore::new()))
            .expect("the knowledge store opens");
        assert_eq!(store.pool().config(), cfg());
        let acme = TenantId("acme".into());
        let h = store
            .blobs()
            .put(&acme, b"a snapshot blob")
            .expect("blob put");
        assert_eq!(
            store.blobs().get(&acme, &h).expect("blob get"),
            b"a snapshot blob",
            "the fs BlobStore round-trips a Knowledge media/snapshot blob"
        );
    }

    #[test]
    fn every_knowledge_query_carries_the_tenant_region_predicate() {
        let store = KnowledgeStore::open(cfg(), Arc::new(myelin_storage::blob::FsBlobStore::new()))
            .expect("open");
        let scope = knowledge_scope(&principal("acme"), Region::new("fr-par"));
        for table in KnowledgeTable::ALL {
            let q = store.query(scope.clone(), table);
            let sql = q.predicate_sql();
            assert!(
                sql.contains("tenant = $1 AND region = $2"),
                "{} query must pin the (tenant, region) via bind placeholders: {sql}",
                table.name()
            );
            assert_eq!(
                q.predicate_binds(),
                vec!["acme".to_string(), "fr-par".to_string()],
                "{} query binds carry the verified token's (tenant, region)",
                table.name()
            );
            assert!(
                sql.starts_with(table.name()),
                "{} query must target its own table: {sql}",
                table.name()
            );
            assert_eq!(
                q.validate(),
                Ok(()),
                "{} query is well-formed",
                table.name()
            );
        }
    }

    #[test]
    fn kn_d13_path_tenant_spoof_resolves_to_token_tenant_zero_cross_tenant() {
        let scope = knowledge_scope(&principal("acme"), Region::new("fr-par"));
        let resolved = KnowledgeStore::resolve_tenant(&scope, Some(&TenantId("evil-corp".into())));
        assert_eq!(
            resolved.tenant,
            TenantId("acme".into()),
            "the effective tenant is the token's, never the spoofed path tenant"
        );
        assert!(
            !resolved.path_derived,
            "path_derived_tenant_count == 0 - the tenant is NEVER taken from the path (KN-D13)"
        );
        assert!(
            resolved.attempted_path_mismatch,
            "the spoof attempt is flagged (the guard held, the read stays in the token's tenant)"
        );
    }

    #[test]
    fn kn_d13_matching_and_absent_path_resolve_to_token_no_mismatch() {
        let scope = knowledge_scope(&principal("acme"), Region::new("fr-par"));
        let matching = KnowledgeStore::resolve_tenant(&scope, Some(&TenantId("acme".into())));
        assert_eq!(matching.tenant, TenantId("acme".into()));
        assert!(!matching.path_derived);
        assert!(
            !matching.attempted_path_mismatch,
            "matching tenants are not a spoof"
        );

        let internal = KnowledgeStore::resolve_tenant(&scope, None);
        assert_eq!(internal.tenant, TenantId("acme".into()));
        assert!(!internal.path_derived);
        assert!(!internal.attempted_path_mismatch);
    }

    #[test]
    fn store_schema_is_forward_only() {
        for m in &knowledge_store_migrations().0 {
            assert!(
                !is_destructive(m.ddl),
                "store migration {} is forward-only (no DROP)",
                m.id
            );
        }
    }

    #[test]
    fn store_schema_applies_and_hot_table_changes_are_online() {
        let hot = HotTables::declare(crate::HOT_TABLES);
        let mut runner = MigrationRunner::new();
        runner
            .run(&knowledge_store_migrations(), &hot)
            .expect("the whole Knowledge store schema applies (no blocking ALTER on a hot table)");
        for m in &knowledge_store_migrations().0 {
            if let Some(table) = m.table {
                if hot.is_hot(table) {
                    assert!(
                        !is_blocking_alter(m.ddl),
                        "hot-table migration {} must be online (no blocking ALTER): {}",
                        m.id,
                        m.ddl
                    );
                }
            }
        }
    }

    #[test]
    fn store_hot_tables_match_the_appspec_declaration() {
        let mut store_hot: Vec<&str> = KnowledgeTable::ALL
            .iter()
            .filter(|t| t.is_hot())
            .map(|t| t.name())
            .collect();
        store_hot.sort_unstable();
        let mut declared: Vec<&str> = crate::HOT_TABLES.to_vec();
        declared.sort_unstable();
        assert_eq!(
            store_hot, declared,
            "the store's hot tables == the AppSpec hot-table declaration (block/db_row/doc_op)"
        );
    }

    #[test]
    fn every_table_is_tenant_region_partitioned() {
        for m in &knowledge_store_migrations().0 {
            if m.ddl.contains("CREATE TABLE") {
                assert!(
                    m.ddl.contains("tenant text NOT NULL")
                        && m.ddl.contains("region text NOT NULL"),
                    "table migration {} must declare the (tenant, region) partition columns",
                    m.id
                );
                assert!(
                    m.ddl.contains("PRIMARY KEY (tenant"),
                    "table migration {} must lead its primary key with the tenant partition key",
                    m.id
                );
            }
        }
        let creates = knowledge_store_migrations()
            .0
            .iter()
            .filter(|m| m.ddl.contains("CREATE TABLE"))
            .count();
        assert_eq!(
            creates,
            KnowledgeTable::ALL.len(),
            "exactly the 9 v1 Knowledge tables"
        );
    }
}
