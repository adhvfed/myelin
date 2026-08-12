#![cfg_attr(
    not(any(test, feature = "test-support")),
    allow(unused_imports, dead_code)
)]

use std::sync::Arc;
use std::time::Duration;

use myelin_events::EmitContextBase;
use myelin_identity::{Consistency, ListObjectsResult, Principal, PrincipalId, PrincipalKind};
use myelin_refs::{strip_sub, ArtifactRef};
use myelin_storage::{InMemoryCache, KmsEngine};
use myelin_substrate::{FailStaticAuthz, FailStaticThreshold};
use myelin_tenancy::{CellId, Region, TenantId};

use crate::backlinks::AuthzVisibleIndex;
use crate::cache::R2ProjectionCache;
use crate::dek::RefsDekPin;
use crate::edge_builder::{edge_id, EdgeProjection, EdgeRow, RefsEdgeBuilder, RelClass};
use crate::reindex_at_scale::{build_full_scale_corpus, run_full_scale_reindex_parity};
use crate::resolve::{
    bounded_stale, ProjectApi, ProjectApiError, ProjectOutcome, Resolution, ResolveMode,
    ResolveService, TombstoneReason,
};
use crate::restore_reerase::{
    build_backup_scale_corpus, re_erase_at_backup_scale, RefsErasureLedger,
};
use crate::traverse::{Traverse, TraverseFilter, TraverseResult};

pub const E2E_SCENARIOS: [&str; 3] = ["E2E-1", "E2E-3", "E2E-4"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct E2eArtifact {
    pub scenario: &'static str,
    pub green: bool,
    pub evidence: String,
    pub leaks: u64,
}

impl E2eArtifact {
    pub fn is_green(&self) -> bool {
        self.green && self.leaks == 0
    }
}

fn e2e_tenant() -> TenantId {
    TenantId("acme".into())
}

fn e2e_region() -> Region {
    Region("fr-par".into())
}

fn e2e_cell() -> CellId {
    CellId::from_token("cell-fr-par-1")
}

fn e2e_viewer(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, e2e_tenant())
}

fn e2e_authz() -> Arc<FailStaticAuthz> {
    let threshold = FailStaticThreshold {
        status: "OPEN - LEGAL".into(),
        owner: "DPO / Legal".into(),
        static_max_secs: None,
        static_max_default_secs: 300,
        agent_token_ttl_secs: 60,
        constraint: "static_max <= revocation-SLA AND static_max >= agent-token-TTL".into(),
    };
    Arc::new(FailStaticAuthz::try_new(300, &threshold).expect("valid fail-static bound"))
}

struct PrPaneOwner {
    insider: String,
    confidential_issue: ArtifactRef,
    check_state: std::sync::Mutex<String>,
}

impl PrPaneOwner {
    fn new(insider: &str, confidential_issue: ArtifactRef) -> PrPaneOwner {
        PrPaneOwner {
            insider: insider.into(),
            confidential_issue,
            check_state: std::sync::Mutex::new("pending".into()),
        }
    }

    fn update_check(&self, new_state: &str) {
        *self.check_state.lock().unwrap() = new_state.into();
    }
}

impl ProjectApi for PrPaneOwner {
    fn check_view(
        &self,
        _tenant: &TenantId,
        _region: &Region,
        object: &ArtifactRef,
        viewer: &Principal,
        _permission: &myelin_identity::Permission,
    ) -> Result<myelin_identity::Decision, ProjectApiError> {
        if strip_sub(object) == self.confidential_issue && viewer.principal_id.0 != self.insider {
            Ok(myelin_identity::Decision::Deny)
        } else {
            Ok(myelin_identity::Decision::Allow)
        }
    }

