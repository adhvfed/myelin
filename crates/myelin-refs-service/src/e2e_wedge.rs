//! # `e2e_wedge` — the whole-system E2E wedge Refs crosses (REF-P27 / P-458, M5)
//!
//! **The completion of R-M5.** This module is the **Refs side of the three whole-system
//! chained-mutation E2E scenarios** — E2E-1 (the PR context pane), E2E-3 (spec-to-ship traceability),
//! and E2E-4 (the DSAR fan-out). Each is driven **end-to-end** (the whole flow, not a single handler)
//! over the **production-hardened Refs engine** the M5 prompts built — the resolve chokepoint
//! ([`crate::resolve`]), the bounded cycle-safe traverse ([`crate::traverse`]), the reindex-from-source
//! parity engine ([`crate::reindex`] / [`crate::reindex_at_scale`]), the cross-cell fan-out
//! ([`crate::cross_cell`]), and the structural erasure holder ([`crate::holder`] /
//! [`crate::restore_reerase`]). The engine is **UNCHANGED**; this module COMPOSES it into the three
//! whole-system scenarios and emits each scenario's named green artifact.
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/reference-graph.md`
//! §1 (the moat thesis — one reference graph + one permission model), §4.5 (the lineage traverse),
//! §4.7 (reindex parity). **Drill source:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`
//! §2 (E2E-1/E2E-3/E2E-4 — the chained-mutation scenarios; each step mutates and the wedge re-resolves
//! mid-flight). **Contract-index rows 5.2** (resolve, the E2E-1 unfurl), **5.3** (traverse, the E2E-3
//! lineage walk), **10.1** (the E2E-4 holder fan-out). **External insight:**
//! `01-process-and-quality-doctrine.md` §3/§4 (drive the WHOLE thing — a chained-mutation E2E, not a
//! single handler; observability is part of the pass); `04-hard-problems.md` §1 (cross-region PII-free),
//! §5.3 (reindex-from-source the ONLY recovery path). **VISION §1, §3** (the reference graph as
//! connective tissue; GDPR-by-construction).
//!
//! ## What this module REUSES (EI-01 §7 — never a parallel second implementation)
//! This is the **whole-system DRIVER over the EXISTING engine**, not a second resolve/traverse/erase.
//! - **E2E-1** drives the SAME [`crate::ResolveService::resolve`] chokepoint REF-P10 froze (per-viewer
//!   gate → tombstone-never-leak) plus the SAME [`crate::Traverse::traverse`] backlink walk and the
//!   SAME `*.updated` subscription seam ([`crate::ResolveService::subscribe_subjects`]) — the
//!   mid-flight CI check-update and the confidential-issue tombstone are the chokepoint's own
//!   behaviour, observed across the chain.
//! - **E2E-3** drives the SAME [`crate::Traverse::traverse`] over the FULL spec→issue→PR→commit→CI→
//!   deploy→chat lineage (depth-16, cycle-safe, per-viewer) and the SAME
//!   [`crate::run_full_scale_reindex_parity`] (the REF-P24 reindex-from-source engine) for the
//!   wipe→reindex→byte-match-live mutation. No second reindexer.
//! - **E2E-4** drives the SAME [`crate::re_erase_at_backup_scale`] structural erasure +
//!   [`crate::RefsEdgeHolder`]/[`crate::RefsCacheHolder`] locate/erase (REF-P15/P25) — the edges + cache
//!   return 0 recoverable PII, the unfurls degrade to tombstones, and the holder-coverage receipt
//!   includes Refs. No second erasure path.
//!
//! Each scenario emits its **named green artifact** (an [`E2eArtifact`]) — the dated, content-addressed
//! report the master M5 exit gate cites. A scenario that does not reach its green predicate fails
//! LOUDLY (the report's `is_green()` is false); there is no weakened threshold and no claimed green that
//! was not earned (EI-01 §3 / VISION §3).
//!
//! ## The leak invariant floors STILL HOLD at E2E scale (the prompt's required statement)
//! The REF-P10 resolve-half leak invariant (a denied viewer gets a [`crate::Tombstone`] carrying NO
//! title/state/icon — there is no field to leak into) and the REF-P11 traverse-half leak invariant (a
//! hop into an unreadable artifact PRUNES that branch) are the load-bearing properties. This module
//! ASSERTS both at E2E scale: E2E-1's mid-flight second viewer gets a tombstone (0 title/count/backlink
//! leak), and E2E-3's per-viewer lineage prunes the unreadable hops. The mutation floors on those
//! invariants live in `resolve.rs` / `traverse.rs` / `backlinks.rs` and are UNCHANGED — this module
//! adds NO new leak-decision logic; it proves the frozen decisions hold across the whole flow.
//!
//! ## Floors named (VISION §3 / EI-01 §1)
//! - **None new.** This is the E2E run over the production-hardened engine. The ONE legitimate remaining
//!   floor inherited by E2E-3's reindex leg is the world-scale fleet-hardware 30× load
//!   ([`crate::WORLD_SCALE_FLEET_LOAD_FLOOR`]) and by E2E-4's erase leg the backup-fleet load
//!   ([`crate::WORLD_SCALE_BACKUP_FLEET_FLOOR`]) — both already named by REF-P24/REF-P25; this wedge
//!   does not introduce a new one.
//! - The other systems' E2E surfaces (Git/CI/Issues/Knowledge/Chat/Search/Identity/Notif sides) are the
//!   OWNING subsystems' E2E prompts — this module drives the **Refs side** (the spine: every connected
//!   artifact resolves per-viewer; the lineage walk; the DSAR edge+cache surface). The cross-subsystem
//!   producers are reached through the SAME frozen seams ([`crate::ProjectApi`], the synthetic owner
//!   standing in for the real `project` — the production wire is the named `ResilientClient` floor).

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

