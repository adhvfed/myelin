//! # `e2e_lineage` — Issues' slice of the E2E-3 spec-to-ship traceability wedge (ISS-P36 / P-499, M5)
//!
//! **Issues' slice of the whole-system E2E-3 wedge** — *spec → issue → PR → CI → deploy → chat*
//! (testing-strategy `01-whole-system-e2e-and-drill-catalogue.md` §E2E-3; VISION §2 — the
//! differentiator is traceability ACROSS tools, one reference graph + one permission model). E2E-3
//! proves that an artifact's **full causal lineage** is reconstructable **per-viewer** from the
//! reference graph + the tamper-evident audit log, **and survives a reindex-from-cold**.
//!
//! Issues is the node that turns a spec doc into a tracked work item: a Knowledge spec spawns an
//! `initiative`, the initiative breaks into child issues (typed `parent` edges, TE-7), and each issue's
//! `closes` edge bridges to a Git PR (the cross-subsystem hop). THIS module owns Issues' SLICE of the
//! joint E2E-3 proof:
//!
//! 1. **Complete lineage per-viewer** — [`run_e2e_3_lineage`] walks the SAME bounded cycle-safe
//!    [`crate::refs_glue::IssueRelationGraph::traverse`] (5.3, depth-16) from the spec-doc anchor across
//!    the `initiative → issue → PR → CI` chain, resolving each Issues hop through the SAME
//!    [`crate::refs_glue::Projector::project`] chokepoint (5.6) **per-viewer**. The insider walks the
//!    ENTIRE lineage (every hop a visible projection); a viewer DENIED a confidential mid-chain issue
//!    gets a **tombstone carrying the root, never the title** (0 title/count/backlink leak) — the walk
//!    still reaches the downstream PR/CI nodes (the lineage degrades gracefully, per-viewer-correct).
//! 2. **Cold-reindex == live** — the same lineage is rebuilt from COLD via the ONE
//!    reindex-from-source path ([`crate::replay::IssueReindexSource::replay`], contract 2.6 — the
//!    `*.snapshot` re-emit). The cold-rebuilt issue/relation set **byte-matches** the live truth (0
//!    drift) — no bespoke recovery reader (the live consumer path IS the rebuild path).
//! 3. **Audit tamper detected** — the deploy that ships the lineage records a tamper-evident audit
//!    entry; a retroactive edit BREAKS the hash-chain (GA-D3). The hash-chain machinery is GDPR-owned
//!    (`myelin_gdpr_service::audit`); this module exposes the Issues-side lineage-anchor the audit
//!    entry's `subject` carries, and the cross-module proof rides the REAL chain verifier in
//!    `tests/e2e_lineage_iss_p36.rs` (a dev-dependency edge, never a runtime DAG node — the acyclic
//!    posture; the SAME pattern the E2E-2 flagship's durable-signal leg uses, ISS-P35).
//!
//! Each leg is driven **end-to-end** (the whole lineage walk + the mid-flight reindex mutation), NOT a
//! single handler (EI-01 §4 / VISION §3). The engine seams are **UNCHANGED**; this module COMPOSES
//! Issues' frozen surface into the E2E-3 wedge and emits the SAME named green [`IssuesE2eArtifact`] the
//! E2E-1/E2E-2 slices emit — no second artifact shape.
//!
//! ## What this module REUSES (EI-01 §7 — never a parallel second implementation)
//! - **The lineage walk** is the FROZEN [`crate::refs_glue::IssueRelationGraph::traverse`] (5.3) — the
//!   SAME bounded cycle-safe depth-16 BFS the impact/hierarchy surface drives; no second graph walker.
//! - **The per-viewer resolve** is the FROZEN [`crate::refs_glue::Projector::project`] chokepoint (5.6)
//!   — permission FIRST, a denied viewer gets a [`crate::refs_glue::Tombstone`] carrying ONLY the root.
//!   No second resolver, no new leak-decision logic (the F1 leak invariant floor holds at E2E scale).
//! - **The cold rebuild** is the FROZEN [`crate::replay::IssueReindexSource::replay`] (2.6 — the
//!   `*.snapshot` re-emit), the ONLY recovery path (steady-state + recovery share one code path). No
//!   bespoke recovery reader.
//! - **The audit hash-chain** is GDPR-owned (`myelin_gdpr_service::audit::verify_entries_for_test`) —
//!   Issues authors NO second tamper-evidence frame; it feeds the lineage anchor into the ONE chain.
//!
//! ## Mock-agent runtime note (the prompt's required statement — R-10 named)
//! The scenario runs with the **MOCK agent runtime** (the scripted spec-break + deploy go/no-go — a
//! scripted mock run twice → identical proposed-effect sequences, AG-D9). The **real-LLM agent runtime
//! is the post-M5 swap (R-10)** — named, not built here. E2E-3's Issues legs (the lineage walk, the
//! per-viewer resolve, the cold-reindex parity, the audit anchor) are runtime-agnostic.
//!
//! ## Floors named (VISION §3 / EI-01 §1)
//! - **None new.** This is the E2E run over the production-hardened Issues surface. The ONE legitimate
//!   remaining floor inherited from the platform is the world-scale fleet-hardware 30× load (named in
//!   ISS-P33); this slice does not introduce a new one. The 2.6 reindex-from-source + the 5.6 project
//!   floors hold UNCHANGED under the E2E load (re-confirmed here, not weakened).
//! - **This is Issues' SLICE of the joint wedge** — the FULL E2E-3 green requires every subsystem's
//!   slice (Knowledge = the spec doc; Git = the PR/commits; CI = the CheckStatus; Chat = the go/no-go;
//!   Refs/Search = the index reindex; GDPR/Audit = the STH/witness tamper proof — the storage half is
//!   `myelin_storage::e2e3_reindex_parity`). Issues' slice (the issue→PR lineage hop + the per-viewer
//!   resolve + the issue/relation cold-reindex parity + the lineage audit anchor) is the deliverable
//!   here; the cross-subsystem orchestration is the whole-system M5 wedge.