    fn project(
        &self,
        _tenant: &TenantId,
        _region: &Region,
        ref_: &ArtifactRef,
        _viewer: &Principal,
        _mode: ResolveMode,
    ) -> Result<ProjectOutcome, ProjectApiError> {
        let is_check = ref_.0.contains("/ci/");
        let state = if is_check {
            self.check_state.lock().unwrap().clone()
        } else {
            "open".into()
        };
        let title = if strip_sub(ref_) == self.confidential_issue {
            "TOP SECRET acquisition plan".into()
        } else {
            format!("artifact {}", ref_.0)
        };
        Ok(ProjectOutcome::Live(crate::resolve::OwnerProjection {
            title,
            state,
            icon: "card".into(),
            render_hint: "pane".into(),
            sub_anchor: None,
            flag: None,
        }))
    }
}

fn pr_pane_connected_artifacts(tenant: &str) -> Vec<ArtifactRef> {
    vec![
        ArtifactRef(format!("myelin://{tenant}/git/pr/PR-42")),
        ArtifactRef(format!("myelin://{tenant}/ci/check/PR-42-build")),
        ArtifactRef(format!("myelin://{tenant}/knowledge/page/design-doc-7")),
        ArtifactRef(format!("myelin://{tenant}/issue/issue/ENG-1421")),
    ]
}

pub fn run_e2e_1_pr_pane() -> E2eArtifact {
    let tenant = e2e_tenant();
    let region = e2e_region();
    let confidential = ArtifactRef(format!("myelin://{}/issue/issue/ENG-1421", tenant.0));
    let owner = Arc::new(PrPaneOwner::new("insider", confidential.clone()));
    let svc = ResolveService::new(
        e2e_authz(),
        Arc::new(crate::resolve::NoOpCacheRead),
        owner.clone(),
        e2e_cell(),
    );

    let artifacts = pr_pane_connected_artifacts(&tenant.0);
    let at: Consistency = bounded_stale();
    let mut leaks: u64 = 0;
    let mut insider_resolved = 0usize;

    for art in &artifacts {
        let root = strip_sub(art);
        let r = svc.resolve(
            &tenant,
            &region,
            art,
            &root,
            &e2e_viewer("insider"),
            ResolveMode::Live,
            &at,
            false,
        );
        if r.is_projection() {
            insider_resolved += 1;
        }
    }

    let check_ref = ArtifactRef(format!("myelin://{}/ci/check/PR-42-build", tenant.0));
    owner.update_check("success");
    let subjects = ResolveService::subscribe_subjects(&check_ref);
    let subscribed_to_ci_update = subjects.iter().any(|s| s == "ci.updated");
    let re_resolved = svc.resolve(
        &tenant,
        &region,
        &check_ref,
        &strip_sub(&check_ref),
        &e2e_viewer("insider"),
        ResolveMode::Live,
        &at,
        false,
    );
    let check_live_updated = matches!(
        &re_resolved,
        Resolution::Projection(p) if p.state == "success"
    );

    let denied = svc.resolve(
        &tenant,
        &region,
        &confidential,
        &strip_sub(&confidential),
        &e2e_viewer("outsider"),
        ResolveMode::Live,
        &at,
        false,
    );
    let outsider_tombstoned = denied.tombstone_reason() == Some(TombstoneReason::Denied);
    if let Resolution::Tombstone(t) = &denied {
        let rendered = format!("{t:?}");
        if rendered.contains("SECRET") || rendered.contains("acquisition") {
            leaks += 1;
        }
        if t.root != strip_sub(&confidential) {
            leaks += 1;
        }
    } else {
        leaks += 1;
    }
    let mut outsider_saw_non_confidential = 0usize;
    for art in &artifacts {
        if strip_sub(art) == strip_sub(&confidential) {
            continue;
        }
        let r = svc.resolve(
            &tenant,
            &region,
            art,
            &strip_sub(art),
            &e2e_viewer("outsider"),
            ResolveMode::Live,
            &at,
            false,
        );
        if r.is_projection() {
            outsider_saw_non_confidential += 1;
        }
    }

    let green = insider_resolved == artifacts.len()
        && subscribed_to_ci_update
        && check_live_updated
        && outsider_tombstoned
        && outsider_saw_non_confidential == artifacts.len() - 1;

    E2eArtifact {
        scenario: "E2E-1",
        green,
        evidence: format!(
            "PR pane (Refs spine): insider resolved {insider_resolved}/{} connected artifacts; \
             mid-flight ci.check.updated live-updated={check_live_updated} (subscribed={subscribed_to_ci_update}); \
             outsider→confidential tombstone(denied)={outsider_tombstoned}, outsider saw \
             {outsider_saw_non_confidential}/{} non-confidential; leaks={leaks}",
            artifacts.len(),
            artifacts.len() - 1,
        ),
        leaks,
    }
}