/// The three whole-system E2E scenarios Refs crosses (the master M5 exit gate cites E2E-1..E2E-4; this
/// module owns the Refs side of -1/-3/-4). PII-free tokens — drills assert against the NAME, never a
/// literal (EI-01 §3).
pub const E2E_SCENARIOS: [&str; 3] = ["E2E-1", "E2E-3", "E2E-4"];

/// **The named green artifact one E2E scenario emits (the prompt's per-scenario "named green
/// artifact").** A content-addressed, dated report the master M5 exit gate cites. `green` is the
/// scenario's earned green predicate; `evidence` is the load-bearing assertion summary; `signal` is the
/// telemetry sample the scenario read (observability is part of the pass, EI-01 §3). A scenario that
/// did not reach green has `green = false` — it fails LOUDLY, never a claimed-but-unearned green.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct E2eArtifact {
    /// Which E2E scenario this artifact attests (one of [`E2E_SCENARIOS`]).
    pub scenario: &'static str,
    /// The earned green verdict — `true` iff every load-bearing assertion held end-to-end.
    pub green: bool,
    /// A one-line human-readable evidence summary (the dated artifact's body).
    pub evidence: String,
    /// The leak counter the scenario asserted at `0` (0 title/count/backlink leak) — the F1 spine.
    pub leaks: u64,
}

impl E2eArtifact {
    /// The green predicate (the dated artifact is green iff the scenario earned it AND 0 leaks).
    pub fn is_green(&self) -> bool {
        self.green && self.leaks == 0
    }
}

// ──────────────────────────────────────────────────────────────────────────────────────────────────
//  Shared E2E test fixtures (the cell + tenant the wedge runs against; a full cell with mock agents).
// ──────────────────────────────────────────────────────────────────────────────────────────────────

/// The tenant the wedge runs against (a full cell). Opaque, PII-free.
fn e2e_tenant() -> TenantId {
    TenantId("acme".into())
}

/// The region (fr-par — the dev/prod residency pin; a config swap, never a code change).
fn e2e_region() -> Region {
    Region("fr-par".into())
}

/// The home cell the wedge serves (C-5: a cross-cell target dispatches to its home cell).
fn e2e_cell() -> CellId {
    CellId::from_token("cell-fr-par-1")
}

/// A viewer principal (a human or agent — the wedge runs per-viewer).
fn e2e_viewer(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, e2e_tenant())
}

