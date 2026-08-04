use std::collections::{HashSet, VecDeque};

use myelin_identity::{ListObjectsResult, Principal, SetExpr};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};

use crate::backlinks::{set_expr_admits, AuthzVisibleIndex};
use crate::edge_builder::{EdgeProjection, RelClass};

pub const TRAVERSE_DEPTH_CEILING: u32 = 16;

pub const TRAVERSE_MAX_NODES: u32 = 10_000;

pub fn depth_ceiling_from_thresholds(t: &myelin_substrate::Thresholds) -> u32 {
    t.refs_traverse.depth_ceiling
}

pub fn max_nodes_from_thresholds(t: &myelin_substrate::Thresholds) -> u32 {
    t.refs_traverse.max_nodes
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TraverseFilter {
    pub rels: Vec<String>,
    pub rel_class: Option<RelClass>,
}

impl TraverseFilter {
    pub fn any() -> TraverseFilter {
        TraverseFilter::default()
    }

    pub fn rels(rels: &[&str]) -> TraverseFilter {
        TraverseFilter {
            rels: rels.iter().map(|s| (*s).to_string()).collect(),
            rel_class: None,
        }
    }

    fn admits(&self, rel: &str, rel_class: RelClass) -> bool {
        let rel_ok = self.rels.is_empty() || self.rels.iter().any(|r| r == rel);
        let class_ok = self.rel_class.map(|c| c == rel_class).unwrap_or(true);
        rel_ok && class_ok
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraverseNode {
    pub artifact: ArtifactRef,
    pub depth: u32,
    pub via_rel: String,
    pub via_rel_class: RelClass,
    pub parent: ArtifactRef,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraverseResult {
    pub nodes: Vec<TraverseNode>,
    pub truncated: bool,
    pub cycle_detected: bool,
    pub visited_count: usize,
}

#[derive(Clone)]
pub struct Traverse {
    edges: EdgeProjection,
    authz: AuthzVisibleIndex,
    depth_ceiling: u32,
    max_nodes: u32,
}

impl Traverse {
    pub fn new(
        edges: EdgeProjection,
        authz: AuthzVisibleIndex,
        thresholds: &myelin_substrate::Thresholds,
    ) -> Traverse {
        Traverse {
            edges,
            authz,
            depth_ceiling: depth_ceiling_from_thresholds(thresholds),
            max_nodes: max_nodes_from_thresholds(thresholds),
        }
    }

    pub fn with_default_bounds(edges: EdgeProjection, authz: AuthzVisibleIndex) -> Traverse {
        Traverse {
            edges,
            authz,
            depth_ceiling: TRAVERSE_DEPTH_CEILING,
            max_nodes: TRAVERSE_MAX_NODES,
        }
    }

    pub fn depth_ceiling(&self) -> u32 {
        self.depth_ceiling
    }

    pub fn max_nodes(&self) -> u32 {
        self.max_nodes
    }

    pub fn edge_projection(&self) -> &EdgeProjection {
        &self.edges
    }

    pub fn authz_index(&self) -> &AuthzVisibleIndex {
        &self.authz
    }

    #[allow(clippy::too_many_arguments)]
    pub fn traverse(
        &self,
        tenant: &TenantId,
        region: &Region,
        root: &ArtifactRef,
        viewer: &Principal,
        filter: &TraverseFilter,
        depth: u32,
        list_objects: &ListObjectsResult,
    ) -> TraverseResult {
        let effective_ceiling = depth.min(self.depth_ceiling);

        let mut visited: HashSet<String> = HashSet::new();
        visited.insert(root.0.clone());
        let mut discovered: Vec<TraverseNode> = Vec::new();
        let mut frontier: VecDeque<(ArtifactRef, u32)> = VecDeque::new();
        frontier.push_back((root.clone(), 0));

        let mut truncated = false;
        let mut cycle_detected = false;

        while let Some((node, node_depth)) = frontier.pop_front() {
            if visited.len() as u32 >= self.max_nodes {
                truncated = true;
                break;
            }

            if node_depth >= effective_ceiling {
                if self.has_followable_outbound(tenant, region, &node, filter) {
                    truncated = true;
                }
                continue;
            }

            for edge in self.edges.outbound_live(tenant, region, &node) {
                if !filter.admits(&edge.rel, edge.rel_class) {
                    continue;
                }
                let neighbour = edge.target_root.clone();

                if visited.contains(&neighbour.0) {
                    cycle_detected = true;
                    continue;
                }

                if visited.len() as u32 >= self.max_nodes {
                    truncated = true;
                    break;
                }

                visited.insert(neighbour.0.clone());
                let child_depth = node_depth + 1;
                discovered.push(TraverseNode {
                    artifact: neighbour.clone(),
                    depth: child_depth,
                    via_rel: edge.rel.clone(),
                    via_rel_class: edge.rel_class,
                    parent: node.clone(),
                });
                frontier.push_back((neighbour, child_depth));
            }
        }

        let visited_count = visited.len();

        let nodes = apply_post_filter(
            &discovered,
            list_objects,
            &self.authz,
            viewer,
            tenant,
            region,
        );

        TraverseResult {
            nodes,
            truncated,
            cycle_detected,
            visited_count,
        }
    }

    fn has_followable_outbound(
        &self,
        tenant: &TenantId,
        region: &Region,
        node: &ArtifactRef,
        filter: &TraverseFilter,
    ) -> bool {
        self.edges
            .outbound_live(tenant, region, node)
            .iter()
            .any(|e| filter.admits(&e.rel, e.rel_class))
    }
}

pub fn apply_post_filter(
    discovered: &[TraverseNode],
    list_objects: &ListObjectsResult,
    authz: &AuthzVisibleIndex,
    viewer: &Principal,
    tenant: &TenantId,
    region: &Region,
) -> Vec<TraverseNode> {
    let set_expr = set_expr_of(list_objects);

    let mut admitted_ids: HashSet<String> = HashSet::new();
    let mut out: Vec<TraverseNode> = Vec::new();

    for node in discovered {
        let parent_reachable = node.depth == 1 || admitted_ids.contains(&node.parent.0);
        if !parent_reachable {
            continue;
        }
        let readable = set_expr_admits(&set_expr, authz, viewer, tenant, region, &node.artifact);
        if !readable {
            continue;
        }
        admitted_ids.insert(node.artifact.0.clone());
        out.push(node.clone());
    }

    out.sort_by(|a, b| (a.depth, &a.artifact.0).cmp(&(b.depth, &b.artifact.0)));
    out
}

fn set_expr_of(list_objects: &ListObjectsResult) -> SetExpr {
    match list_objects {
        ListObjectsResult::Ids { ids, .. } => SetExpr::Ids(ids.clone()),
        ListObjectsResult::Filter { set_expr, .. } => set_expr.clone(),
    }
}

#[cfg(test)]
mod tests;
