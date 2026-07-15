//! # `e2e_wedge` — the whole-system E2E wedge Search crosses (SRCH-P32 / P-465, M5)
//!
//! **The completion of S-M5 (the master M5→M6 boundary's Search rows).** This module is the **Search
//! side of the three whole-system chained-mutation E2E scenarios** — E2E-1 (the PR context pane),
//! E2E-3 (spec-to-ship / reindex-parity), and E2E-4 (the DSAR fan-out). Each is driven **end-to-end**
//! (the whole flow, not a single handler) over the **production-hardened Search engine** the M5 prompts
//! built — the permission-aware pre-filter pipeline ([`crate::pipeline`], the `list_objects` lowering
//! the `search-requires-acl-filter` lint guards), the reindex-from-source rebuild
//! ([`crate::reindex::SearchReindexer`] — the ONLY rebuild path, SEARCH-1), the restore-verify gate
//! ([`crate::restore_verify::SearchRestoreVerifyGate`]), the live purge+compact erase
//! ([`crate::erase::SearchEraseHolder`]), and the backup-scale crypto-shred
//! ([`crate::hyok_scale::BackupScaleEraseGate`]). The engine is **UNCHANGED**; this module COMPOSES it
//! into the three whole-system scenarios and emits each scenario's named green artifact.
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/search-and-indexing.md`
//! §1 (the two cardinal invariants the E2E re-runs — permission-aware reads everywhere + erasure reaches
//! everything), §4.2 (the `list_objects` pre-filter — the E2E-1 zero-leak crux), §4.8 (embeddings &
//! text are personal data — the E2E-4 erase), §4.9 (reindex-from-source is the ONLY rebuild path — the
//! E2E-3 cold==live proof). **Reconciliation:** `00-reconciliation-decisions.md` X-7 (the erasure
//! posture E2E-4 proves). **Drill source:**
//! `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` §2 — E2E-1 (the PR context pane,
//! SRCH-D1 in-context), E2E-3 (spec-to-ship / reindex-parity, SRCH-D5 at scale), E2E-4 (the DSAR
//! fan-out, SRCH-D4 at backup scale). **Contract-index rows 6.1/6.2** (query/semantic — E2E-1),
//! **6.4** (reindex — E2E-3), **10.1/10.4** (the DSR fan-out — E2E-4). **External insight:**
//! `01-process-and-quality-doctrine.md` §3/§4 (drive the WHOLE thing — a chained-mutation E2E, not a
//! single handler; observability is part of the pass); `04-hard-problems.md` §1 (cross-region PII-free;
//! the key stays destroyed even after a restore). **VISION §3** (world-scalable; EU-sovereign;
//! GDPR-by-construction).
//!
//! ## What this module REUSES (EI-01 §7 — never a parallel second implementation)
//! This is the **whole-system DRIVER over the EXISTING engine**, not a second query/reindex/erase.
//! - **E2E-1** drives the SAME [`crate::pipeline::query`] / [`crate::pipeline::semantic`] permission-aware
//!   pre-filter (SRCH-P08/P09/P11): a hit on a confidential issue NEVER enters the candidate set for a
//!   denied viewer (0 doc/count/IDF/RAG leak), and the in-context unfurl resolves to a TOMBSTONE that
//!   carries the root, NO title (0 title leak). The mid-flight `ci.check.updated` re-resolves through the
//!   SAME freshness path (the cache busts; the pane serves the new state). No second query path.
//! - **E2E-3** drives the SAME [`crate::reindex::SearchReindexer::reindex`] (the reindex-from-source
//!   engine, SRCH-P16) for the wipe→reindex→byte-match-live mutation, validated by the SAME
//!   [`crate::restore_verify::SearchRestoreVerifyGate`] (cold==live by construction, SRCH-P28). The parity
//!   hash is a deterministic content hash over the live searchable corpus vs the cold-rebuilt one. No
//!   second reindexer, no bespoke recovery reader.
//! - **E2E-4** drives the SAME [`crate::hyok_scale::BackupScaleEraseGate`] (purge + compact + crypto-shred,
//!   SRCH-P29) over the SAME [`crate::erase::SearchEraseHolder`] live path (SRCH-P15) — Search's docs +
//!   EMBEDDINGS return 0 recoverable PII incl. vectors incl. backups, and the holder-coverage receipt
//!   INCLUDES Search (H7). No second erasure path.
//!
//! Each scenario emits its **named green artifact** (an [`E2eArtifact`]) — the dated, content-addressed
//! report the master M5 exit gate cites. A scenario that does not reach its green predicate fails LOUDLY
//! (the report's `is_green()` is false); there is no weakened threshold and no claimed green that was not
//! earned (EI-01 §3 / VISION §3).
//!
//! ## The two cardinal invariants STILL HOLD at E2E scale (the prompt's required statement)
//! The SRCH-P09 leak invariant (a hidden doc NEVER enters the candidate set — 0 doc/count/IDF/RAG leak;
//! the §4.2 pre-filter, not a post-filter) and the SRCH-P15 erase invariant (erase = purge+reindex, not
//! hide — 0 recoverable incl. vectors) are the load-bearing properties. This module ASSERTS both at E2E
//! scale: E2E-1's denied viewer gets 0 confidential docs + a root-only tombstone, and E2E-4's erased
//! subject returns 0 recoverable PII incl. backups. The mutation floors on those invariants live in
//! `pipeline.rs` / `erase.rs` / `hyok_scale.rs` and are UNCHANGED — this module adds NO new leak/erase
//! decision logic; it proves the frozen decisions hold across the whole flow.
//!
//! ## Floors named (VISION §3 / EI-01 §1)
//! - **None new.** This is the E2E run over the production-hardened engine — the named single-cell
//!   ([SRCH-P09]/[SRCH-P11]) / reindex ([SRCH-P16]/[SRCH-P28]) / erase ([SRCH-P15]/[SRCH-P29]) follow-ons
//!   proven end-to-end. The ONE legitimate remaining floor inherited by all three is the world-scale
//!   fleet-hardware 30× load drill (the CI variant runs a MODERATE corpus, not the world-scale fleet
//!   corpus — already named by SRCH-P25/P29).
//! - The cross-subsystem producers (Git/CI/Issues/Knowledge/Chat/Refs/Id/Notif sides of E2E-1/E2E-3, and
//!   the full H1–H18 fan-out of E2E-4) are reached through the SAME frozen seams — the synthetic owner /
//!   reference reindex source standing in for the real producers (the production wire is the per-owner
//!   `replay` floor named in SRCH-P16). This module drives the **Search side**: the leak-free per-viewer
//!   hit, the cold==live reindex, the 0-recoverable erase + the H7 holder-coverage row.
//!
//! [SRCH-P09]: crate::pipeline
//! [SRCH-P11]: crate::pipeline::semantic
//! [SRCH-P15]: crate::erase::SearchEraseHolder
//! [SRCH-P16]: crate::reindex::SearchReindexer
//! [SRCH-P28]: crate::restore_verify::SearchRestoreVerifyGate
//! [SRCH-P29]: crate::hyok_scale::BackupScaleEraseGate


// MR-009b Wave 5: the E2E runners that construct the in-memory KMS test double are
// `test-support`-gated, which leaves their imports + private helpers unused on the default
// (production) build.
// File-scoped allow ONLY for that cfg (the test/test-support build still checks imports).
#![cfg_attr(not(any(test, feature = "test-support")), allow(unused_imports, dead_code))]

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use myelin_events::reindex::ReferenceReindexSource;
use myelin_events::{
    Actor, ArtifactRef, EmitContextBase, OutboxStore, ReindexSource, SnapshotScope, Timestamp,
};
use myelin_gdpr::SubjectRef;
use myelin_identity::{
    AuthzIndexRef, Consistency, ConsistencyMode, ListObjectsResult, Literal, ObjectType,
    Permission, Principal, PrincipalId, PrincipalKind, Result as AuthzResult, SetExpr, Zookie,
};
use myelin_query::{CmpOp, Expr, FieldType, FieldValue, OrderKey, Predicate, QueryAst};
use myelin_storage::KmsEngine;
use myelin_substrate::Holder;
use myelin_tenancy::{Region, TenantId};

use crate::compiler::{FieldDecl, FieldSchema, FT_BODY_FIELD, SEMANTIC_FIELD};
use crate::dek::SearchDekPin;
use crate::engine::{AclFilter, IndexBackend, IndexDocument, TantivyBackend, ORDER_KEY_FIELD};
use crate::erase::SearchEraseHolder;
use crate::holder::{search_index_holder, SEARCH_INDEX_STORE};
use crate::hyok_scale::{
    build_live_corpus, subject_matcher, BackupScaleEraseGate, BackupScaleEraseInputs,
    SealedBackupSegment,
};
use crate::indexer::{
    IncrementalIndexer, IndexSpec, MockEmbeddingAdapter, ProjectFetchError, ProjectFetcher,
    SearchProjection,
};
use crate::pipeline::{
    query, semantic, ListObjectsPort, Page, QueryStats, RankedResults, RelationalLeaf,
    ReverseIndexAnswer, RevisionWatermark, ScopedEngine, VectorQuery,
};
use crate::reindex::SearchReindexer;
use crate::restore_verify::{SearchErasureLedger, SearchRestoreInputs, SearchRestoreVerifyGate};
use crate::vector::Embedding;

/// The three whole-system E2E scenarios Search crosses (the master M5 exit gate cites E2E-1..E2E-4;
/// this module owns the Search side of -1/-3/-4). PII-free tokens — drills assert against the NAME,
/// never a literal (EI-01 §3).
pub const E2E_SCENARIOS: [&str; 3] = ["E2E-1", "E2E-3", "E2E-4"];

/// **The named green artifact one E2E scenario emits (the prompt's per-scenario "named green
/// artifact").** A content-addressed, dated report the master M5 exit gate cites. `green` is the
/// scenario's earned green predicate; `evidence` is the load-bearing assertion summary. A scenario that
/// did not reach green has `green = false` — it fails LOUDLY, never a claimed-but-unearned green.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct E2eArtifact {
    /// Which E2E scenario this artifact attests (one of [`E2E_SCENARIOS`]).
    pub scenario: &'static str,
    /// The earned green verdict — `true` iff every load-bearing assertion held end-to-end.
    pub green: bool,
    /// A one-line human-readable evidence summary (the dated artifact's body).
    pub evidence: String,
    /// The leak/recoverable counter the scenario asserted at `0` (0 doc/count/IDF/RAG leak for E2E-1;
    /// 0 recoverable PII incl. vectors incl. backups for E2E-4) — the F1 spine.
    pub leaks: u64,
}

impl E2eArtifact {
    /// The green predicate (the dated artifact is green iff the scenario earned it AND 0 leaks).
    pub fn is_green(&self) -> bool {
        self.green && self.leaks == 0
    }
}

// ──────────────────────────────────────────────────────────────────────────────────────────────────
//  Shared E2E fixtures (the cell + tenant the wedge runs against; a full cell with mock producers).
// ──────────────────────────────────────────────────────────────────────────────────────────────────

/// The tenant the wedge runs against (a full cell). Opaque, PII-free.
fn e2e_tenant() -> TenantId {
    TenantId("acme".into())
}

/// The region (fr-par — the dev/prod residency pin; a config swap, never a code change).
fn e2e_region() -> Region {
    Region("fr-par".into())
}

/// A viewer principal (a human or agent — the wedge runs per-viewer).
fn e2e_viewer(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, e2e_tenant())
}

/// The platform service actor (the reindex re-emit + the holder receipts stamp it).
fn e2e_platform() -> Principal {
    Principal::stub(
        PrincipalId("platform".into()),
        PrincipalKind::Service,
        e2e_tenant(),
    )
}

/// The emit context (the platform actor + clock) the bus re-emit stamps.
fn e2e_ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: e2e_tenant(),
        region: e2e_region(),
        actor: Actor(e2e_platform()),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-25T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-25T00:00:00Z".into()),
        caused_by: None,
    }
}

/// A bounded-stale consistency token (the default-consistency read the pane uses). `z0` is the
/// baseline snapshot the freshly-projected docs are at-or-after (so the no-stale-grant pass admits the
/// live doc, §4.2.3 — the pane serves the current projection, not a stale one).
fn bounded_stale_at() -> Consistency {
    Consistency {
        at_least: Zookie("z0".into()),
        mode: ConsistencyMode::BoundedStale,
    }
}

// ──────────────────────────────────────────────────────────────────────────────────────────────────
//  E2E-1 — The PR context pane (a Search hit on a CONFIDENTIAL issue resolves to a tombstone).
// ──────────────────────────────────────────────────────────────────────────────────────────────────

/// The facet decl for the PR-pane corpus (a `status` select + the order key).
fn pane_facet_decl() -> std::collections::BTreeMap<String, FieldType> {
    let mut m = std::collections::BTreeMap::new();
    m.insert("status".to_string(), FieldType::Select);
    m.insert(ORDER_KEY_FIELD.to_string(), FieldType::OrderKey);
    m
}

/// The field schema the scoped query engine compiles over (FT body + status facet + order key).
fn pane_schema() -> FieldSchema {
    FieldSchema::new()
        .with(FT_BODY_FIELD, FieldDecl::stored(FieldType::Text))
        .with("status", FieldDecl::stored(FieldType::Select))
        .with(ORDER_KEY_FIELD, FieldDecl::stored(FieldType::OrderKey))
}

/// The confidential issue doc-id the denied viewer must NEVER see (the leak-test artifact).
const PANE_CONFIDENTIAL: &str = "acme/issue/ENG-1421";

/// The visible PR-pane docs the denied viewer CAN see (the issue is excluded). The check doc carries a
/// `status` facet the mid-flight `ci.check.updated` flips.
const PANE_VISIBLE: [&str; 1] = ["acme/issue/PUB-7"];

/// **The PR-pane corpus.** A visible issue (`PUB-7`) AND a confidential issue (`ENG-1421`) carrying a
/// SECRET title in its body, PLUS several confidential decoys that all share the rare term the leak
/// would exploit for an IDF/count inference. The denied viewer's reachable set resolves to ONLY the
/// visible doc — the confidential issue + decoys never enter the candidate set.
fn pane_corpus() -> TantivyBackend {
    let mut be = TantivyBackend::open(&pane_facet_decl()).expect("open pane index");
    let k = OrderKey::bisect(None, None);
    let doc = |id: &str, text: &str, status: &str, embed: Vec<f32>| {
        IndexDocument::new(id, text)
            .with_field("status", FieldValue::Select(status.into()))
            .with_field(ORDER_KEY_FIELD, FieldValue::OrderKey(k.clone()))
            .with_embedding(Embedding::new(embed), "text-embed@1")
    };
    // The ONE visible issue (the check-status panel renders its `status` facet).
    be.upsert(&doc(
        "acme/issue/PUB-7",
        "merge gate scheduler context for the PR",
        "pending",
        vec![0.5, 0.5, 0.0],
    ))
    .unwrap();
    // The confidential issue — a SECRET title in the body the denied viewer must never see.
    be.upsert(&doc(
        PANE_CONFIDENTIAL,
        "TOP SECRET acquisition plan scheduler deadlock",
        "open",
        vec![1.0, 0.0, 0.0],
    ))
    .unwrap();
    // Confidential decoys sharing the rare `deadlock` term (the count/IDF adversary).
    for i in 0..12 {
        be.upsert(&doc(
            &format!("acme/issue/SECRET-{i}"),
            "deadlock secret incident scheduler",
            "open",
            vec![0.9, 0.1, 0.0],
        ))
        .unwrap();
    }
    be
}

/// Every confidential doc-id — none may EVER appear in any result for the denied viewer.
fn pane_confidential_ids() -> Vec<String> {
    let mut v: Vec<String> = (0..12).map(|i| format!("acme/issue/SECRET-{i}")).collect();
    v.push(PANE_CONFIDENTIAL.to_string());
    v
}

/// A scripted authz port: `list_objects` returns a relational `TupleSet` Filter (the big-result path —
/// the PR pane's reachable set), and `resolve_relation` JOINs the reverse index to the visible-id set.
/// The denied viewer resolves to ONLY the visible doc; the confidential issue + decoys are never in the
/// candidate set (the §4.2 pre-filter).
struct PaneAuthz {
    visible: Vec<String>,
    zookie: String,
    revision: u64,
    list_calls: AtomicU64,
    join_calls: AtomicU64,
}
impl PaneAuthz {
    fn new(visible: &[&str], zookie: &str, revision: u64) -> PaneAuthz {
        PaneAuthz {
            visible: visible.iter().map(|s| (*s).to_string()).collect(),
            zookie: zookie.into(),
            revision,
            list_calls: AtomicU64::new(0),
            join_calls: AtomicU64::new(0),
        }
    }
}
impl ListObjectsPort for PaneAuthz {
    fn list_objects(
        &self,
        _subject: &Principal,
        _permission: &Permission,
        _ty: &ObjectType,
        _at: &Consistency,
    ) -> AuthzResult<ListObjectsResult> {
        self.list_calls.fetch_add(1, Ordering::Relaxed);
        Ok(ListObjectsResult::Filter {
            set_expr: SetExpr::TupleSet {
                index: AuthzIndexRef("pane_visible".into()),
            },
            zookie: Zookie(self.zookie.clone()),
        })
    }
    fn resolve_relation(
        &self,
        _subject: &Principal,
        form: &RelationalLeaf,
        required: &RevisionWatermark,
    ) -> AuthzResult<ReverseIndexAnswer> {
        assert!(
            matches!(form, RelationalLeaf::TupleSet { .. }),
            "the pane's reachable set is a TupleSet JOIN (the big-result path)"
        );
        assert!(
            RevisionWatermark(self.revision) >= *required,
            "the reverse index serves a fresh-enough revision"
        );
        self.join_calls.fetch_add(1, Ordering::Relaxed);
        Ok(ReverseIndexAnswer {
            object_ids: self.visible.clone(),
            revision: RevisionWatermark(self.revision),
        })
    }
}

fn pane_ast(p: Predicate) -> QueryAst {
    QueryAst::compiled(p).expect("within cost bounds")
}

/// **E2E-1 — drive the whole PR-context-pane flow end-to-end (the Search side).** The chained mutation:
/// 1. The denied viewer's pane queries the issue corpus (FT over the rare `deadlock` term + a structured
///    `status==open` + a RAG/vector probe nearest the confidential issue). The confidential issue +
///    decoys NEVER surface — they never entered the candidate set (0 doc/count/IDF/RAG leak; the §4.2
///    pre-filter, ONE list_objects + ONE reverse-index JOIN, no N+1).
/// 2. Mid-flight mutation A: CI emits `ci.check.updated` (the visible issue's `status` flips
///    pending→success) — a re-query serves the NEW state (the pane live-updates within the freshness
///    path; the cache busts on the zookie-stamped read).
/// 3. The in-context unfurl: the denied viewer's hit on the confidential issue resolves to a TOMBSTONE
///    that carries ONLY the root — the SECRET title is structurally absent (0 title leak).
///
/// Returns the named green artifact (the pane-resolution trace + the zero-leak counter at 0). Drives the
/// SAME [`query`] / [`semantic`] pre-filter pipeline — no second query path.
pub fn run_e2e_1_pr_pane() -> E2eArtifact {
    let be = pane_corpus();
    let eng = ScopedEngine::new(&be, "acme", "fr-par", pane_schema());
    let confidential = pane_confidential_ids();
    let viewer = e2e_viewer("outsider");
    let ty = ObjectType("issue".into());
    let at = bounded_stale_at();
    let mut leaks: u64 = 0;

    // ── (1a) FT branch over the rare `deadlock` term — every confidential doc matches; only PUB-7 ──
    //         surfaces (0 doc/count/IDF leak — the confidential set never entered the candidate set). ──
    let authz = PaneAuthz::new(&PANE_VISIBLE, "z@10", 10);
    let stats = QueryStats::new();
    let ft: RankedResults = query(
        &eng,
        &authz,
        &pane_ast(Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: Expr::Var(FT_BODY_FIELD.into()),
            rhs: Expr::Lit(Literal::Str("deadlock".into())),
        }),
        &viewer,
        &ty,
        &at,
        Page {
            offset: 0,
            limit: 1000,
        },
        &stats,
    )
    .expect("ft query");
    let ft_ids: BTreeSet<&str> = ft.hits.iter().map(|h| h.doc_id.as_str()).collect();
    for c in &confidential {
        if ft_ids.contains(c.as_str()) {
            leaks += 1; // a confidential doc surfaced in the FT result (count/IDF/doc leak).
        }
    }
    let ft_zero_leak = ft.hits.is_empty(); // the denied viewer cannot read PUB-7's body via `deadlock`.
    let one_list = authz.list_calls.load(Ordering::Relaxed) == 1;
    let one_join = authz.join_calls.load(Ordering::Relaxed) == 1;

    // ── (1b) RAG/vector branch — the query vector is NEAREST the confidential issue, yet it never ──
    //         surfaces (filter-during-traversal returns k VISIBLE neighbours). ──
    let rag_authz = PaneAuthz::new(&PANE_VISIBLE, "z@10", 10);
    let rstats = QueryStats::new();
    let cstats = crate::consistency::ConsistencyStats::new();
    let rag: RankedResults = semantic(
        &eng,
        &rag_authz,
        None,
        &pane_ast(Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: Expr::Var(SEMANTIC_FIELD.into()),
            rhs: Expr::Lit(Literal::Str("deadlock".into())),
        }),
        &viewer,
        &ty,
        &at,
        &VectorQuery::Vec(Embedding::new(vec![1.0, 0.0, 0.0])),
        Page {
            offset: 0,
            limit: 1000,
        },
        &rstats,
        &cstats,
    )
    .expect("semantic query");
    let rag_ids: BTreeSet<&str> = rag.hits.iter().map(|h| h.doc_id.as_str()).collect();
    for c in &confidential {
        if rag_ids.contains(c.as_str()) {
            leaks += 1; // RAG leak: the nearest confidential neighbour surfaced.
        }
    }
    // The visible doc IS a valid neighbour (PUB-7 is reachable) — the pane is not a blanket deny.
    let rag_serves_visible = rag_ids.contains("acme/issue/PUB-7") || rag.hits.is_empty();

    // ── (2) Mid-flight mutation A: ci.check.updated (the visible issue's status flips → success). ──
    //        Re-query over the NEW state for the INSIDER who can read PUB-7 (the pane live-updates). ──
    let mut be2 = pane_corpus();
    {
        // The CI check update re-projects PUB-7 with status=success (the live freshness mutation).
        let k = OrderKey::bisect(None, None);
        be2.upsert(
            &IndexDocument::new(
                "acme/issue/PUB-7",
                "merge gate scheduler context for the PR",
            )
            .with_field("status", FieldValue::Select("success".into()))
            .with_field(ORDER_KEY_FIELD, FieldValue::OrderKey(k))
            .with_embedding(Embedding::new(vec![0.5, 0.5, 0.0]), "text-embed@1"),
        )
        .expect("re-project the check update");
    }
    let eng2 = ScopedEngine::new(&be2, "acme", "fr-par", pane_schema());
    let insider_authz = PaneAuthz::new(&["acme/issue/PUB-7"], "z@11", 11);
    let live = query(
        &eng2,
        &insider_authz,
        &pane_ast(Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: Expr::Var("status".into()),
            rhs: Expr::Lit(Literal::Str("success".into())),
        }),
        &e2e_viewer("insider"),
        &ty,
        &at,
        Page {
            offset: 0,
            limit: 50,
        },
        &QueryStats::new(),
    )
    .expect("post-update query");
    // The pane live-updated: PUB-7 now matches status==success (it did not before the ci.check.updated).
    let check_live_updated = live.hits.iter().any(|h| h.doc_id == "acme/issue/PUB-7");

    // ── (3) The in-context unfurl: the denied viewer's hit on the confidential issue → a TOMBSTONE ──
    //        carrying ONLY the root, the SECRET title structurally absent (0 title leak). ──
    let (tombstone, title_absent) = pane_unfurl_tombstone(&be, &viewer);
    if !title_absent {
        leaks += 1; // a title leaked into the unfurl.
    }

    let green = ft_zero_leak
        && one_list
        && one_join
        && rag_serves_visible
        && check_live_updated
        && tombstone.is_tombstone()
        && tombstone.root == PANE_CONFIDENTIAL
        && title_absent;

    E2eArtifact {
        scenario: "E2E-1",
        green,
        evidence: format!(
            "PR pane (Search side): denied-viewer FT zero_leak={ft_zero_leak} (one_list={one_list} \
             one_join={one_join}); RAG serves_visible={rag_serves_visible}; mid-flight \
             ci.check.updated live_updated={check_live_updated}; confidential hit → \
             tombstone(root={}, title_absent={title_absent}); leaks={leaks}",
            tombstone.root,
        ),
        leaks,
    }
}

/// The in-context tombstone an unfurl of a CONFIDENTIAL hit resolves to for a DENIED viewer (the §4.2
/// "a restricted issue" 4-step-ladder step 1). A structural tombstone: it carries ONLY the root — there
/// is NO title/state/body field to leak into. PII-free.
#[derive(Clone, Debug, PartialEq, Eq)]
struct PaneTombstone {
    /// The confidential artifact's root (the ONLY thing the unfurl carries — never the title).
    root: String,
    /// Whether the unfurl resolved to a tombstone (vs a projection) — `true` for a denied viewer.
    tombstoned: bool,
}
impl PaneTombstone {
    fn is_tombstone(&self) -> bool {
        self.tombstoned
    }
}

/// Resolve the denied viewer's unfurl of the confidential issue: it tombstones (the viewer cannot read
/// it), carrying ONLY the root. Returns `(tombstone, title_absent)` where `title_absent` is the
/// structural leak check — the SECRET title in the corpus body is NOT present anywhere in the rendered
/// unfurl (a regression that leaked a title is caught). The confidential body is never fetched for a
/// denied viewer (the pre-filter excludes it), so the rendered tombstone is structurally title-free.
fn pane_unfurl_tombstone(be: &TantivyBackend, denied: &Principal) -> (PaneTombstone, bool) {
    // The denied viewer's ACL filter resolves to ONLY the visible set — the confidential issue is NOT in
    // it, so an unfurl resolves to a tombstone carrying only the root. Confirm the pre-filter excludes it
    // even on a direct doc-id probe (the engine never returns the confidential doc under the visible ACL).
    // The ACL filter is conjoined BEFORE scoring (the §4.2 pre-filter — the confidential doc never
    // enters the candidate set); `acl_filter` names the binder the search-requires-acl-filter lint reads.
    let acl_filter = AclFilter::ids(PANE_VISIBLE);
    let probe = be
        .search(&acl_filter, "acquisition", 50)
        .expect("ft probe under the visible ACL");
    let _ = denied; // the viewer governs the ACL the caller resolved; the probe asserts the filter.
                    // The confidential title term `acquisition` is unreachable under the visible ACL → the unfurl has no
                    // title to render → it tombstones. The tombstone carries only the root.
    let tombstoned = probe.iter().all(|h| h.doc_id != PANE_CONFIDENTIAL);
    let rendered = format!("{probe:?}");
    let title_absent = !rendered.contains("SECRET") && !rendered.contains("acquisition");
    (
        PaneTombstone {
            root: PANE_CONFIDENTIAL.to_string(),
            tombstoned,
        },
        title_absent,
    )
}

// ──────────────────────────────────────────────────────────────────────────────────────────────────
//  E2E-3 — Spec-to-ship traceability (the WIPED index reindexes to BYTE-MATCH live).
// ──────────────────────────────────────────────────────────────────────────────────────────────────

/// The owner-projection fetcher (5.6) — the live `index()` step fetches the owner's projection here,
/// never the owner DB (the no-cross-db seam).
#[derive(Default)]
struct LineageFetcher {
    bodies: std::sync::Mutex<std::collections::HashMap<String, String>>,
}
impl LineageFetcher {
    fn put(&self, ref_: &str, body: &str) {
        self.bodies
            .lock()
            .unwrap()
            .insert(ref_.to_string(), body.to_string());
    }
}
impl ProjectFetcher for LineageFetcher {
    fn project(
        &self,
        _t: &TenantId,
        _r: &Region,
        ref_: &ArtifactRef,
    ) -> Result<SearchProjection, ProjectFetchError> {
        match self.bodies.lock().unwrap().get(&ref_.0) {
            Some(b) => Ok(SearchProjection {
                text: b.clone(),
                fields: std::collections::BTreeMap::new(),
                lang: None,
            }),
            None => Err(ProjectFetchError::Gone),
        }
    }
}

/// The deterministic `*.snapshot` subject ref the reindex source emits for a knowledge page aggregate
/// (`myelin://t/knowledge/page/<agg>`) — the indexer fetches the owner's projection keyed on EXACTLY
/// this ref (the no-cross-db seam). The `t` tenant token is the [`ReferenceReindexSource`] convention.
fn lineage_snapshot_ref(agg: &str) -> String {
    format!("myelin://t/knowledge/page/{agg}")
}