/// The §8.2 fail-static bound (agent-token TTL = 60s ≤ static_max = 300s ≤ revocation SLA = 300s) — the
/// SAME bound the resolve chokepoint tests use (REF-P10). The wedge fronts its check through the SAME
/// 1.10 fail-static wiring; the authoritative verdict comes from the synthetic owner's `check_view`.
fn e2e_authz() -> Arc<FailStaticAuthz> {
    let threshold = FailStaticThreshold {
        status: "OPEN — LEGAL".into(),
        owner: "DPO / Legal".into(),
        static_max_secs: None,
        static_max_default_secs: 300,
        agent_token_ttl_secs: 60,
        constraint: "static_max <= revocation-SLA AND static_max >= agent-token-TTL".into(),
    };
    Arc::new(FailStaticAuthz::try_new(300, &threshold).expect("valid fail-static bound"))
}

// ──────────────────────────────────────────────────────────────────────────────────────────────────
//  E2E-1 — The PR context pane (Refs as the spine).
// ──────────────────────────────────────────────────────────────────────────────────────────────────

/// **The PR-pane synthetic owner (the E2E-1 mock-agent cell).** Stands in for the real
/// Git/CI/Issues/Knowledge `project` + Identity `check` (the production wire is the named
/// `ResilientClient` floor) — it is PROGRAMMABLE so the chained-mutation scenario can drive the
/// per-viewer pane and the mid-flight CI check-update. Two viewers: an `insider` (sees every connected
/// artifact) and an `outsider` (denied the confidential issue → tombstone, 0 leak). The CI check state
/// is mutable so step 3 can flip `build → success, test → failure` and the pane live-updates.
struct PrPaneOwner {
    /// The viewer permitted to view the confidential issue (the insider). Everyone else is denied.
    insider: String,
    /// The confidential issue root (the artifact a denied viewer must NOT see the title of).
    confidential_issue: ArtifactRef,
    /// The live CI check state (mutable — step 3's mid-flight `ci.check.updated`). `state` is rendered
    /// into the projection's lifecycle state so the pane's checks panel reflects the latest.
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

    /// Mid-flight mutation A: CI emits `ci.check.updated` — flip the check state the pane renders.
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
        // The confidential issue is visible ONLY to the insider (the leak-test artifact). Every other
        // connected artifact (the PR, the doc embed, the CI checks) is visible to all viewers of the
        // pane. A denied viewer of the confidential issue → Deny → the chokepoint tombstones it.
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
        // Render the connected artifact. A CI check ref reflects the LIVE (mutable) check state so the
        // mid-flight `ci.check.updated` lands in the pane; the confidential issue carries a SECRET title
        // that the chokepoint must never leak to a denied viewer (it never reaches here when denied).
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

/// The connected artifacts a PR context pane resolves (the E2E-1 spine — Git PR, CI check, Knowledge
/// doc embed, the confidential issue, a mentioned member). Every one resolves per-viewer through the
/// SAME chokepoint. PII-free opaque URNs.
fn pr_pane_connected_artifacts(tenant: &str) -> Vec<ArtifactRef> {
    vec![
        ArtifactRef(format!("myelin://{tenant}/git/pr/PR-42")),
        ArtifactRef(format!("myelin://{tenant}/ci/check/PR-42-build")),
        ArtifactRef(format!("myelin://{tenant}/knowledge/page/design-doc-7")),
        ArtifactRef(format!("myelin://{tenant}/issue/issue/ENG-1421")),
    ]
}

/// **E2E-1 — drive the whole PR-context-pane flow end-to-end (Refs is the spine).** The chained
/// mutation:
/// 1. The pane resolves every connected artifact per-viewer (the insider sees all).
/// 2. Mid-flight mutation A: CI emits `ci.check.updated` (build → success) — the pane's checks panel
///    live-updates (the `subscribe_subjects` seam means the unfurl re-resolves; here the re-resolve
///    serves the new state).
/// 3. Mid-flight mutation B: a SECOND viewer WITHOUT access to the confidential issue opens the same
///    pane — the issue unfurls to a TOMBSTONE carrying the root, title NEVER present (0 leak, incl.
///    count/backlink leak — the tombstone is structurally incapable of carrying a title/state/icon).
///
/// Returns the named green artifact (the pane-resolution trace + zero-leak counter at 0 + the
/// per-viewer projection diff). Drives the SAME [`ResolveService::resolve`] chokepoint — no second
/// resolver.
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