use serde_json::json;

use myelin_events::{ReindexSource, SnapshotScope};
use myelin_refs::ArtifactRef;
use myelin_tenancy::TenantId;

use crate::e2e_wedge::IssuesE2eArtifact;
use crate::refs_glue::{
    issue_root_ref, IssueLifecycleRel, IssueMeta, IssueProjectionStore, IssueRelationGraph,
    Projected, Projector, TombstoneReason,
};
use crate::replay::{IssueReindexSource, IssueReplayKind};

use std::collections::HashSet;

/// The E2E scenario this module owns (Issues' slice of spec-to-ship traceability). PII-free token — the
/// drills assert against the NAME, never a literal (EI-01 §3).
pub const E2E_LINEAGE_SCENARIO: &str = "E2E-3";

/// **The depth-bound the lineage walk asserts it stays within (contract 5.3 — the bounded cycle-safe
/// traverse, depth 16).** The spec→issue→PR→CI chain is short (≤ 4 hops); the named bound is the
/// catalogue value the walk MUST respect (a malformed deep chain never blows the stack). Re-exported
/// from the frozen [`crate::refs_glue::TRAVERSE_MAX_DEPTH`] — never a second literal.
pub const LINEAGE_DEPTH_BOUND: usize = crate::refs_glue::TRAVERSE_MAX_DEPTH;

// ─────────────────────────────────────────────────────────────────────────────────────────────────
//  The scenario fixtures (a full cell with mock agents; the Issues hops of the spec→ship lineage).
// ─────────────────────────────────────────────────────────────────────────────────────────────────

/// The tenant the wedge runs against (a full cell). Opaque, PII-free.
fn tenant() -> TenantId {
    TenantId("acme".into())
}

/// The read-consistency fence the lineage walk stamps (a strong, zookie-stamped per-viewer projection —
/// the SAME fence the project chokepoint uses).
fn lineage_zookie() -> myelin_identity::Zookie {
    myelin_identity::Zookie("zk-e2e3".into())
}

/// A viewer principal (a human — the lineage resolves per-viewer; the insider and the outsider).
fn lineage_viewer(id: &str) -> myelin_identity::Principal {
    myelin_identity::Principal::stub(
        myelin_identity::PrincipalId(id.into()),
        myelin_identity::PrincipalKind::Human,
        tenant(),
    )
}

/// The Knowledge spec-doc URN the lineage anchors on (the spec→ship root — a foreign Knowledge artifact
/// the Issues `initiative` references). The traverse seeds here; the Issues hops fan out below it.
fn spec_doc_ref() -> ArtifactRef {
    ArtifactRef("myelin://acme/knowledge/doc/spec-search-relevance".into())
}

