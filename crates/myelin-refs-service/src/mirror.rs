use std::collections::HashSet;

use myelin_refs::{strip_sub, ArtifactRef};
use myelin_tenancy::{Region, TenantId};

use crate::edge_builder::{edge_id, EdgeProjection, EdgeRow, RelClass};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LifecycleRel {
    Closes,
    Blocks,
    BlockedBy,
    DependsOn,
    Parent,
    Child,
    Assigns,
    Relates,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inverse {
    Paired(LifecycleRel),
    Symmetric,
    None,
}

impl LifecycleRel {
    pub fn as_str(self) -> &'static str {
        match self {
            LifecycleRel::Closes => "closes",
            LifecycleRel::Blocks => "blocks",
            LifecycleRel::BlockedBy => "blocked_by",
            LifecycleRel::DependsOn => "depends_on",
            LifecycleRel::Parent => "parent",
            LifecycleRel::Child => "child",
            LifecycleRel::Assigns => "assigns",
            LifecycleRel::Relates => "relates",
        }
    }

    pub fn parse(token: &str) -> Option<LifecycleRel> {
        match token {
            "closes" => Some(LifecycleRel::Closes),
            "blocks" => Some(LifecycleRel::Blocks),
            "blocked_by" => Some(LifecycleRel::BlockedBy),
            "depends_on" => Some(LifecycleRel::DependsOn),
            "parent" => Some(LifecycleRel::Parent),
            "child" => Some(LifecycleRel::Child),
            "assigns" => Some(LifecycleRel::Assigns),
            "relates" => Some(LifecycleRel::Relates),
            _ => None,
        }
    }

    pub fn inverse(self) -> Inverse {
        match self {
            LifecycleRel::Blocks => Inverse::Paired(LifecycleRel::BlockedBy),
            LifecycleRel::BlockedBy => Inverse::Paired(LifecycleRel::Blocks),
            LifecycleRel::Parent => Inverse::Paired(LifecycleRel::Child),
            LifecycleRel::Child => Inverse::Paired(LifecycleRel::Parent),
            LifecycleRel::Relates => Inverse::Symmetric,
            LifecycleRel::Closes | LifecycleRel::DependsOn | LifecycleRel::Assigns => Inverse::None,
        }
    }

    pub const FORWARD_VOCABULARY: &'static [LifecycleRel] = &[
        LifecycleRel::Closes,
        LifecycleRel::Blocks,
        LifecycleRel::BlockedBy,
        LifecycleRel::DependsOn,
        LifecycleRel::Parent,
        LifecycleRel::Assigns,
        LifecycleRel::Relates,
    ];
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntheticTypedEvent {
    pub source: ArtifactRef,
    pub target: ArtifactRef,
    pub rel: LifecycleRel,
    pub origin_event: String,
    pub origin_actor: String,
    pub zookie: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MirrorError {
    UnknownRel(String),
}

pub fn mirror_edges(tenant: &TenantId, ev: &SyntheticTypedEvent) -> Vec<EdgeRow> {
    let mut rows = vec![lifecycle_edge(tenant, &ev.source, &ev.target, ev.rel, ev)];
    match ev.rel.inverse() {
        Inverse::Paired(inv) => {
            rows.push(lifecycle_edge(tenant, &ev.target, &ev.source, inv, ev));
        }
        Inverse::Symmetric => {
            rows.push(lifecycle_edge(tenant, &ev.target, &ev.source, ev.rel, ev));
        }
        Inverse::None => {}
    }
    rows
}

fn lifecycle_edge(
    tenant: &TenantId,
    source: &ArtifactRef,
    target: &ArtifactRef,
    rel: LifecycleRel,
    ev: &SyntheticTypedEvent,
) -> EdgeRow {
    EdgeRow {
        edge_id: edge_id(tenant, &source.0, &target.0, rel.as_str()),
        source_root: strip_sub(source),
        target_root: strip_sub(target),
        source: source.clone(),
        target: target.clone(),
        rel: rel.as_str().to_string(),
        rel_class: RelClass::Lifecycle,
        origin_event: ev.origin_event.clone(),
        origin_actor: ev.origin_actor.clone(),
        zookie: ev.zookie.clone(),
        tombstoned: false,
    }
}

pub fn project_typed_event(
    proj: &EdgeProjection,
    tenant: &TenantId,
    region: &Region,
    ev: &SyntheticTypedEvent,
) -> Result<Vec<String>, MirrorError> {
    let rows = mirror_edges(tenant, ev);
    let ids: Vec<String> = rows.iter().map(|r| r.edge_id.clone()).collect();
    for row in rows {
        proj.upsert(tenant, region, row);
    }
    Ok(ids)
}

pub fn reconverge(
    proj: &EdgeProjection,
    tenant: &TenantId,
    region: &Region,
    typed_snapshot: &[SyntheticTypedEvent],
    covered_roots: &[ArtifactRef],
    reindex_event_id: &str,
) -> Result<(usize, usize), MirrorError> {
    let mut backed: HashSet<String> = HashSet::new();
    let mut reprojected = 0usize;
    for ev in typed_snapshot {
        let rows = mirror_edges(tenant, ev);
        for row in rows {
            backed.insert(row.edge_id.clone());
            proj.upsert(tenant, region, row);
            reprojected += 1;
        }
    }

    let mut tombstoned = 0usize;
    for root in covered_roots {
        for row in proj.inbound_live(tenant, region, root) {
            if row.rel_class == RelClass::Lifecycle && !backed.contains(&row.edge_id) {
                proj.tombstone(tenant, region, &row.edge_id, reindex_event_id);
                tombstoned += 1;
            }
        }
    }
    Ok((reprojected, tombstoned))
}

#[cfg(test)]
mod tests;
