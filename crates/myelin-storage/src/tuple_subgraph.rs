//! Reads the part of a tenant's relationship graph that a permission question can reach.
//!
//! A check on an object consults that object's tuples and, through every userset subject
//! (`object#relation`), the tuples of the object the userset names. Starting from the objects
//! being asked about and following usersets hop by hop reads exactly those tuples, so a check
//! costs a few indexed queries however many tuples the tenant holds.

use std::collections::BTreeSet;

use sqlx::Row;

use crate::identity_durable::DurableTupleBacking;
use crate::migration::{Migration, MigrationPhase, Migrations};
use crate::pg::PgError;
use crate::provider::ProviderError;

/// Usersets are found per object without walking the object's direct grants, which a large
/// organization or channel has thousands of.
pub const REBAC_TUPLE_USERSET_INDEX_MIGRATION: &str = "\
CREATE INDEX CONCURRENTLY IF NOT EXISTS rebac_tuple_userset_edges
    ON rebac_tuple (tenant_id, region, object_id, relation, subject)
    WHERE strpos(subject, '#') > 0;";

/// A check's own grants are found per object and subject without walking every grant on the
/// object or every grant the subject holds.
pub const REBAC_TUPLE_OBJECT_SUBJECT_INDEX_MIGRATION: &str = "\
CREATE INDEX CONCURRENTLY IF NOT EXISTS rebac_tuple_object_subject
    ON rebac_tuple (tenant_id, region, object_id, subject);";

/// Fails boot when a concurrently built `rebac_tuple` index is missing or was left invalid.
fn validate_rebac_tuple_index(index: &str) -> String {
    format!(
        "\
DO $myelin$
BEGIN
  IF NOT EXISTS (
    SELECT 1
      FROM pg_catalog.pg_index AS index_state
      JOIN pg_catalog.pg_class AS index_relation
        ON index_relation.oid = index_state.indexrelid
      JOIN pg_catalog.pg_class AS table_relation
        ON table_relation.oid = index_state.indrelid
      JOIN pg_catalog.pg_namespace AS relation_namespace
        ON relation_namespace.oid = table_relation.relnamespace
     WHERE relation_namespace.nspname = current_schema()
       AND table_relation.relname = 'rebac_tuple'
       AND table_relation.relkind = 'r'
       AND index_relation.relnamespace = relation_namespace.oid
       AND index_relation.relname = '{index}'
       AND index_relation.relkind = 'i'
       AND index_state.indisvalid
       AND index_state.indisready
  ) THEN
    RAISE EXCEPTION '{index} on %.rebac_tuple is missing, invalid, or not ready; verify the index exists, then repair it with REINDEX INDEX CONCURRENTLY %.{index} before restarting', current_schema(), current_schema();
  END IF;
END
$myelin$"
    )
}

pub fn rebac_tuple_subgraph_migrations() -> Migrations {
    Migrations::of([
        Migration::phased(
            "0191_rebac_tuple_userset_edge_index",
            REBAC_TUPLE_USERSET_INDEX_MIGRATION,
            MigrationPhase::Expand,
            "rebac_tuple",
        ),
        Migration::phased(
            "0192_validate_rebac_tuple_userset_edge_index",
            validate_rebac_tuple_index("rebac_tuple_userset_edges"),
            MigrationPhase::Expand,
            "rebac_tuple",
        ),
        Migration::phased(
            "0193_rebac_tuple_object_subject_index",
            REBAC_TUPLE_OBJECT_SUBJECT_INDEX_MIGRATION,
            MigrationPhase::Expand,
            "rebac_tuple",
        ),
        Migration::phased(
            "0194_validate_rebac_tuple_object_subject_index",
            validate_rebac_tuple_index("rebac_tuple_object_subject"),
            MigrationPhase::Expand,
            "rebac_tuple",
        ),
    ])
}

/// Where a subgraph read starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubgraphRoots {
    /// The objects being asked about, in any spelling an object key accepts.
    Objects(Vec<String>),
    /// Every object of one type (`type:` keys), for listing what a subject can reach.
    ObjectType(String),
}