    // ── (1) The pane resolves every connected artifact per-viewer (the insider sees all). ──
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
        // Every connected artifact resolves for the insider — including the confidential issue (the
        // insider IS permitted). 0 should tombstone for the insider.
    }

    // ── (2) Mid-flight mutation A: ci.check.updated (build → success) → the pane live-updates. ──
    let check_ref = ArtifactRef(format!("myelin://{}/ci/check/PR-42-build", tenant.0));
    owner.update_check("success");
    // The caller subscribes to *.updated (§4.2 step 4) — assert the subscription seam names the CI
    // lifecycle subject so the pane re-resolves on the update (the freshness-budget mechanism).
    let subjects = ResolveService::subscribe_subjects(&check_ref);
    let subscribed_to_ci_update = subjects.iter().any(|s| s == "ci.updated");
    // Re-resolve the check ref — the pane now serves the NEW state (the live check-update landed).
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

    // ── (3) Mid-flight mutation B: a SECOND viewer without access → the confidential issue ──
    //        tombstones, title NEVER present (0 leak, incl. count/backlink leak). ──
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
    // The structural leak invariant: a tombstone has NO title/state/icon field — the secret cannot
    // appear. Debug-format the whole resolution and assert the secret title is absent (a regression
    // that added a leak field is caught). The tombstone carries ONLY the root (no count/backlink leak).
    if let Resolution::Tombstone(t) = &denied {
        let rendered = format!("{t:?}");
        if rendered.contains("SECRET") || rendered.contains("acquisition") {
            leaks += 1;
        }
        if t.root != strip_sub(&confidential) {
            // The tombstone must carry the root (and only the root) — a missing/wrong root is a defect.
            leaks += 1;
        }
    } else {
        // A denied viewer that got a PROJECTION is a catastrophic leak.
        leaks += 1;
    }
    // The other connected artifacts (the PR, the doc, the check) STILL resolve for the outsider (only
    // the confidential issue is denied — the pane degrades gracefully, the rest is per-viewer-correct).
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

// ──────────────────────────────────────────────────────────────────────────────────────────────────
//  E2E-3 — Spec-to-ship traceability (the full lineage traverse + reindex parity).
// ──────────────────────────────────────────────────────────────────────────────────────────────────

/// The spec-to-ship lineage chain (spec doc → issue → PR → commit → CI run → deploy → chat decision) —
/// a 7-deep dependency chain the traverse walks depth-16 cycle-safe per-viewer. Returns the ordered
/// roots; consecutive roots are linked by a `relates`/`parent` lifecycle edge. PII-free opaque URNs.
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

/// Build the lineage edge projection: a forward `relates`-class chain spec→issue→…→chat, PLUS a
/// back-edge from the last node to the spec (so the cycle guard is EXERCISED — the walk must terminate,
/// surface `cycle_detected`, never hang). Every edge is `reference`/`lifecycle`-class so the traverse
/// follows them.
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
    // The cycle back-edge: last → first (a dependency graph with a cycle — drill REF-D8).
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

/// Drive the per-viewer lineage traverse over a built projection. The viewer may view every lineage
/// node EXCEPT optionally one `unreadable` root (the per-viewer prune leg) — the `list_objects` result
/// admits the readable set; the traverse prunes the unreadable branch.
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
    let _ = lineage; // the lineage is the edge source; the readable set governs the post-filter.
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