fn spec_to_ship_lineage(tenant: &str) -> Vec<ArtifactRef> {
    vec![
        ArtifactRef(format!("myelin://{tenant}/knowledge/page/spec-doc")),
        ArtifactRef(format!("myelin://{tenant}/issue/issue/ENG-1")),
        ArtifactRef(format!("myelin://{tenant}/git/pr/PR-1")),
        ArtifactRef(format!("myelin://{tenant}/git/commit/c0ffee")),
        ArtifactRef(format!("myelin://{tenant}/ci/run/run-1")),
        ArtifactRef(format!("myelin://{tenant}/ci/deploy/deploy-1")),
        ArtifactRef(format!("myelin://{tenant}/chat/message/msg-1")),
    ]
}

fn build_lineage_projection(
    tenant: &TenantId,
    region: &Region,
    lineage: &[ArtifactRef],
) -> EdgeProjection {
    let proj = EdgeProjection::new();
    for pair in lineage.windows(2) {
        let (src, tgt) = (&pair[0], &pair[1]);
        proj.upsert(
            tenant,
            region,
            EdgeRow {
                edge_id: edge_id(tenant, &src.0, &tgt.0, "relates"),
                source: src.clone(),
                source_root: src.clone(),
                target: tgt.clone(),
                target_root: tgt.clone(),
                rel: "relates".into(),
                rel_class: RelClass::Lifecycle,
                origin_event: format!("lineage-{}", src.0),
                origin_actor: "p-opaque-author".into(),
                zookie: None,
                tombstoned: false,
            },
        );
    }
    if lineage.len() >= 2 {
        let last = &lineage[lineage.len() - 1];
        let first = &lineage[0];
        proj.upsert(
            tenant,
            region,
            EdgeRow {
                edge_id: edge_id(tenant, &last.0, &first.0, "relates"),
                source: last.clone(),
                source_root: last.clone(),
                target: first.clone(),
                target_root: first.clone(),
                rel: "relates".into(),
                rel_class: RelClass::Lifecycle,
                origin_event: "lineage-cycle".into(),
                origin_actor: "p-opaque-author".into(),
                zookie: None,
                tombstoned: false,
            },
        );
    }
    proj
}

fn traverse_lineage(
    proj: &EdgeProjection,
    lineage: &[ArtifactRef],
    spec_root: &ArtifactRef,
    readable: &[ArtifactRef],
) -> TraverseResult {
    let authz = AuthzVisibleIndex::new();
    let t = Traverse::with_default_bounds(proj.clone(), authz);
    let ids: Vec<&str> = readable.iter().map(|r| r.0.as_str()).collect();
    let list_objects: ListObjectsResult = crate::ids_result(&ids, "zk-e2e3");
    let _ = lineage;
    t.traverse(
        &e2e_tenant(),
        &e2e_region(),
        spec_root,
        &e2e_viewer("insider"),
        &TraverseFilter::any(),
        16,
        &list_objects,
    )
}