/// Which tuples a subgraph read keeps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubgraphSubjects {
    /// Every tuple on a reached object: an expansion lists them all.
    All,
    /// Usersets plus the tuples that name this subject directly: all a check for it consults.
    Only(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubgraphEdge {
    pub object: String,
    pub relation: String,
    pub subject: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TupleSubgraph {
    pub edges: Vec<SubgraphEdge>,
    pub revision: u64,
}

/// The object a userset subject names (`object#relation`), when the subject is one.
pub fn userset_object(subject: &str) -> Option<&str> {
    let (object, relation) = subject.split_once('#')?;
    (!object.is_empty() && !relation.is_empty() && !relation.contains('#')).then_some(object)
}

/// The key a tuple object is stored and looked up under.
pub fn tuple_object_key(object: &str) -> String {
    myelin_refs::object_key(&myelin_events::ArtifactRef(object.to_string()))
        .map(|key| key.tuple_key())
        .unwrap_or_else(|| object.to_string())
}

/// Whether `object` is the key it would be stored under: non-empty, no surrounding whitespace
/// (as `str::trim` sees it), no `#` anchor and not an artifact reference. Every key
/// `tuple_object_key` returns has this shape, and every object of this shape is its own key.
pub fn is_canonical_tuple_object(object: &str) -> bool {
    !object.is_empty()
        && object.trim() == object
        && !object.contains('#')
        && !object.starts_with(myelin_refs::SCHEME)
}

/// The key a tuple on `object` is stored under, or `None` when the object has no key a check
/// could ever find it by.
pub fn canonical_tuple_object(object: &str) -> Option<String> {
    let key = tuple_object_key(object);
    is_canonical_tuple_object(&key).then_some(key)
}

/// `is_canonical_tuple_object` as SQL over `column`. The whitespace class is exactly Unicode
/// `White_Space`, which `char::is_whitespace` (and so `str::trim`) tests.
fn canonical_object_sql(column: &str) -> String {
    const WHITESPACE: &str =
        r"[\0009-\000D\0020\0085\00A0\1680\2000-\200A\2028\2029\202F\205F\3000]";
    format!(
        "({column} <> '' AND strpos({column}, '#') = 0 \
         AND NOT starts_with({column}, 'myelin://') \
         AND {column} !~ U&'^{WHITESPACE}' AND {column} !~ U&'{WHITESPACE}$')"
    )
}

/// Refuses relationship rows whose object is not its own key, so the exact-key reads above see
/// every tuple. Added unvalidated; `0196` rewrites older rows to their keys, then validates.
fn object_key_constraints_ddl() -> String {
    format!(
        "ALTER TABLE rebac_tuple ADD CONSTRAINT rebac_tuple_object_is_key \
           CHECK {tuple} NOT VALID;\n\
         ALTER TABLE rebac_object_revision ADD CONSTRAINT rebac_object_revision_object_is_key \
           CHECK {revision} NOT VALID;",
        tuple = canonical_object_sql("object_id"),
        revision = canonical_object_sql("object_id"),
    )
}

pub const CANONICALIZE_RELATIONSHIP_OBJECTS_MIGRATION_ID: &str =
    "0196_canonicalize_relationship_objects";

/// Fails, so the migration and boot fail, while any row is still not stored under its key.
pub const VALIDATE_OBJECT_KEY_CONSTRAINTS_DDL: &str = "\
ALTER TABLE rebac_tuple VALIDATE CONSTRAINT rebac_tuple_object_is_key;
ALTER TABLE rebac_object_revision VALIDATE CONSTRAINT rebac_object_revision_object_is_key;";

pub fn relationship_object_key_migrations() -> Migrations {
    Migrations::of([
        Migration::phased(
            "0195_relationship_object_key_constraints",
            object_key_constraints_ddl(),
            MigrationPhase::Expand,
            "rebac_tuple",
        ),
        Migration::phased(
            CANONICALIZE_RELATIONSHIP_OBJECTS_MIGRATION_ID,
            VALIDATE_OBJECT_KEY_CONSTRAINTS_DDL,
            MigrationPhase::Expand,
            "rebac_tuple",
        ),
    ])
}

/// Rewrites every relationship row stored under another spelling of its object (an artifact
/// reference, an anchored or padded id) to the object's key. Rows that meet on the same key
/// merge: a tuple keeps the longest lifetime, a revision the highest. A row whose object has no
/// key cannot be rewritten; it is reported and the migration fails rather than guessing.
///
/// Runs inside the migration lock, as the migration role, before `0196` validates the key
/// constraints.
pub(crate) async fn canonicalize_relationship_objects(
    conn: &mut sqlx::PgConnection,
) -> Result<(), PgError> {
    let query_error =
        |error: sqlx::Error| PgError::Migrate(format!("canonicalize tuples: {error}"));
    let not_canonical = format!("NOT {}", canonical_object_sql("object_id"));
    let mut tx = sqlx::Connection::begin(&mut *conn)
        .await
        .map_err(query_error)?;
    let tuples: Vec<(String, String, String, String, String)> = sqlx::query_as(&format!(
        "SELECT tenant_id, region, object_id, relation, subject FROM rebac_tuple \
         WHERE {not_canonical}"
    ))
    .fetch_all(&mut *tx)
    .await
    .map_err(query_error)?;
    let revisions: Vec<(String, String, String)> = sqlx::query_as(&format!(
        "SELECT tenant_id, region, object_id FROM rebac_object_revision WHERE {not_canonical}"
    ))
    .fetch_all(&mut *tx)
    .await
    .map_err(query_error)?;
    let keyless = tuples
        .iter()
        .map(|row| &row.2)
        .chain(revisions.iter().map(|row| &row.2))
        .filter(|object| canonical_tuple_object(object).is_none())
        .collect::<BTreeSet<_>>();
    if !keyless.is_empty() {
        return Err(PgError::Migrate(format!(
            "{} relationship object(s) have no key a check could find them by, e.g. {:?}; \
             rewrite or delete them deliberately, then restart",
            keyless.len(),
            keyless.iter().take(3).collect::<Vec<_>>()
        )));
    }
    for (tenant, region, object, relation, subject) in &tuples {
        let key = canonical_tuple_object(object).unwrap_or_default();
        sqlx::query(
            "INSERT INTO rebac_tuple (tenant_id, region, object_id, relation, subject, expires_at) \
             SELECT tenant_id, region, $6, relation, subject, expires_at FROM rebac_tuple \
             WHERE tenant_id = $1 AND region = $2 AND object_id = $3 AND relation = $4 \
               AND subject = $5 \
             ON CONFLICT (tenant_id, region, object_id, relation, subject) DO UPDATE \
             SET expires_at = CASE \
               WHEN rebac_tuple.expires_at IS NULL OR EXCLUDED.expires_at IS NULL THEN NULL \
               ELSE GREATEST(rebac_tuple.expires_at, EXCLUDED.expires_at) END",
        )
        .bind(tenant)
        .bind(region)
        .bind(object)
        .bind(relation)
        .bind(subject)
        .bind(&key)
        .execute(&mut *tx)
        .await
        .map_err(query_error)?;
        sqlx::query(
            "DELETE FROM rebac_tuple WHERE tenant_id = $1 AND region = $2 AND object_id = $3 \
               AND relation = $4 AND subject = $5",
        )
        .bind(tenant)
        .bind(region)
        .bind(object)
        .bind(relation)
        .bind(subject)
        .execute(&mut *tx)
        .await
        .map_err(query_error)?;
    }
    for (tenant, region, object) in &revisions {
        let key = canonical_tuple_object(object).unwrap_or_default();
        sqlx::query(
            "INSERT INTO rebac_object_revision (tenant_id, region, object_id, revision) \
             SELECT tenant_id, region, $4, revision FROM rebac_object_revision \
             WHERE tenant_id = $1 AND region = $2 AND object_id = $3 \
             ON CONFLICT (tenant_id, region, object_id) DO UPDATE \
             SET revision = GREATEST(rebac_object_revision.revision, EXCLUDED.revision)",
        )
        .bind(tenant)
        .bind(region)
        .bind(object)
        .bind(&key)
        .execute(&mut *tx)
        .await
        .map_err(query_error)?;
        sqlx::query(
            "DELETE FROM rebac_object_revision \
             WHERE tenant_id = $1 AND region = $2 AND object_id = $3",
        )
        .bind(tenant)
        .bind(region)
        .bind(object)
        .execute(&mut *tx)
        .await
        .map_err(query_error)?;
    }
    tx.commit().await.map_err(query_error)
}

/// Tracks which objects a walk has read and which it must read next.
#[derive(Default)]
pub struct SubgraphFrontier {
    seen: BTreeSet<String>,
    next: Vec<String>,
}

impl SubgraphFrontier {
    /// Queues an object under both its spelling and its key: checks look usersets up by the
    /// spelling the userset carries, while rewrites normalise it first.
    pub fn push(&mut self, object: &str) {
        for candidate in [object.to_string(), tuple_object_key(object)] {
            if self.seen.insert(candidate.clone()) {
                self.next.push(candidate);
            }
        }
    }

    pub fn push_usersets<'a>(&mut self, subjects: impl IntoIterator<Item = &'a str>) {
        for subject in subjects {
            if let Some(object) = userset_object(subject) {
                self.push(object);
            }
        }
    }

    /// Records an object whose tuples were already read, so no later hop reads it again.
    pub fn mark_read(&mut self, object: &str) {
        self.seen.insert(object.to_string());
    }

    pub fn take(&mut self) -> Vec<String> {
        std::mem::take(&mut self.next)
    }
}

pub const TOO_DEEP: &str = "the relationship graph reaches further than a check can evaluate";

const LIVE: &str = "(expires_at IS NULL OR expires_at > CURRENT_TIMESTAMP)";

/// Which objects one hop reads.
enum HopObjects<'a> {
    Listed(&'a [String]),
    OfType(&'a str),
}

impl HopObjects<'_> {
    fn predicate(&self) -> &'static str {
        match self {
            HopObjects::Listed(_) => "object_id = ANY($3)",
            HopObjects::OfType(_) => "(starts_with(object_id, $3) OR object_id = $4)",
        }
    }

    fn subject_param(&self) -> &'static str {
        match self {
            HopObjects::Listed(_) => "$4",
            HopObjects::OfType(_) => "$5",
        }
    }
}