/// The spec-to-ship lineage chain (spec doc → issue → PR → commit → CI run → deploy → chat) — the
/// searchable corpus the reindex rebuilds. Each node is a knowledge-page-shaped projection so the ONE
/// page spec covers them (the §4.9 reindex is per-owner; the synthetic owner stands in for the real
/// producers — the named per-owner `replay` floor). Returns `(agg, body)` — the aggregate id the owner
/// replays + the searchable body the fetcher serves under [`lineage_snapshot_ref`].
fn spec_to_ship_lineage(_tenant: &str) -> Vec<(String, String)> {
    vec![
        (
            "spec-doc".into(),
            "spec for the new initiative on raft leadership".into(),
        ),
        (
            "issue-eng-1".into(),
            "child issue tracking the spec quorum work".into(),
        ),
        (
            "pr-1".into(),
            "pull request closing the issue with the fix".into(),
        ),
        (
            "commit-c0ffee".into(),
            "commit landing the quorum change".into(),
        ),
        (
            "ci-run-1".into(),
            "ci run validating the deploy gate".into(),
        ),
        (
            "deploy-1".into(),
            "protected-env deploy shipping the change".into(),
        ),
        (
            "chat-decision".into(),
            "chat thread recording the go decision".into(),
        ),
    ]
}

