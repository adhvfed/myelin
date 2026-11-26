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
    let sql = match subjects {
        SubgraphSubjects::All => format!("{base} AND {predicate} AND {LIVE}"),
        SubgraphSubjects::Only(_) => format!(
            "{base} AND {predicate} AND strpos(subject, '#') > 0 AND {LIVE} \
             UNION ALL \
             {base} AND subject = {subject} AND {predicate} AND {LIVE}",
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
    /// state at one instant, as a whole-tenant snapshot would be.
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
                            edges
                        }
                    };
                    let mut edges = Vec::new();
                    for _ in 0..=max_hops {
                        frontier.push_usersets(hop.iter().map(|edge| edge.subject.as_str()));
                        edges.append(&mut hop);
                        let objects = frontier.take();
                        if objects.is_empty() {
                            break;
                        }
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