async fn read_hop(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    objects: HopObjects<'_>,
    subjects: &SubgraphSubjects,
) -> Result<Vec<SubgraphEdge>, PgError> {
    let base = "SELECT object_id, relation, subject FROM rebac_tuple \
                WHERE tenant_id = $1 AND region = $2";
    let predicate = objects.predicate();
    // Two branches so each is index-driven: usersets through the partial userset index, the
    // subject's own grants through the reverse (subject-first) index.
    // Rows come back in key order, so evaluation never depends on how a plan interleaves them.
    let sql = match subjects {
        SubgraphSubjects::All => {
            format!("{base} AND {predicate} AND {LIVE} ORDER BY object_id, relation, subject")
        }
        SubgraphSubjects::Only(_) => format!(
            "SELECT object_id, relation, subject FROM ( \
             {base} AND {predicate} AND strpos(subject, '#') > 0 AND {LIVE} \
             UNION ALL \
             {base} AND subject = {subject} AND {predicate} AND {LIVE} \
             ) AS hop ORDER BY object_id, relation, subject",
            subject = objects.subject_param(),
        ),
    };
    let mut query = sqlx::query(&sql).bind(tenant).bind(region);
    query = match objects {
        HopObjects::Listed(objects) => query.bind(objects.to_vec()),
        HopObjects::OfType(object_type) => query
            .bind(format!("{object_type}:"))
            .bind(object_type.to_string()),
    };
    if let SubgraphSubjects::Only(subject) = subjects {
        query = query.bind(subject.clone());
    }
    let rows = query
        .fetch_all(&mut *conn)
        .await
        .map_err(|error| PgError::Query(error.to_string()))?;
    Ok(rows
        .iter()
        .map(|row| SubgraphEdge {
            object: row.get("object_id"),
            relation: row.get("relation"),
            subject: row.get("subject"),
        })
        .collect())
}