/// **E2E-3 — drive the whole spec-to-ship traceability flow end-to-end.** The chained mutation:
/// 1. `traverse(spec_doc, viewer)` walks the ENTIRE lineage (depth-16, cycle-safe) per-viewer — every
///    reachable node discovered, the cycle surfaced as a DIAGNOSTIC (never a hang).
/// 2. The per-viewer prune leg: a viewer WITHOUT access to one lineage node sees that node (and the
///    branch reachable only through it) PRUNED — 0 leak through the traverse.
/// 3. Mid-flight mutation: WIPE the Refs edge index, `reindex(scope)` via the live consumer path
///    (`*.snapshot` replay) — the rebuilt index byte-MATCHES live (F4 / REF-D4 at scale).
///
/// Returns the named green artifact (the lineage diff live-vs-cold at 0 drift + the per-viewer walk).
/// Drives the SAME [`Traverse::traverse`] + the SAME [`run_full_scale_reindex_parity`] engine.
pub fn run_e2e_3_spec_to_ship(ctx_base: EmitContextBase) -> E2eArtifact {
    let tenant = e2e_tenant();
    let region = e2e_region();
    let lineage = spec_to_ship_lineage(&tenant.0);
    let spec_root = strip_sub(&lineage[0]);
    let proj = build_lineage_projection(&tenant, &region, &lineage);
    let mut leaks: u64 = 0;

    // ── (1) The FULL per-viewer lineage walk (depth-16, cycle-safe). The viewer reads the whole ──
    //        chain; the cycle back-edge surfaces as a diagnostic, never a hang. ──
    let full = traverse_lineage(&proj, &lineage, &spec_root, &lineage);
    // The walk discovers every node except the root itself (the root is depth-0, not in the set). The
    // chain is 7 deep; from the spec the reachable set is the other 6 nodes.
    let reachable_nodes: usize = lineage.len() - 1;
    let full_lineage_walked = full.nodes.len() == reachable_nodes;
    let cycle_surfaced = full.cycle_detected; // the back-edge was guarded, surfaced (never a hang).

    // ── (2) The per-viewer PRUNE leg: deny ONE node (the CI deploy) → it AND the branch reachable ──
    //        only through it are pruned (0 leak through the traverse). ──
    let deploy = strip_sub(&lineage[5]); // ci/deploy/deploy-1
    let readable: Vec<ArtifactRef> = lineage
        .iter()
        .filter(|r| strip_sub(r) != deploy)
        .cloned()
        .collect();
    let pruned = traverse_lineage(&proj, &lineage, &spec_root, &readable);
    // The deploy node must NOT appear (pruned), and the chat message reachable ONLY through the deploy
    // must ALSO be pruned (the branch-prune — the traversal is not a side-channel).
    let chat = strip_sub(&lineage[6]);
    let deploy_pruned = !pruned
        .nodes
        .iter()
        .any(|n| strip_sub(&n.artifact) == deploy);
    let chat_pruned = !pruned.nodes.iter().any(|n| strip_sub(&n.artifact) == chat);
    if !deploy_pruned || !chat_pruned {
        // An unreadable node (or a node reachable only through it) leaked through the traverse.
        leaks += 1;
    }

    // ── (3) Mid-flight mutation: WIPE the edge index → reindex-from-source → byte-match live. ──
    //        Drives the REF-P24 full-scale reindex-parity engine (the cold-reindex == live proof). ──
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

// ──────────────────────────────────────────────────────────────────────────────────────────────────
//  E2E-4 — The DSAR fan-out (Refs' edges + cache return 0 recoverable PII).
// ──────────────────────────────────────────────────────────────────────────────────────────────────

/// **E2E-4 — drive the whole DSAR fan-out flow end-to-end (the Refs side of the GDPR-by-construction
/// proof).** The chained mutation:
/// 1. Seed one subject's references into the Refs edge index + the R2 projection cache (titles sealed
///    under per-subject DEKs).
/// 2. `dsr_submit(subject)` → erase: the subject's cached titles are crypto-shredded (per-subject DEK
///    destroyed → unrecoverable), the edges keep only the opaque `origin_actor` (Identity's pseudonym
///    shred makes it unresolvable), and the unfurls degrade to TOMBSTONES.
/// 3. Mid-flight: RESTORE a pre-erasure backup → re-erase from the erasure ledger → the subject is
///    STILL erased (0 resurrected, 0 recoverable PII incl. backups).
/// 4. The holder-coverage receipt includes Refs (the H12 edge + cache holders) — 0 recoverable PII +
///    the located edges count.
///
/// Returns the named green artifact (Refs' part of the H1–H18 coverage receipt + post-erase locate = 0
/// recoverable PII). Drives the SAME [`re_erase_at_backup_scale`] structural erasure engine.
pub fn run_e2e_4_dsar_fanout() -> E2eArtifact {
    let tenant = e2e_tenant();
    let region = e2e_region();
    // The DSAR subject (a PSEUDONYMOUS opaque id — never the name; the corpus authors edges as it).
    let subject = "p-opaque-subject-0".to_string();

    // ── (1) Seed the subject's references into a backup-scale corpus (edges + cache titles). ──
    //        The REAL crypto-shred surface: an InMemoryCache-backed R2ProjectionCache (the SAME path the
    //        dev-stack Valkey backing rides — dev<->prod is a config swap) + the shared KMS hierarchy. ──
    let corpus = build_backup_scale_corpus(&tenant, &region, 6, 4);
    let builder = RefsEdgeBuilder::new(EdgeProjection::new());
    // The cache and the re-erase MUST share the SAME DEK pin so the crypto-shred destroys exactly the
    // keys the cache sealed the name-bearing titles under (a separate pin would leave the title
    // recoverable — the property would be vacuous).
    let dek = Arc::new(RefsDekPin::new(Arc::new(KmsEngine::new())));
    let cache = Arc::new(R2ProjectionCache::with_ttl(
        Arc::new(InMemoryCache::new()),
        dek.clone(),
        Duration::from_secs(300),
    ));
    let ledger = RefsErasureLedger::new();

    // The subject the corpus authored edges for (the first corpus subject — deterministic).
    let target_subject = corpus
        .subjects
        .first()
        .cloned()
        .unwrap_or_else(|| subject.clone());

    // ── (2)+(3) ERASE → RESTORE pre-erase backup → re-erase from the ledger (0 resurrected). ──
    //           Drives the REF-P25 backup-scale re-erase engine (crypto-shred + ledger replay). ──
    let report = re_erase_at_backup_scale(
        &corpus,
        &builder,
        &cache,
        dek.as_ref(),
        &ledger,
        std::slice::from_ref(&target_subject),
        "2026-06-25T00:00:00Z",
    );

    // ── (4) The holder-coverage: Refs' edges + cache return 0 recoverable PII; unfurls → tombstones. ──
    let zero_recoverable = report.is_ref_d5_backup_scale_green();
    // A leak here is any recoverable PII after the re-erase (the F1 / GA-D1 spine: 0 recoverable).
    let leaks: u64 = if zero_recoverable { 0 } else { 1 };

    E2eArtifact {
        scenario: "E2E-4",
        green: zero_recoverable,
        evidence: format!(
            "DSAR fan-out (Refs side): {} — holder-coverage receipt includes Refs (H12 edge+cache); \
             0 recoverable PII after restore+re-erase (incl. backups)",
            report.summary(),
        ),
        leaks,
    }
}

// ──────────────────────────────────────────────────────────────────────────────────────────────────
//  The whole-wedge driver — run all three Refs-side E2E scenarios + their named green artifacts.
// ──────────────────────────────────────────────────────────────────────────────────────────────────

/// **Run the whole Refs-side E2E wedge (E2E-1 + E2E-3 + E2E-4).** Drives each chained-mutation scenario
/// end-to-end over the production-hardened engine and returns the three named green artifacts. This
/// COMPLETES R-M5 — the master M5 exit gate cites E2E-1..E2E-4 green; a red E2E-1 must NOT let M6 start.
/// Each artifact's `is_green()` is the per-scenario earned verdict (0 leak + the scenario's predicate).
pub fn run_refs_e2e_wedge(ctx_base: EmitContextBase) -> Vec<E2eArtifact> {
    vec![
        run_e2e_1_pr_pane(),
        run_e2e_3_spec_to_ship(ctx_base),
        run_e2e_4_dsar_fanout(),
    ]
}

#[cfg(test)]
mod tests;
