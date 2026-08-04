use std::collections::HashMap;
use std::sync::Arc;

use myelin_storage::placement_durable::DurablePlacementBacking;
use myelin_tenancy::CellId;

use crate::cross_cell_bridge::{CellLocalResolver, CellResolverRegistry, ResolverProjection};

pub type ResolverFactory =
    dyn Fn(&CellId, &str) -> Result<Arc<dyn CellLocalResolver>, String> + Send + Sync;

struct ProjectedResolverSet {
    resolvers: HashMap<CellId, Arc<dyn CellLocalResolver>>,
}

impl ResolverProjection for ProjectedResolverSet {
    fn resolver_for(&self, cell: &CellId) -> Option<Arc<dyn CellLocalResolver>> {
        self.resolvers.get(cell).cloned()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectionError {
    MissingEndpoint {
        cell_id: String,
    },
    Unresolvable {
        cell_id: String,
        why: String,
    },
    Db(String),
}

impl core::fmt::Display for ProjectionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ProjectionError::MissingEndpoint { cell_id } => write!(
                f,
                "cross-cell resolver projection REFUSED: cell `{cell_id}` has an empty/missing routing \
                 endpoint - fail loud rather than project a silently-unreachable cell (never a silent \
                 empty registry)"
            ),
            ProjectionError::Unresolvable { cell_id, why } => write!(
                f,
                "cross-cell resolver projection REFUSED: could not construct a resolver handle for cell \
                 `{cell_id}` from its durable endpoint: {why}"
            ),
            ProjectionError::Db(why) => write!(
                f,
                "cross-cell resolver projection FAILED (durable `cell` read error, projection NOT built): \
                 {why}"
            ),
        }
    }
}

impl std::error::Error for ProjectionError {}

impl CellResolverRegistry {
    pub async fn project_from_durable_cells(
        backing: &DurablePlacementBacking,
        factory: &ResolverFactory,
    ) -> Result<CellResolverRegistry, ProjectionError> {
        let cells = backing
            .all_cells()
            .await
            .map_err(|e| ProjectionError::Db(e.to_string()))?;

        let mut resolvers: HashMap<CellId, Arc<dyn CellLocalResolver>> = HashMap::new();
        for row in &cells {
            if row.endpoint.trim().is_empty() {
                return Err(ProjectionError::MissingEndpoint {
                    cell_id: row.cell_id.clone(),
                });
            }
            let cell = CellId::from_token(&row.cell_id);
            let handle = factory(&cell, &row.endpoint).map_err(|why| ProjectionError::Unresolvable {
                cell_id: row.cell_id.clone(),
                why,
            })?;
            resolvers.insert(cell, handle);
        }
        Ok(CellResolverRegistry::projected(Arc::new(ProjectedResolverSet {
            resolvers,
        })))
    }
}