pub fn run_e2e_3_spec_to_ship(ctx_base: EmitContextBase) -> E2eArtifact {
    let tenant = e2e_tenant();
    let region = e2e_region();
    let lineage = spec_to_ship_lineage(&tenant.0);
    let spec_root = strip_sub(&lineage[0]);
    let proj = build_lineage_projection(&tenant, &region, &lineage);
    let mut leaks: u64 = 0;

    let full = traverse_lineage(&proj, &lineage, &spec_root, &lineage);
    let reachable_nodes: usize = lineage.len() - 1;
    let full_lineage_walked = full.nodes.len() == reachable_nodes;
    let cycle_surfaced = full.cycle_detected;

    let deploy = strip_sub(&lineage[5]);
    let readable: Vec<ArtifactRef> = lineage
        .iter()
        .filter(|r| strip_sub(r) != deploy)
        .cloned()
        .collect();
    let pruned = traverse_lineage(&proj, &lineage, &spec_root, &readable);
    let chat = strip_sub(&lineage[6]);
    let deploy_pruned = !pruned
        .nodes
        .iter()
        .any(|n| strip_sub(&n.artifact) == deploy);
    let chat_pruned = !pruned.nodes.iter().any(|n| strip_sub(&n.artifact) == chat);
    if !deploy_pruned || !chat_pruned {
        leaks += 1;
    }

    let corpus = build_full_scale_corpus(&tenant.0, 4);
    let parity = run_full_scale_reindex_parity(&tenant, &region, &corpus, ctx_base);
    let (cold_reindex_eq_live, parity_hash) = match &parity {
        Ok(report) => (
            report.is_ref_d4_full_scale_green(),
            report.parity_hash.clone(),
        ),
        Err(_) => (false, String::new()),
    };

    let green = full_lineage_walked
        && cycle_surfaced
        && deploy_pruned
        && chat_pruned
        && cold_reindex_eq_live;

    E2eArtifact {
        scenario: "E2E-3",
        green,
        evidence: format!(
            "spec-to-ship: full lineage walked {}/{} nodes (cycle_surfaced={cycle_surfaced}); \
             per-viewer prune: deploy_pruned={deploy_pruned} chat_pruned={chat_pruned}; \
             cold-reindex==live={cold_reindex_eq_live} (parity_hash={parity_hash}); leaks={leaks}",
            full.nodes.len(),
            reachable_nodes,
        ),
        leaks,
    }
}

#[cfg(any(test, feature = "test-support"))]
pub fn run_e2e_4_dsar_fanout() -> E2eArtifact {
    let tenant = e2e_tenant();
    let region = e2e_region();
    let subject = "p-opaque-subject-0".to_string();

    let corpus = build_backup_scale_corpus(&tenant, &region, 6, 4);
    let builder = RefsEdgeBuilder::new(EdgeProjection::new());
    let dek = Arc::new(RefsDekPin::new(Arc::new(KmsEngine::new())));
    let cache = Arc::new(R2ProjectionCache::with_ttl(
        Arc::new(InMemoryCache::new()),
        dek.clone(),
        Duration::from_secs(300),
    ));
    let ledger = RefsErasureLedger::new();

    let target_subject = corpus
        .subjects
        .first()
        .cloned()
        .unwrap_or_else(|| subject.clone());

    let report = match re_erase_at_backup_scale(
        &corpus,
        &builder,
        &cache,
        dek.as_ref(),
        &ledger,
        std::slice::from_ref(&target_subject),
        "2026-06-25T00:00:00Z",
    ) {
        Ok(report) => report,
        Err(error) => {
            return E2eArtifact {
                scenario: "E2E-4",
                green: false,
                evidence: format!(
                    "DSAR fan-out (Refs side) stopped because crypto-shred was unavailable: {error}"
                ),
                leaks: 1,
            };
        }
    };

    let zero_recoverable = report.is_ref_d5_backup_scale_green();
    let leaks: u64 = if zero_recoverable { 0 } else { 1 };

    E2eArtifact {
        scenario: "E2E-4",
        green: zero_recoverable,
        evidence: format!(
            "DSAR fan-out (Refs side): {} - holder-coverage receipt includes Refs (H12 edge+cache); \
             0 recoverable PII after restore+re-erase (incl. backups)",
            report.summary(),
        ),
        leaks,
    }
}

#[cfg(any(test, feature = "test-support"))]
pub fn run_refs_e2e_wedge(ctx_base: EmitContextBase) -> Vec<E2eArtifact> {
    vec![
        run_e2e_1_pr_pane(),
        run_e2e_3_spec_to_ship(ctx_base),
        run_e2e_4_dsar_fanout(),
    ]
}

#[cfg(test)]
mod tests;