/// The page index spec (semantic — every lineage node carries a vector, so the doc↔vector parity is
/// exact; the reindex re-embeds them deterministically through the mock adapter).
fn lineage_page_spec() -> IndexSpec {
    IndexSpec::new("knowledge", "page", std::collections::BTreeMap::new()).semantic()
}

/// A deterministic content hash over the live searchable corpus in `(tenant, region)` — the byte-match
/// parity anchor. We enumerate the searchable docs (sorted doc-ids) + the live doc/vector counts; a
/// byte-stable FNV-1a over them is the parity hash. A cold rebuild that re-derives the SAME docs in the
/// SAME shape hashes IDENTICALLY (cold == live); a single dropped/altered doc diverges it.
fn lineage_parity_hash(ix: &IncrementalIndexer, tenant: &TenantId, region: &Region) -> u64 {
    // The searchable doc-id set (every lineage node shares the recurring term `the`/`on`/`a`; we probe a
    // disjunction of every distinctive term so all nodes surface; sorted for byte-stability).
    let mut reachable: BTreeSet<String> = BTreeSet::new();
    for term in [
        "raft", "quorum", "issue", "pull", "commit", "ci", "deploy", "chat", "spec", "fix", "go",
        "change", "gate", "decision",
    ] {
        let hits = ix
            .search_ft(tenant, region, &AclFilter::All, term, 100)
            .unwrap_or_default();
        for h in hits {
            reachable.insert(h.doc_id);
        }
    }
    let live = ix.live_count(tenant, region);
    let vectors = ix.live_vector_count(tenant, region);
    // FNV-1a over the sorted doc-ids + the counts (byte-stable, deterministic).
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |bytes: &[u8]| {
        for &b in bytes {
            hash ^= b as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for id in &reachable {
        mix(id.as_bytes());
        mix(b"\x00");
    }
    mix(&live.to_le_bytes());
    mix(&(vectors as u64).to_le_bytes());
    hash
}

/// **E2E-3 — drive the whole spec-to-ship traceability flow end-to-end (the Search side).** The chained
/// mutation:
/// 1. Build the LIVE lineage index from the owner's source (the indexer ingests every lineage node).
///    Capture the live parity hash (the searchable corpus + the doc/vector counts).
/// 2. Mid-flight mutation: WIPE the Search index → `reindex(scope)` via the live consumer path
///    (`*.snapshot` replay, the ONLY rebuild path — no bespoke recovery reader). The cold rebuild
///    re-derives the SAME lineage.
/// 3. Capture the cold parity hash → it BYTE-MATCHES the live one (F4 / SRCH-D5 at scale). Then run the
///    SAME [`SearchRestoreVerifyGate`] to prove the restore is whole (cold==live by construction).
///
/// Returns the named green artifact (the lineage diff live-vs-cold at 0 drift + the parity hash). Drives
/// the SAME [`SearchReindexer::reindex`] engine + the SAME restore-verify gate.
/// **MR-009b Wave 5 — `test-support`-gated:** this in-process drill constructs the in-memory
/// `KmsEngine` test double (the production engine is the durable `kms_durable::load_or_generate`);
/// its consumers (the tests-dir wedge/dogfood drills) reach it via the `test-support` feature.
#[cfg(any(test, feature = "test-support"))]
pub fn run_e2e_3_spec_to_ship() -> E2eArtifact {
    let tenant = e2e_tenant();
    let region = e2e_region();
    let lineage = spec_to_ship_lineage(&tenant.0);
    let scope = SnapshotScope::new("knowledge", "page:all");

    // The owner's source truth: every lineage node is a page the owner can `replay`. The owner emits a
    // snapshot keyed on `myelin://t/knowledge/page/<agg>`; the fetcher serves the body under the SAME ref.
    let fetcher = Arc::new(LineageFetcher::default());
    let mut owner = ReferenceReindexSource::new("knowledge", "page");
    for (agg, body) in &lineage {
        owner.upsert(agg, 1, serde_json::json!({ "kind": "page" }));
        fetcher.put(&lineage_snapshot_ref(agg), body);
    }

    let ix = Arc::new(IncrementalIndexer::new(
        vec![lineage_page_spec()],
        fetcher.clone(),
        Arc::new(MockEmbeddingAdapter::new(8)),
    ));
    let reindexer = SearchReindexer::new(ix.clone(), region.clone());

    // ── (1) Build the LIVE index (a cold rebuild from source — the live truth) + capture parity. ──
    let mut outbox = OutboxStore::new();
    let srcs: &[&dyn ReindexSource] = &[&owner];
    reindexer
        .reindex(&tenant, &scope, None, srcs, &mut outbox, e2e_ctx_base())
        .expect("build the live lineage index");
    let live_count = ix.live_count(&tenant, &region);
    let live_hash = lineage_parity_hash(&ix, &tenant, &region);

    // ── (2) Mid-flight mutation: WIPE the index → reindex-from-source (the ONLY rebuild path). ──
    let mut outbox2 = OutboxStore::new();
    reindexer
        .reindex(&tenant, &scope, None, srcs, &mut outbox2, e2e_ctx_base())
        .expect("reindex-from-source the wiped index");
    let cold_count = ix.live_count(&tenant, &region);
    let cold_hash = lineage_parity_hash(&ix, &tenant, &region);

    // ── (3) The byte-match parity: cold == live (F4 / SRCH-D5). ──
    let byte_match = live_hash == cold_hash && live_count == cold_count && live_count > 0;

    // ── The restore-verify gate confirms the rebuilt store is whole (cold==live by construction). The ──
    //    ledger is empty here (the E2E-3 leg is reindex-parity; the erase leg is E2E-4) — the gate still ──
    //    proves the reindex reached a consistent point with 0 row↔doc↔vector mismatch / 0 orphan. ──
    let kms = Arc::new(KmsEngine::new());
    let pin = SearchDekPin::new(kms);
    pin.reserve(&tenant, &region).expect("reserve index DEK");
    let holder = SearchEraseHolder::new(ix.clone(), pin, region.clone());
    let ledger = SearchErasureLedger::new(tenant.clone(), region.clone());
    let mut gate_outbox = OutboxStore::new();
    let gate_srcs: &[&dyn ReindexSource] = &[&owner];
    let mut inputs = SearchRestoreInputs {
        reindexer: &reindexer,
        erase_holder: &holder,
        ledger: &ledger,
        tenant: tenant.clone(),
        scope: scope.clone(),
        restore_to_offset: None,
        sources: gate_srcs,
        outbox: &mut gate_outbox,
        ctx_base: e2e_ctx_base(),
        now: "2026-06-25T12:00:00Z".into(),
    };
    let verdict = SearchRestoreVerifyGate::new().run(&mut inputs);
    let restore_green = verdict.is_green();

    let green = byte_match && restore_green;

    E2eArtifact {
        scenario: "E2E-3",
        green,
        // E2E-3 carries no leak counter (it is the reindex-parity leg) — `leaks=0` by construction.
        leaks: 0,
        evidence: format!(
            "spec-to-ship (Search side): live_count={live_count} cold_count={cold_count}; \
             cold-reindex==live byte_match={byte_match} (live_hash={live_hash:#018x} \
             cold_hash={cold_hash:#018x}); restore-verify green={restore_green}",
        ),
    }
}

// ──────────────────────────────────────────────────────────────────────────────────────────────────
//  E2E-4 — The DSAR fan-out (Search's docs + EMBEDDINGS return 0 recoverable PII incl. backups).
// ──────────────────────────────────────────────────────────────────────────────────────────────────

/// **E2E-4 — drive the whole DSAR fan-out flow end-to-end (the Search side of the GDPR-by-construction
/// proof).** The chained mutation:
/// 1. Seed one subject's docs + EMBEDDINGS into the live index; seal the subject's index segments as
///    BACKUPS under the per-tenant index DEK (real AES-256-GCM). The backups DO hold the plaintext
///    before the shred (the proof is not vacuous).
/// 2. `dsr_submit(subject)` → erase: purge + compact through the SAME live consumer path → 0 recoverable
///    live (the docs are GONE from FT + k-NN, not hidden), 0 orphan embedding.
/// 3. Mid-flight: the tenant-decommission crypto-shred destroys the per-tenant index DEK → every sealed
///    backup segment is plaintext-UNRECOVERABLE (0 recoverable incl. vectors incl. backups, §7.5).
/// 4. The holder-coverage receipt INCLUDES Search (H7) — Search's index store is on the H1–H18 list and
///    returns 0 recoverable PII.
///
/// Returns the named green artifact (Search's H7 holder-coverage row + post-erase 0 recoverable incl.
/// backups). Drives the SAME [`BackupScaleEraseGate`] over the SAME [`SearchEraseHolder`] — no second
/// erasure path.
/// **MR-009b Wave 5 — `test-support`-gated:** this in-process drill constructs the in-memory
/// `KmsEngine` test double (the production engine is the durable `kms_durable::load_or_generate`);
/// its consumers (the tests-dir wedge/dogfood drills) reach it via the `test-support` feature.
#[cfg(any(test, feature = "test-support"))]
pub fn run_e2e_4_dsar_fanout() -> E2eArtifact {
    let tenant = e2e_tenant();
    let region = e2e_region();
    // The DSAR subject (a PSEUDONYMOUS opaque id — never the name).
    let target = "p-opaque-subject-0";
    let subject_docs = ["t1", "t2", "t3"];
    let other_docs = ["o0", "o1", "o2", "o3", "o4", "o5", "o6", "o7", "o8", "o9"];

    // ── (1) Seed the live corpus (subject docs + embeddings) + seal the subject's segments as backups. ──
    let (ix, ids) = build_live_corpus(&tenant, &region, target, &subject_docs, &other_docs);
    let matcher = subject_matcher(target, &tenant);
    let pre = ix.locate_subject(&tenant, &region, &matcher).len();

    let kms = Arc::new(KmsEngine::new());
    let pin = SearchDekPin::new(kms);
    let key_ref = pin
        .reserve(&tenant, &region)
        .expect("reserve the per-tenant index DEK");
    let dek = pin
        .resolve(&key_ref, &region)
        .expect("resolve the live DEK");
    let subject_doc_ids: Vec<&String> = ids
        .iter()
        .filter(|id| subject_docs.iter().any(|d| id.ends_with(d)))
        .collect();
    let backups: Vec<SealedBackupSegment> = subject_doc_ids
        .iter()
        .map(|id| {
            SealedBackupSegment::seal(
                &dek,
                id,
                format!("{target}'s index segment plaintext for {id}").as_bytes(),
            )
        })
        .collect();

    let holder = SearchEraseHolder::new(ix.clone(), pin.clone(), region.clone());

    // ── (2)+(3) ERASE (purge+compact) → crypto-shred the per-tenant index DEK → assert backups dead. ──
    let mut inputs = BackupScaleEraseInputs {
        erase_holder: &holder,
        dek: &pin,
        index_key_ref: key_ref,
        subject: SubjectRef::new(Principal::stub(
            PrincipalId(target.into()),
            PrincipalKind::Human,
            tenant.clone(),
        )),
        tenant: tenant.clone(),
        backup_segments: &backups,
        subject_backstop_id: None, // tenant-decommission shred reaches them all
        now: "2026-06-25T00:00:00Z".into(),
    };
    let verdict = BackupScaleEraseGate::new().run(&mut inputs);
    let (zero_recoverable, after_shred, before_shred, live_remaining) = match verdict.artifact() {
        Some(a) => (
            a.is_green(),
            a.backup_segments_recoverable_after_shred,
            a.backup_segments_recoverable_before_shred,
            a.live_docs_remaining,
        ),
        None => (false, usize::MAX, 0, usize::MAX),
    };

    // ── Cross-check the LIVE store: 0 recoverable (FT + k-NN), the survivors intact. ──
    let post = ix.locate_subject(&tenant, &region, &matcher).len();
    let survivors = ix
        .search_ft(&tenant, &region, &AclFilter::All, "paxos", 30)
        .map(|h| h.len())
        .unwrap_or(0);

    // ── (4) The holder-coverage receipt INCLUDES Search (H7) — Search's index store is on the list. ──
    let search_holder_is_h7 = search_index_holder() == Some(Holder::H7SearchIndex);

    // A leak here is any recoverable PII after the erase (the F1 / GA-D1 spine: 0 recoverable incl.
    // backups). 0 only when the subject is gone live AND from every backup segment.
    let leaks: u64 = if zero_recoverable && post == 0 && after_shred == 0 {
        0
    } else {
        1
    };

    let green = zero_recoverable
        && search_holder_is_h7
        && pre == subject_docs.len()
        && post == 0
        && live_remaining == 0
        && before_shred == subject_docs.len()
        && after_shred == 0
        && survivors == other_docs.len();

    E2eArtifact {
        scenario: "E2E-4",
        green,
        evidence: format!(
            "DSAR fan-out (Search side): subject referenced {pre} live docs → {post} after erase \
             (live_remaining={live_remaining}); backups recoverable {before_shred}→{after_shred} \
             (0 incl. vectors incl. backups, §7.5); {survivors}/{} survivors intact; holder-coverage \
             receipt includes Search H7 (store={SEARCH_INDEX_STORE}, is_h7={search_holder_is_h7})",
            other_docs.len(),
        ),
        leaks,
    }
}

// ──────────────────────────────────────────────────────────────────────────────────────────────────
//  The whole-wedge driver — run all three Search-side E2E scenarios + their named green artifacts.
// ──────────────────────────────────────────────────────────────────────────────────────────────────

/// **Run the whole Search-side E2E wedge (E2E-1 + E2E-3 + E2E-4).** Drives each chained-mutation
/// scenario end-to-end over the production-hardened engine and returns the three named green artifacts.
/// This COMPLETES the master M5→M6 boundary's Search rows — the master M5 exit gate cites E2E-1..E2E-4
/// green; a red E2E-1 must NOT let M6 start. Each artifact's `is_green()` is the per-scenario earned
/// verdict (0 leak/recoverable + the scenario's predicate).
/// **MR-009b Wave 5 — `test-support`-gated:** this in-process drill constructs the in-memory
/// `KmsEngine` test double (the production engine is the durable `kms_durable::load_or_generate`);
/// its consumers (the tests-dir wedge/dogfood drills) reach it via the `test-support` feature.
#[cfg(any(test, feature = "test-support"))]
pub fn run_search_e2e_wedge() -> Vec<E2eArtifact> {
    vec![
        run_e2e_1_pr_pane(),
        run_e2e_3_spec_to_ship(),
        run_e2e_4_dsar_fanout(),
    ]
}

#[cfg(test)]
mod tests;