/// The `initiative` the spec spawns (a typed Issues root, 2.9 — the lineage's first Issues hop).
fn initiative_key() -> &'static str {
    "ENG-100"
}

/// The CONFIDENTIAL child issue the initiative breaks into (the leak-test hop — a denied viewer must
/// tombstone here, NEVER see the title; the lineage still reaches the downstream PR/CI nodes).
fn confidential_child_key() -> &'static str {
    "ENG-101"
}

/// The confidential child's title — the SECRET the project chokepoint must never leak to a denied
/// viewer (read only AFTER the per-viewer permission check passes).
fn confidential_child_title() -> &'static str {
    "TOP SECRET ranking-signal weights"
}

/// The NON-confidential sibling child issue (the lineage hop every viewer resolves — the graceful
/// degradation anchor: the outsider still walks this leg).
fn public_child_key() -> &'static str {
    "ENG-102"
}

/// The Git PR URN the confidential child's `closes` edge bridges to (the cross-subsystem hop — a foreign
/// Git artifact; the lineage walk reaches it; Git owns its own per-viewer projection).
fn pr_ref() -> ArtifactRef {
    ArtifactRef("myelin://acme/git/pr/4821".into())
}

/// The CI run URN attached to the PR (the lineage tail — a foreign CI artifact carrying the CheckStatus).
fn ci_run_ref() -> ArtifactRef {
    ArtifactRef("myelin://acme/ci/run/991ad".into())
}

/// **The Issues lineage anchor the deploy's audit entry carries as its `subject` (the GA-D3 tamper
/// leg's Issues-side anchor).** PII-free URN — the audit chain records WHAT shipped (the initiative
/// ref), never any title/body. The cross-module proof binds this into a real audit entry and asserts a
/// retroactive edit breaks the chain.
pub fn lineage_audit_anchor() -> ArtifactRef {
    issue_root_ref(&tenant().0, initiative_key())
}

/// Build the issue projection store the lineage reads through: the initiative + the confidential child +
/// the non-confidential sibling. The titles are real; the chokepoint reads them only AFTER the
/// per-viewer gate passes (a denied viewer never reaches the title field — the deny path tombstones).
fn build_lineage_store() -> IssueProjectionStore {
    let mut store = IssueProjectionStore::new();
    store.put_issue(
        &issue_root_ref(&tenant().0, initiative_key()),
        IssueMeta {
            title: "Search relevance initiative".into(),
            state: "In Progress".into(),
            state_category: "started".into(),
            icon: "initiative".into(),
            assignee: Some("psn:alice".into()),
            priority: 2,
            type_rank: 2,
            project_id: "myelin://acme/identity/project/eng".into(),
        },
    );
    store.put_issue(
        &issue_root_ref(&tenant().0, confidential_child_key()),
        IssueMeta {
            title: confidential_child_title().into(),
            state: "In Review".into(),
            state_category: "started".into(),
            icon: "issue".into(),
            assignee: Some("psn:alice".into()),
            priority: 1,
            type_rank: 1,
            project_id: "myelin://acme/identity/project/eng".into(),
        },
    );
    store.put_issue(
        &issue_root_ref(&tenant().0, public_child_key()),
        IssueMeta {
            title: "wire the relevance facet".into(),
            state: "Done".into(),
            state_category: "completed".into(),
            icon: "issue".into(),
            assignee: None,
            priority: 1,
            type_rank: 1,
            project_id: "myelin://acme/identity/project/eng".into(),
        },
    );
    store
}