impl DurableTupleBacking {
    /// Reads the tuples a check on `roots` can reach, following usersets for at most `max_hops`
    /// hops. Every hop runs in one repeatable-read transaction, so the result is the committed
    /// state at one instant, as a whole-tenant snapshot would be. A graph that still reaches
    /// further is an error, never a silently shortened read: a dropped tuple could be the one an
    /// exclusion subtracts.
    pub async fn subgraph_in(
        &self,
        tenant: &str,
        region: &str,
        roots: SubgraphRoots,
        subjects: SubgraphSubjects,
        max_hops: usize,
    ) -> Result<TupleSubgraph, ProviderError> {
        let tenant_owned = tenant.to_string();
        let region_owned = region.to_string();
        self.provider()
            .with_tenant_read_snapshot_tx(tenant, move |conn| {
                Box::pin(async move {
                    let (tenant, region) = (tenant_owned.as_str(), region_owned.as_str());
                    let revision: i64 = sqlx::query_scalar(
                        "SELECT COALESCE((SELECT revision FROM rebac_revision \
                                          WHERE tenant_id = $1 AND region = $2), 0)",
                    )
                    .bind(tenant)
                    .bind(region)
                    .fetch_one(&mut *conn)
                    .await
                    .map_err(|error| PgError::Query(error.to_string()))?;
                    let revision = u64::try_from(revision)
                        .map_err(|_| PgError::Query("relationship revision is negative".into()))?;

                    let mut frontier = SubgraphFrontier::default();
                    // How many distances from the roots have been read.
                    let mut distances_read = 0;
                    let mut hop = match &roots {
                        SubgraphRoots::Objects(objects) => {
                            for object in objects {
                                frontier.push(object);
                            }
                            Vec::new()
                        }
                        SubgraphRoots::ObjectType(object_type) => {
                            let edges = read_hop(
                                conn,
                                tenant,
                                region,
                                HopObjects::OfType(object_type),
                                &subjects,
                            )
                            .await?;
                            for edge in &edges {
                                frontier.mark_read(&edge.object);
                            }
                            distances_read = 1;
                            edges
                        }
                    };
                    let mut edges = Vec::new();
                    loop {
                        frontier.push_usersets(hop.iter().map(|edge| edge.subject.as_str()));
                        edges.append(&mut hop);
                        let objects = frontier.take();
                        if objects.is_empty() {
                            break;
                        }
                        if distances_read > max_hops {
                            return Err(PgError::Query(TOO_DEEP.into()));
                        }
                        distances_read += 1;
                        hop = read_hop(
                            conn,
                            tenant,
                            region,
                            HopObjects::Listed(&objects),
                            &subjects,
                        )
                        .await?;
                    }
                    Ok(TupleSubgraph { edges, revision })
                })
            })
            .await
    }
}