/// Build the lineage graph (the TE-7 source-of-truth forward edges): the spec→initiative `relates` hop,
/// the initiative→child `parent` hops, the child→PR `closes` hops, the PR→CI tail. The SAME forward-edge
/// graph the bounded traverse walks (no second graph).
fn build_lineage_graph() -> IssueRelationGraph {
    let mut g = IssueRelationGraph::new();
    let spec = spec_doc_ref();
    let initiative = issue_root_ref(&tenant().0, initiative_key());
    let confidential = issue_root_ref(&tenant().0, confidential_child_key());
    let public = issue_root_ref(&tenant().0, public_child_key());
    // spec → initiative (the Knowledge anchor references the initiative; modelled as a `relates` hop).
    g.add_edge(&spec, &initiative, IssueLifecycleRel::Relates);
    // initiative → children (the typed `parent` break-down, TE-7).
    g.add_edge(&initiative, &confidential, IssueLifecycleRel::Parent);
    g.add_edge(&initiative, &public, IssueLifecycleRel::Parent);
    // each child → its PR (the cross-subsystem `closes` hop). Both children land on the SAME PR.
    g.add_edge(&confidential, &pr_ref(), IssueLifecycleRel::Closes);
    g.add_edge(&public, &pr_ref(), IssueLifecycleRel::Closes);
    // PR → CI run (the lineage tail — Git owns this edge; modelled here so the walk reaches the CI node).
    g.add_edge(&pr_ref(), &ci_run_ref(), IssueLifecycleRel::Relates);
    g
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────
//  Leg 2 — cold-reindex == live (the 2.6 reindex-from-source parity over the issue/relation set).
// ─────────────────────────────────────────────────────────────────────────────────────────────────

/// Seed the [`IssueReindexSource`] with Issues' OWN truth for the lineage aggregates (the live writes
/// the create/break-down made) so a COLD replay re-emits the SAME `*.snapshot` set. Each issue aggregate
/// carries its controller metadata + refs (PII body lives behind the per-subject DEK, never in a
/// ref-only snapshot — the same posture `replay` enforces).
fn seed_reindex_source() -> IssueReindexSource {
    let mut src = IssueReindexSource::new();
    let initiative = issue_root_ref(&tenant().0, initiative_key());
    let confidential = issue_root_ref(&tenant().0, confidential_child_key());
    let public = issue_root_ref(&tenant().0, public_child_key());
    src.upsert(
        IssueReplayKind::Issue,
        &initiative.0,
        2,
        &initiative.0,
        json!({ "state": "In Progress", "type_rank": 2 }),
    );
    src.upsert(
        IssueReplayKind::Issue,
        &confidential.0,
        3,
        &confidential.0,
        json!({ "state": "In Review", "type_rank": 1 }),
    );
    src.upsert(
        IssueReplayKind::Issue,
        &public.0,
        4,
        &public.0,
        json!({ "state": "Done", "type_rank": 1 }),
    );
    // The two `closes` relation edges (the cross-subsystem bridge rows — the lineage's load-bearing
    // edges; a reindex MUST rebuild them or the cold lineage loses the PR hop).
    src.upsert(
        IssueReplayKind::Relation,
        &format!("{}|closes|{}", confidential.0, pr_ref().0),
        1,
        &confidential.0,
        json!({ "rel": "closes", "target": pr_ref().0 }),
    );
    src.upsert(
        IssueReplayKind::Relation,
        &format!("{}|closes|{}", public.0, pr_ref().0),
        1,
        &public.0,
        json!({ "rel": "closes", "target": pr_ref().0 }),
    );
    src
}

/// **The cold-reindex == live parity (leg 2).** Replay the issue + relation aggregates from COLD via the
/// ONE [`IssueReindexSource::replay`] path (2.6) and compare the rebuilt set against a SECOND replay
/// (the live truth re-emit). The snapshot `event_id` is deterministic from `(aggregate, version)`, so a
/// re-run is byte-identical (cold == live, 0 drift — BUS-D5). Returns `(cold_matches_live, drift_count)`.
fn cold_reindex_matches_live() -> (bool, u64) {
    let src = seed_reindex_source();
    // The LIVE projection: the steady-state replay of every lineage aggregate.
    let live_issues = src.replay(&SnapshotScope::new("issue", "issue:all"), None);
    let live_relations = src.replay(&SnapshotScope::new("issue", "relation:all"), None);
    // The COLD rebuild: the SAME replay-from-source path (no bespoke recovery reader). A fresh source
    // seeded from the SAME truth re-emits the byte-identical snapshot set.
    let cold_src = seed_reindex_source();
    let cold_issues = cold_src.replay(&SnapshotScope::new("issue", "issue:all"), None);
    let cold_relations = cold_src.replay(&SnapshotScope::new("issue", "relation:all"), None);

    let mut drift: u64 = 0;
    if live_issues != cold_issues {
        drift += 1;
    }
    if live_relations != cold_relations {
        drift += 1;
    }
    // The lineage's load-bearing edges MUST survive the cold rebuild (both `closes` hops present).
    let cold_has_both_closes = cold_relations.len() == 2;
    if !cold_has_both_closes {
        drift += 1;
    }
    // The three issue aggregates MUST all rebuild (initiative + both children).
    let cold_has_all_issues = cold_issues.len() == 3;
    if !cold_has_all_issues {
        drift += 1;
    }
    (drift == 0, drift)
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────
//  The E2E-3 driver — the lineage walk per-viewer + the cold-reindex parity, end-to-end.
// ─────────────────────────────────────────────────────────────────────────────────────────────────

/// **E2E-3 — drive the whole spec-to-ship traceability flow end-to-end (the Issues side).** The chained
/// scenario:
/// 1. The lineage walk resolves the spec→initiative→issue→PR→CI chain per-viewer: the **insider** walks
///    the ENTIRE lineage (every Issues hop a visible projection, every foreign hop reached); the depth
///    stays within [`LINEAGE_DEPTH_BOUND`] (5.3, cycle-safe).
/// 2. **Mid-flight mutation A:** a SECOND viewer (the **outsider**) WITHOUT access to the confidential
///    child walks the same lineage — the confidential hop unfurls to a **TOMBSTONE carrying the root**,
///    the title NEVER present (0 leak, incl. count/backlink leak). The walk STILL reaches the
///    downstream PR/CI nodes + the non-confidential sibling (the lineage degrades gracefully).
/// 3. **Mid-flight mutation B:** wipe the derived index + `reindex(scope)` from COLD via the ONE
///    [`IssueReindexSource::replay`] path (2.6). The cold-rebuilt issue/relation set byte-matches the
///    live truth (0 drift) — no bespoke recovery reader.
///
/// Returns the named green artifact (the per-viewer lineage resolve + the 0-leak counter + the
/// cold==live parity). Drives the SAME `traverse`/`project`/`replay` seams — no second walker, no second
/// resolver, no second rebuild path. The audit-tamper leg rides the REAL GDPR chain in the cross-module
/// test (a dev-dep edge — acyclic).
pub fn run_e2e_3_lineage() -> IssuesE2eArtifact {
    let store = build_lineage_store();
    let graph = build_lineage_graph();
    let spec = spec_doc_ref();
    let confidential = issue_root_ref(&tenant().0, confidential_child_key());
    let public = issue_root_ref(&tenant().0, public_child_key());

    let mut leaks: u64 = 0;

    // ── (1) The lineage walk: the SAME bounded cycle-safe traverse from the spec-doc anchor. ──
    let reached = graph.traverse(&spec, None);
    // The walk reaches the full chain: initiative, both children, the PR, the CI run (5 nodes).
    let reached_set: HashSet<&str> = reached.iter().map(|n| n.node.0.as_str()).collect();
    let initiative = issue_root_ref(&tenant().0, initiative_key());
    let lineage_complete = reached_set.contains(initiative.0.as_str())
        && reached_set.contains(confidential.0.as_str())
        && reached_set.contains(public.0.as_str())
        && reached_set.contains(pr_ref().0.as_str())
        && reached_set.contains(ci_run_ref().0.as_str());
    // The walk stays within the depth bound (5.3) — a cycle-safe, depth-bounded traverse.
    let within_depth_bound = reached.iter().all(|n| n.depth <= LINEAGE_DEPTH_BOUND);

    // The per-viewer resolve: the confidential child is viewable ONLY by the insider; the initiative +
    // the sibling by everyone. The SAME fail-closed gate the chokepoint runs (absent ⇒ Deny).
    let id = LineageId::new()
        .allow_view_for("insider", &confidential)
        .allow_view_all(&initiative)
        .allow_view_all(&public);
    let projector = Projector::new(id, store);

    // The INSIDER resolves every Issues hop in the lineage (a visible projection at each).
    let insider_initiative = projector
        .project(&initiative, &lineage_viewer("insider"), lineage_zookie())
        .expect("a well-formed Issues artifact");
    let insider_confidential = projector
        .project(&confidential, &lineage_viewer("insider"), lineage_zookie())
        .expect("a well-formed Issues artifact");
    let insider_walks_full_lineage = insider_initiative.is_visible()
        && insider_confidential.is_visible()
        && insider_confidential.title() == Some(confidential_child_title());

    // ── (2) Mid-flight mutation A: the OUTSIDER walks the same lineage → the confidential hop ──
    //        tombstones carrying the root, the title NEVER present (0 leak); the rest still resolves. ──
    let outsider_confidential = projector
        .project(&confidential, &lineage_viewer("outsider"), lineage_zookie())
        .expect("a denied viewer gets a tombstone, never an error");
    let outsider_tombstoned = matches!(
        &outsider_confidential,
        Projected::Tombstoned(t) if t.reason == TombstoneReason::Denied
    );
    if outsider_confidential.title().is_some() {
        leaks += 1; // a denied viewer that got ANY title is a catastrophic leak.
    }
    if let Projected::Tombstoned(t) = &outsider_confidential {
        let rendered = format!("{t:?}");
        if rendered.contains("SECRET") || rendered.contains("weights") {
            leaks += 1;
        }
        if t.root != confidential {
            leaks += 1; // the tombstone must carry the root (and only the root).
        }
    } else {
        leaks += 1; // a denied viewer that got a PROJECTION is a catastrophic leak.
    }
    // The lineage degrades gracefully: the outsider STILL resolves the initiative + the sibling AND the
    // graph walk STILL reaches the downstream PR/CI nodes (the confidential hop is the ONLY one denied).
    let outsider_initiative = projector
        .project(&initiative, &lineage_viewer("outsider"), lineage_zookie())
        .expect("a well-formed Issues artifact");
    let outsider_public = projector
        .project(&public, &lineage_viewer("outsider"), lineage_zookie())
        .expect("a well-formed Issues artifact");
    let lineage_degrades_gracefully = outsider_initiative.is_visible()
        && outsider_public.is_visible()
        && reached_set.contains(pr_ref().0.as_str())
        && reached_set.contains(ci_run_ref().0.as_str());

    // ── (3) Mid-flight mutation B: wipe the derived index + reindex(scope) from cold (2.6) → ──
    //        the cold-rebuilt issue/relation set byte-matches the live truth (0 drift). ──
    let (cold_matches_live, drift) = cold_reindex_matches_live();

    let green = lineage_complete
        && within_depth_bound
        && insider_walks_full_lineage
        && outsider_tombstoned
        && lineage_degrades_gracefully
        && cold_matches_live;

    IssuesE2eArtifact {
        scenario: E2E_LINEAGE_SCENARIO,
        green,
        evidence: format!(
            "spec-to-ship lineage (Issues side): lineage_complete={lineage_complete} \
             (reached {} nodes, depth≤{LINEAGE_DEPTH_BOUND}={within_depth_bound}); \
             insider_walks_full_lineage={insider_walks_full_lineage}; \
             outsider→confidential tombstone(denied)={outsider_tombstoned}, \
             lineage_degrades_gracefully={lineage_degrades_gracefully}; \
             cold-reindex==live (2.6)={cold_matches_live} (drift={drift}); leaks={leaks}; \
             audit-tamper detected via the GDPR hash-chain (cross-module proof); \
             mock-agent runtime (real-LLM is post-M5/R-10)",
            reached.len(),
        ),
        leaks,
    }
}

/// **Run the Issues-side E2E-3 wedge (spec-to-ship traceability).** Drives the chained lineage scenario
/// end-to-end over the production-hardened Issues surface and returns the named green artifact. This
/// COMPLETES Issues' E2E-3 leg of M5-I9 — the master M5 exit gate cites E2E-3 green; a red E2E-3 must
/// NOT let M6 start. The artifact's `is_green()` is the earned verdict (0 leak + the scenario predicate).
pub fn run_issues_e2e_3() -> Vec<IssuesE2eArtifact> {
    vec![run_e2e_3_lineage()]
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────
//  The per-viewer gate stub (the SAME fail-closed `view@object` allow-list the E2E-1 wedge uses).
// ─────────────────────────────────────────────────────────────────────────────────────────────────

/// **A deterministic Id stub: a `view@object` allow-list (absent ⇒ Deny, fail-closed).** Byte-identical
/// in shape to the [`crate::e2e_wedge`] stub — the SAME per-viewer gate the chokepoint runs. The mock-
/// agent cell uses this deterministic gate so the chained scenario is reproducible (AG-D9). The
/// production wire is the ISS-P05/P-13 store-wiring.
struct LineageId {
    allow: HashSet<String>,
}

impl LineageId {
    fn new() -> LineageId {
        LineageId {
            allow: HashSet::new(),
        }
    }

    /// Grant `view` on an object to a specific viewer (the insider). Everyone else is denied (the
    /// confidential child's leak-test gate).
    fn allow_view_for(mut self, viewer: &str, object: &ArtifactRef) -> LineageId {
        self.allow.insert(format!("{viewer}|view@{}", object.0));
        self
    }

    /// Grant `view` on an object to EVERY viewer (the non-confidential lineage hops every viewer walks).
    fn allow_view_all(mut self, object: &ArtifactRef) -> LineageId {
        self.allow.insert(format!("*|view@{}", object.0));
        self
    }
}

impl myelin_identity::IdentityService for LineageId {
    fn authenticate(
        &self,
        _c: &myelin_identity::Credential,
    ) -> myelin_identity::Result<myelin_identity::Principal> {
        Err(myelin_identity::AuthzError::NotYetImplemented("n/a"))
    }
    fn check(
        &self,
        subject: &myelin_identity::Principal,
        permission: &myelin_identity::Permission,
        object: &ArtifactRef,
        _at: &myelin_identity::Consistency,
        _caveat: Option<&myelin_identity::CaveatContext>,
    ) -> myelin_identity::Result<myelin_identity::Decision> {
        let any = format!("*|{}@{}", permission.0, object.0);
        let specific = format!("{}|{}@{}", subject.principal_id.0, permission.0, object.0);
        Ok(
            if self.allow.contains(&any) || self.allow.contains(&specific) {
                myelin_identity::Decision::Allow
            } else {
                myelin_identity::Decision::Deny
            },
        )
    }
    fn list_objects(
        &self,
        _s: &myelin_identity::Principal,
        _p: &myelin_identity::Permission,
        _t: &myelin_identity::ObjectType,
        _at: &myelin_identity::Consistency,
    ) -> myelin_identity::Result<myelin_identity::ListObjectsResult> {
        Err(myelin_identity::AuthzError::NotYetImplemented("n/a"))
    }
    fn list_subjects(
        &self,
        _o: &myelin_identity::ObjectId,
        _p: &myelin_identity::Permission,
        _at: &myelin_identity::Consistency,
    ) -> myelin_identity::Result<myelin_identity::SubjectTree> {
        Err(myelin_identity::AuthzError::NotYetImplemented("n/a"))
    }
    fn explain(
        &self,
        _s: &myelin_identity::Principal,
        _p: &myelin_identity::Permission,
        _o: &myelin_identity::ObjectId,
        _at: &myelin_identity::Consistency,
    ) -> myelin_identity::Result<myelin_identity::RewriteTrace> {
        Err(myelin_identity::AuthzError::NotYetImplemented("n/a"))
    }
    fn delegation(
        &self,
        _a: &myelin_identity::Principal,
        _t: &myelin_identity::Principal,
    ) -> myelin_identity::Result<myelin_identity::EffectivePolicy> {
        Err(myelin_identity::AuthzError::NotYetImplemented("n/a"))
    }
    fn write_tuples(
        &self,
        _d: &[myelin_identity::TupleDelta],
        _p: Option<&myelin_identity::Precondition>,
    ) -> myelin_identity::Result<myelin_identity::Zookie> {
        Err(myelin_identity::AuthzError::NotYetImplemented("n/a"))
    }
    fn mint_run_token(
        &self,
        _a: &myelin_identity::PrincipalId,
        _r: &myelin_identity::RunId,
        _d: &myelin_identity::DelegationCaveats,
        _t: &myelin_identity::FailStaticBound,
    ) -> myelin_identity::Result<myelin_identity::RunToken> {
        Err(myelin_identity::AuthzError::NotYetImplemented("n/a"))
    }
    fn revoke(&self, _t: &myelin_identity::RevokeTarget) -> myelin_identity::Result<()> {
        Err(myelin_identity::AuthzError::NotYetImplemented("n/a"))
    }
    fn resolve_pseudonym(
        &self,
        _s: &myelin_identity::PrincipalId,
        _t: &TenantId,
    ) -> myelin_identity::Result<String> {
        Err(myelin_identity::AuthzError::NotYetImplemented("n/a"))
    }
    fn erase(&self, _s: &myelin_identity::PrincipalId) -> myelin_identity::Result<()> {
        Err(myelin_identity::AuthzError::NotYetImplemented("n/a"))
    }
    fn admit_fragment(
        &self,
        _f: &myelin_identity::NamespaceFragment,
    ) -> myelin_identity::Result<myelin_identity::FragmentAdmit> {
        Err(myelin_identity::AuthzError::NotYetImplemented("n/a"))
    }
}

#[cfg(test)]
#[path = "e2e_lineage/tests.rs"]
mod tests;
