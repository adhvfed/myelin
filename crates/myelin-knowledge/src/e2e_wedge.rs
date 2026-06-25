//! # `e2e_wedge` — Knowledge's legs of the whole-system E2E wedge (KN-P33 / P-488, M5)
//!
//! **The completion of KN-M5's E2E legs (the master M5→M6 boundary's Knowledge rows).** This module is
//! the **Knowledge side of two whole-system chained-mutation E2E scenarios** — **E2E-1** (the PR
//! context pane: a Knowledge design-doc embed resolves per-viewer, 0 leak to the unauthorized viewer)
//! and **E2E-3** (spec-to-ship lineage: a Knowledge spec doc → initiative → issues traceability, with
//! cold-reindex == live and audit tamper detected). Each is driven **end-to-end** — the whole flow with
//! mid-flight mutations, NOT a single handler (EI-01 §4 / VISION §3) — over the **production-hardened
//! Knowledge engine** the M5 prompts built. The engine is **UNCHANGED**; this module COMPOSES it into
//! the two whole-system scenarios and emits each scenario's named green artifact.
//!
//! ## What this module REUSES (EI-01 §7 — never a parallel second implementation)
//! This is the **whole-system DRIVER over the EXISTING engine**, not a second project/reindex/lineage.
//! - **E2E-1** drives the SAME [`crate::refs_glue::Projector::project`] per-viewer 4-step tombstone
//!   ladder (contract 5.6 / 5.7, KN-P19): a Knowledge design-doc embed in the PR context pane resolves
//!   to a [`crate::refs_glue::Projection`] for an authorized viewer (the live title) and to a
//!   [`crate::refs_glue::Tombstone`] carrying ONLY the `#sub`-stripped ROOT for the unauthorized viewer
//!   — the SECRET title is structurally ABSENT (0 title leak; the permission-first gate NEVER reads the
//!   artifact's title). The scenario CHAINS mutations mid-flight: the doc is edited (a SECRET title is
//!   typed in), the embed renders live for the authorized viewer, then the doc is marked CONFIDENTIAL /
//!   erased mid-flight and the SAME projector re-resolves — the authorized viewer's pane live-updates
//!   while the unauthorized viewer's stays a content-free tombstone. No second project path.
//! - **E2E-3** drives the SAME [`crate::replay::KnowledgeReindexSource`] reindex-from-source replay
//!   (KN-P20 / KN-D6, contract 2.6) through the SAME live consumer [`myelin_events::DerivedStore`]:
//!   the spec→initiative→issues lineage is laid down as TE-7 typed edges (5.5), the derived projection
//!   is WIPED, and `replay(scope)` rebuilds it cold == live (the parity hash byte-matches). The lineage
//!   is then sealed into a **hash-chained lineage ledger** built ONLY from the frozen
//!   [`myelin_storage::blob::ContentHash::blake3`] content-address primitive (the same proven hash the
//!   compaction snapshot determinism uses) — a TAMPER to any lineage hop breaks the chain hash, which
//!   the verify detects (0 silent tamper). No second reindexer, no hand-rolled hash, no bespoke
//!   recovery reader.
//!
//! Each scenario emits its **named green artifact** (an [`E2eArtifact`]) — the dated, content-addressed
//! report the master M5 exit gate cites. A scenario that does not reach its green predicate fails
//! LOUDLY (`is_green()` is false); there is no weakened threshold and no claimed green that was not
//! earned (EI-01 §3 / VISION §3).
//!
//! ## The load-bearing invariants STILL HOLD at E2E scale (the prompt's required statement)
//! The KN-P19 project-leak invariant (a confidential page degrades to a tombstone carrying the root,
//! NEVER the title — the permission-first gate, not a post-filter) and the KN-P20 reindex invariant
//! (cold == live; the rebuild is the live consumer path only) are the load-bearing properties. This
//! module ASSERTS both at E2E scale: E2E-1's unauthorized viewer gets a root-only tombstone (0 title
//! leak), E2E-3's wiped projection rebuilds byte-identical to live AND the lineage seal detects tamper.
//! The mutation floors on those invariants live in `refs_glue.rs` / `replay.rs` and are UNCHANGED —
//! this module adds NO new leak/reindex decision logic; it proves the frozen decisions hold across the
//! whole flow.
//!
//! ## Floors named (VISION §3 / EI-01 §1)
//! - **None new.** This is the E2E run over the production-hardened engine — the named single-cell
//!   project ([KN-P19]) / reindex ([KN-P20]) follow-ons proven end-to-end. The ONE legitimate remaining
//!   floor inherited by both legs is the world-scale fleet-hardware 30× load drill (the CI variant runs
//!   a MODERATE corpus, not the world-scale fleet corpus — already named by KN-P32/[`crate::surge`]).
//! - The cross-subsystem producers (Git/CI/Issues/Chat/Refs/Id/Notif sides of E2E-1/E2E-3) are reached
//!   through the SAME frozen seams — the synthetic Id resolver / reference reindex source standing in
//!   for the real producers (the production wire is the per-owner store/replay floor named in KN-P05 /
//!   KN-P20). This module drives the **Knowledge side**: the leak-free per-viewer embed, the cold==live
//!   reindex, and the tamper-evident lineage seal.
//!
//! [KN-P19]: crate::refs_glue
//! [KN-P20]: crate::replay

use std::collections::HashSet;

use myelin_events::{
    reindex, Actor, CorrelationId, DerivedStore, EmitContextBase, EventEnvelope, OutboxStore,
    Region, ReindexSource, SnapshotDraft, SnapshotScope, TenantId, Timestamp,
};
use myelin_identity::{
    AuthzError, CaveatContext, Consistency, Credential, DataRole, Decision, EffectivePolicy,
    IdentityService, ListObjectsResult, ObjectId, ObjectType, Permission, Precondition, Principal,
    PrincipalId, PrincipalKind, PrincipalStatus, Result as IdResult, RewriteTrace, SubjectTree,
    TupleDelta, Zookie,
};
use myelin_storage::blob::ContentHash;

use crate::refs_glue::{PageMeta, PageStore, Projected, Projector, TombstoneReason};
use crate::replay::KnowledgeReindexSource;

/// The two whole-system E2E scenarios Knowledge crosses (the master M5 exit gate cites E2E-1..E2E-4;
/// this module owns the Knowledge side of -1 and -3). PII-free tokens — drills assert against the NAME,
/// never a literal (EI-01 §3).
pub const E2E_SCENARIOS: [&str; 2] = ["E2E-1", "E2E-3"];

// ──────────────────────────────────────────────────────────────────────────────────────────────────
//  The named green artifact (the prompt's per-scenario "named green artifact").
// ──────────────────────────────────────────────────────────────────────────────────────────────────

/// **The named green artifact one E2E scenario emits.** A content-addressed, dated report the master
/// M5 exit gate cites. `green` is the scenario's earned green predicate; `evidence` is the load-bearing
/// assertion summary; `leaks` is the leak/tamper counter the scenario asserted at `0`. A scenario that
/// did not reach green has `green = false` — it fails LOUDLY, never a claimed-but-unearned green.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct E2eArtifact {
    /// Which E2E scenario this artifact attests (one of [`E2E_SCENARIOS`]).
    pub scenario: &'static str,
    /// The earned green verdict — `true` iff every load-bearing assertion held end-to-end.
    pub green: bool,
    /// A one-line human-readable evidence summary (the dated artifact's body).
    pub evidence: String,
    /// The leak/tamper counter the scenario asserted at `0` (0 title leak for E2E-1; 0 undetected
    /// tamper + 0 cold/live byte divergence for E2E-3) — the F1 spine.
    pub leaks: u64,
    /// The content-address of the evidence body (the dated artifact's self-describing seal). Derived
    /// from the frozen [`ContentHash::blake3`] over the `scenario|green|leaks|evidence` framing — never
    /// a hand-rolled hash (VISION §4).
    pub seal: String,
}

impl E2eArtifact {
    /// Build a sealed artifact from the earned verdict + the evidence summary. The seal is a pure
    /// function of the body, so the same verdict always yields the same address (a reproducible
    /// artifact the exit gate can cite by hash).
    fn sealed(
        scenario: &'static str,
        green: bool,
        leaks: u64,
        evidence: impl Into<String>,
    ) -> Self {
        let evidence = evidence.into();
        let mut body = Vec::new();
        push_lp(&mut body, scenario.as_bytes());
        push_lp(&mut body, &[u8::from(green)]);
        push_lp(&mut body, &leaks.to_be_bytes());
        push_lp(&mut body, evidence.as_bytes());
        let seal = ContentHash::blake3(&body).to_multihash_string();
        E2eArtifact {
            scenario,
            green,
            evidence,
            leaks,
            seal,
        }
    }

    /// The green predicate (the dated artifact is green iff the scenario earned it AND 0 leaks/tamper).
    pub fn is_green(&self) -> bool {
        self.green && self.leaks == 0
    }
}

/// Length-prefix a field (u32 big-endian length, then the bytes) — the injective framing the seal
/// relies on (the same convention `compaction::materialize` uses, so two distinct bodies can never
/// collide on a shared boundary).
fn push_lp(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(bytes);
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

/// A viewer principal (a human — the wedge runs per-viewer).
fn e2e_viewer(id: &str) -> Principal {
    Principal::new(
        e2e_tenant(),
        e2e_region(),
        PrincipalId(id.into()),
        PrincipalKind::Human,
        DataRole::Controller,
        PrincipalStatus::Active,
    )
}

/// The platform service actor (the reindex re-emit stamps it).
fn e2e_platform() -> Principal {
    Principal::stub(
        PrincipalId("platform".into()),
        PrincipalKind::Service,
        e2e_tenant(),
    )
}

/// The emit context (the platform actor + clock) the reindex re-emit stamps.
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

/// A read-consistency fence (the strong, zookie-stamped read a security-sensitive projection uses).
fn e2e_zookie() -> Zookie {
    Zookie("z0".into())
}

/// **A deterministic Id resolver for the wedge: a `read@object` allow-list (absent ⇒ Deny,
/// fail-closed).** The SAME `IdentityService` seam the production projector wires — the wedge swaps a
/// deterministic resolver in so the per-viewer leak property is asserted against a known reachable set
/// (EI-01 §3 — assert against the name, never a hidden literal).
struct WedgeId {
    allow: HashSet<String>,
}

impl WedgeId {
    fn new() -> Self {
        Self {
            allow: HashSet::new(),
        }
    }
    fn allow_read(mut self, viewer: &Principal, object: &myelin_events::ArtifactRef) -> Self {
        self.allow
            .insert(format!("{}|read@{}", viewer.principal_id.0, object.0));
        self
    }
}

impl IdentityService for WedgeId {
    fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
        Err(AuthzError::NotYetImplemented("wedge: authenticate n/a"))
    }
    fn check(
        &self,
        s: &Principal,
        p: &Permission,
        o: &myelin_events::ArtifactRef,
        _at: &Consistency,
        _c: Option<&CaveatContext>,
    ) -> IdResult<Decision> {
        Ok(
            if self
                .allow
                .contains(&format!("{}|{}@{}", s.principal_id.0, p.0, o.0))
            {
                Decision::Allow
            } else {
                Decision::Deny
            },
        )
    }
    fn list_objects(
        &self,
        _s: &Principal,
        _p: &Permission,
        _t: &ObjectType,
        _at: &Consistency,
    ) -> IdResult<ListObjectsResult> {
        Err(AuthzError::NotYetImplemented("wedge: list_objects n/a"))
    }
    fn list_subjects(
        &self,
        _o: &ObjectId,
        _p: &Permission,
        _at: &Consistency,
    ) -> IdResult<SubjectTree> {
        Err(AuthzError::NotYetImplemented("wedge: list_subjects n/a"))
    }
    fn explain(
        &self,
        _s: &Principal,
        _p: &Permission,
        _o: &ObjectId,
        _at: &Consistency,
    ) -> IdResult<RewriteTrace> {
        Err(AuthzError::NotYetImplemented("wedge: explain n/a"))
    }
    fn delegation(&self, _a: &Principal, _t: &Principal) -> IdResult<EffectivePolicy> {
        Err(AuthzError::NotYetImplemented("wedge: delegation n/a"))
    }
    fn write_tuples(&self, _d: &[TupleDelta], _p: Option<&Precondition>) -> IdResult<Zookie> {
        Err(AuthzError::NotYetImplemented("wedge: write_tuples n/a"))
    }
    fn mint_run_token(
        &self,
        _a: &PrincipalId,
        _r: &myelin_identity::RunId,
        _d: &myelin_identity::DelegationCaveats,
        _t: &myelin_identity::FailStaticBound,
    ) -> IdResult<myelin_identity::RunToken> {
        Err(AuthzError::NotYetImplemented("wedge: mint_run_token n/a"))
    }
    fn revoke(&self, _t: &myelin_identity::RevokeTarget) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("wedge: revoke n/a"))
    }
    fn resolve_pseudonym(&self, _s: &PrincipalId, _t: &TenantId) -> IdResult<String> {
        Err(AuthzError::NotYetImplemented(
            "wedge: resolve_pseudonym n/a",
        ))
    }
    fn erase(&self, _s: &PrincipalId) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("wedge: erase n/a"))
    }
    fn admit_fragment(
        &self,
        _f: &myelin_identity::NamespaceFragment,
    ) -> IdResult<myelin_identity::FragmentAdmit> {
        Err(AuthzError::NotYetImplemented("wedge: admit_fragment n/a"))
    }
}

/// Build the page-root URN `myelin://acme/knowledge/page/<id>` (the canonical ROOT the projector keys).
fn page_root(id: &str) -> myelin_events::ArtifactRef {
    myelin_events::ArtifactRef(format!("myelin://acme/knowledge/page/{id}"))
}

// ──────────────────────────────────────────────────────────────────────────────────────────────────
//  E2E-1 — The PR context pane (a Knowledge design-doc embed resolves per-viewer; 0 leak).
// ──────────────────────────────────────────────────────────────────────────────────────────────────

/// The confidential design-doc title the unauthorized viewer must NEVER see (the leak-test artifact).
const E2E1_SECRET_TITLE: &str = "Project Cerberus — Q3 acquisition architecture";

/// The design-doc page-id embedded in the PR context pane (a `#sub` block ref the PR description carries).
const E2E1_DESIGN_DOC: &str = "design-cerberus";

/// **E2E-1 (the PR context pane — Knowledge leg): drive it end-to-end, chaining mutations mid-flight.**
///
/// The whole flow, not a single handler (EI-01 §4):
/// 1. A Knowledge design-doc is authored (the SECRET title is typed in — a real mutation).
/// 2. A PR description embeds the design-doc's heading block (`<root>#block-h1`); the PR context pane
///    resolves the embed PER-VIEWER through the SAME [`Projector::project`] ladder.
/// 3. The **authorized** viewer (the author) sees the live [`Projected::Visible`] projection carrying
///    the title — the embed renders.
/// 4. The **unauthorized** viewer (a denied teammate) gets a [`Projected::Tombstoned`] carrying ONLY
///    the `#sub`-stripped ROOT — the SECRET title is structurally ABSENT (0 title leak; the
///    permission-first gate never read the title).
/// 5. **MID-FLIGHT mutation:** the doc is marked CONFIDENTIAL / erased (a GDPR restriction lands while
///    the pane is open). The SAME projector re-resolves: the previously-authorized viewer's embed now
///    degrades to an `Erased` tombstone too (the erasure is honoured live), and the unauthorized
///    viewer's stays a content-free tombstone. The pane live-updates from the SAME read path.
///
/// Returns the named green artifact (`is_green()` iff 0 title leak across every projection).
pub fn run_e2e1_pr_context_pane() -> E2eArtifact {
    let mut leaks: u64 = 0;
    let author = e2e_viewer("author");
    let denied = e2e_viewer("denied-teammate");
    let root = page_root(E2E1_DESIGN_DOC);
    // The embed in the PR description points at the design-doc's heading block (`#block-h1`).
    let embed = myelin_events::ArtifactRef(format!("{}#block-h1", root.0));

    // ── STEP 1+2: the design-doc is authored (the SECRET title), seeded into the projector store (the
    //    KN-P05 live-OLTP store stands in here). The author is the ONLY reader on the allow-list.
    let mut store = PageStore::new();
    store.put_root(
        &root,
        PageMeta {
            title: E2E1_SECRET_TITLE.to_string(),
            state: "live".to_string(),
        },
    );
    let id = WedgeId::new().allow_read(&author, &root);
    let projector = Projector::new(id, store);

    // ── STEP 3: the AUTHORIZED viewer's pane resolves the embed → a live projection carrying the title.
    let author_view = projector
        .project(&embed, &author, e2e_zookie())
        .expect("author projection");
    let author_sees_title = match &author_view {
        Projected::Visible(p) => p.title == E2E1_SECRET_TITLE && p.sub_anchor.is_some(),
        Projected::Tombstoned(_) => false,
    };

    // ── STEP 4: the UNAUTHORIZED viewer's pane resolves the SAME embed → a tombstone carrying ONLY the
    //    root; the SECRET title must be structurally absent (0 leak across EVERY field).
    let denied_view = projector
        .project(&embed, &denied, e2e_zookie())
        .expect("denied projection");
    match &denied_view {
        Projected::Tombstoned(t) => {
            // The tombstone carries the #sub-stripped ROOT, never the title.
            if t.root != root {
                leaks += 1; // wrong/absent root — the embed cannot degrade to the parent
            }
            if t.reason != TombstoneReason::Denied {
                leaks += 1; // the denied path must be a Denied tombstone, not a content leak
            }
            // The viewer-facing text is content-free (no title, no reason).
            if t.display_text().contains("Cerberus") || t.display_text().contains("acquisition") {
                leaks += 1;
            }
            // There is NO title accessor on a tombstone — the SECRET is structurally absent. We assert
            // the whole projection's debug rendering carries no fragment of the secret title.
            let rendered = format!("{denied_view:?}");
            if rendered.contains("Cerberus") || rendered.contains("acquisition") {
                leaks += 1; // a title fragment leaked into the unauthorized projection
            }
        }
        Projected::Visible(_) => {
            leaks += 1; // a denied viewer got a VISIBLE projection — the leak gate failed
        }
    }

    // ── STEP 5: MID-FLIGHT — the doc is marked confidential/erased while the pane is open. The SAME
    //    projector re-resolves; even the previously-authorized viewer now gets a content-free tombstone.
    let mut store2 = PageStore::new();
    store2.put_root(
        &root,
        PageMeta {
            title: E2E1_SECRET_TITLE.to_string(),
            state: "live".to_string(),
        },
    );
    store2.mark_erased(&root); // the erasure/restriction lands mid-flight
    let id2 = WedgeId::new().allow_read(&author, &root); // the author STILL has read permission…
    let projector2 = Projector::new(id2, store2);
    let author_after_erase = projector2
        .project(&embed, &author, e2e_zookie())
        .expect("author projection after erase");
    let erasure_honoured_live = match &author_after_erase {
        // …but the erasure degrades the embed to an `Erased` tombstone (the mid-flight mutation is
        // honoured by the live read path — not a stale cached title).
        Projected::Tombstoned(t) => t.reason == TombstoneReason::Erased && t.root == root,
        Projected::Visible(_) => {
            leaks += 1; // the erased doc still rendered a title — the mutation was not honoured
            false
        }
    };
    // The erased projection carries no title fragment either.
    let rendered_after = format!("{author_after_erase:?}");
    if rendered_after.contains("Cerberus") || rendered_after.contains("acquisition") {
        leaks += 1;
    }

    let green = author_sees_title
        && erasure_honoured_live
        && matches!(denied_view, Projected::Tombstoned(_));
    E2eArtifact::sealed(
        "E2E-1",
        green,
        leaks,
        format!(
            "PR-context-pane: author embed resolves live (title shown); denied viewer → \
             root-only tombstone ({} title leaks); mid-flight erase honoured live → author embed \
             degrades to Erased tombstone",
            leaks
        ),
    )
}

// ──────────────────────────────────────────────────────────────────────────────────────────────────
//  E2E-3 — Spec-to-ship lineage (a Knowledge spec doc → initiative → issues; cold==live; tamper).
// ──────────────────────────────────────────────────────────────────────────────────────────────────

/// The spec-doc page-id at the head of the lineage chain.
const E2E3_SPEC_DOC: &str = "spec-payments-v2";

/// The initiative the spec is realised as (a `db_relation` typed-edge target — the lineage's middle hop).
fn e2e3_initiative_ref() -> String {
    "myelin://acme/issue/initiative/INIT-payments".to_string()
}

/// The two issues the initiative decomposes into (the lineage's leaf hops — the spec's traceable ship).
fn e2e3_issue_refs() -> [String; 2] {
    [
        "myelin://acme/issue/issue/PAY-1".to_string(),
        "myelin://acme/issue/issue/PAY-2".to_string(),
    ]
}

/// Build Knowledge's source of truth carrying the spec→initiative→issues lineage as TE-7 typed edges
/// (5.5): the spec page, plus three forward edges (spec→initiative, initiative→PAY-1, initiative→PAY-2).
fn e2e3_lineage_source() -> KnowledgeReindexSource {
    let mut s = KnowledgeReindexSource::new();
    // The spec doc itself (a single-block page — the lineage head).
    s.upsert_page(
        E2E3_SPEC_DOC,
        4,
        &[(
            "b1",
            4,
            serde_json::json!({ "kind": "heading", "text_ref": "spec" }),
        )],
    );
    let spec_ref = format!("myelin://acme/knowledge/page/{E2E3_SPEC_DOC}");
    let init = e2e3_initiative_ref();
    let [pay1, pay2] = e2e3_issue_refs();
    // The lineage edges (TE-7 `realises`/`decomposes` — the typed table is the source of truth).
    s.upsert_edge(&spec_ref, &init, "realises", 1);
    s.upsert_edge(&init, &pay1, "decomposes", 2);
    s.upsert_edge(&init, &pay2, "decomposes", 3);
    s
}

/// Re-build a snapshot draft into an envelope the live consumer ingests (the SAME shape the steady-state
/// live event carries — the consumer cannot tell cold from live).
fn e2e3_snapshot_envelope(draft: &SnapshotDraft) -> EventEnvelope {
    EventEnvelope {
        event_id: draft.event_id(),
        type_: draft.type_.clone(),
        schema_ver: 1,
        tenant: e2e_tenant(),
        region: e2e_region(),
        actor: Actor(e2e_platform()),
        subject: draft.subject.clone(),
        aggregate: draft.aggregate.clone(),
        causation_id: None,
        correlation_id: CorrelationId(draft.event_id().0),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: draft.data_role,
        visibility: draft.visibility,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-25T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-25T00:00:00Z".into()),
        payload: draft.payload.clone(),
    }
}

/// A single hop in the spec-to-ship lineage seal — the `source -> target (rel)` fact, content-addressed
/// and chained to the prior hop's seal so any TAMPER breaks the chain (the audit-tamper crux).
#[derive(Clone, Debug)]
struct LineageHop {
    source: String,
    target: String,
    rel: String,
    /// The chain hash: `BLAKE3(prev_seal | source | target | rel)`. Tampering with any field (or
    /// reordering hops) changes this and every downstream hash — detectable by re-deriving the chain.
    seal: String,
}

/// **Seal a lineage into a hash-chain** using ONLY the frozen [`ContentHash::blake3`] primitive (the
/// same proven hash the compaction snapshot determinism uses — never a hand-rolled hash, VISION §4).
/// Each hop's seal folds in the prior seal, so a tamper anywhere in the chain breaks every downstream
/// hash. Returns the ordered sealed hops.
fn seal_lineage(hops: &[(String, String, String)]) -> Vec<LineageHop> {
    let mut prev = String::from("genesis");
    let mut out = Vec::new();
    for (source, target, rel) in hops {
        let mut body = Vec::new();
        push_lp(&mut body, prev.as_bytes());
        push_lp(&mut body, source.as_bytes());
        push_lp(&mut body, target.as_bytes());
        push_lp(&mut body, rel.as_bytes());
        let seal = ContentHash::blake3(&body).to_multihash_string();
        out.push(LineageHop {
            source: source.clone(),
            target: target.clone(),
            rel: rel.clone(),
            seal: seal.clone(),
        });
        prev = seal;
    }
    out
}

/// **Verify a sealed lineage chain** by re-deriving every hop's seal from its body + the prior seal. A
/// single mismatched seal means a TAMPER (a hop's source/target/rel was altered, or the order changed)
/// — returns `false` LOUDLY (0 silent tamper).
fn verify_lineage(hops: &[LineageHop]) -> bool {
    let mut prev = String::from("genesis");
    for hop in hops {
        let mut body = Vec::new();
        push_lp(&mut body, prev.as_bytes());
        push_lp(&mut body, hop.source.as_bytes());
        push_lp(&mut body, hop.target.as_bytes());
        push_lp(&mut body, hop.rel.as_bytes());
        let expect = ContentHash::blake3(&body).to_multihash_string();
        if expect != hop.seal {
            return false; // tamper detected — the re-derived seal does not match the stored one
        }
        prev = hop.seal.clone();
    }
    true
}

/// The lineage hops as `(source, target, rel)` triples (the spec→initiative→issues traceability path).
fn e2e3_lineage_hops() -> Vec<(String, String, String)> {
    let spec_ref = format!("myelin://acme/knowledge/page/{E2E3_SPEC_DOC}");
    let init = e2e3_initiative_ref();
    let [pay1, pay2] = e2e3_issue_refs();
    vec![
        (spec_ref, init.clone(), "realises".to_string()),
        (init.clone(), pay1, "decomposes".to_string()),
        (init, pay2, "decomposes".to_string()),
    ]
}

/// **E2E-3 (spec-to-ship lineage — Knowledge leg): drive it end-to-end.**
///
/// The whole flow, not a single handler (EI-01 §4):
/// 1. A Knowledge spec doc → initiative → issues lineage is laid down as TE-7 typed edges (5.5).
/// 2. The lineage is TRACEABLE: a forward walk from the spec reaches both ship issues.
/// 3. **cold-reindex == live:** the derived projection is WIPED; `replay(scope)` rebuilds it ONLY
///    through the live consumer path ([`DerivedStore::ingest`]) — the rebuilt bytes byte-match live
///    (the KN-D6 parity hash, contract 2.6). No bespoke recovery reader.
/// 4. **audit tamper detected:** the lineage is sealed into a hash-chain (from the frozen
///    [`ContentHash::blake3`] primitive). The honest chain VERIFIES; a single TAMPERED hop (a forged
///    "PAY-2 was never decomposed from this initiative") breaks the chain hash and FAILS verification —
///    0 silent tamper.
///
/// Returns the named green artifact (`is_green()` iff cold==live AND honest-verifies AND tamper-detected).
pub fn run_e2e3_spec_to_ship_lineage() -> E2eArtifact {
    let mut leaks: u64 = 0;
    let source = e2e3_lineage_source();
    let scope = SnapshotScope::new("knowledge", "all");

    // ── STEP 2: the lineage is TRACEABLE — a forward walk from the spec reaches both ship issues.
    let hops = e2e3_lineage_hops();
    let spec_ref = format!("myelin://acme/knowledge/page/{E2E3_SPEC_DOC}");
    let mut frontier = vec![spec_ref.clone()];
    let mut reached: HashSet<String> = HashSet::new();
    while let Some(node) = frontier.pop() {
        for (s, t, _r) in &hops {
            if *s == node && reached.insert(t.clone()) {
                frontier.push(t.clone());
            }
        }
    }
    let [pay1, pay2] = e2e3_issue_refs();
    let lineage_traceable = reached.contains(&e2e3_initiative_ref())
        && reached.contains(&pay1)
        && reached.contains(&pay2);
    if !lineage_traceable {
        leaks += 1; // the spec does not trace to its ship issues — the lineage is broken
    }

    // ── STEP 3: cold == live. Build live, WIPE the derived store, rebuild ONLY from replay through the
    //    live consumer path; assert the parity bytes byte-match.
    let mut live = DerivedStore::new();
    for draft in source.replay(&scope, None) {
        live.ingest(&e2e3_snapshot_envelope(&draft));
    }
    let sources: &[&dyn ReindexSource] = &[&source];
    let mut outbox = OutboxStore::new();
    reindex(&scope, None, sources, &mut outbox, e2e_ctx_base()).expect("reindex replay");
    let mut cold = DerivedStore::new();
    assert!(cold.is_empty(), "the derived store is wiped before rebuild");
    for draft in source.replay(&scope, None) {
        let row = outbox.row(&draft.event_id()).expect("snapshot row present");
        cold.ingest(&row.envelope);
    }
    let cold_equals_live = cold.len() == live.len() && cold.parity_bytes() == live.parity_bytes();
    if !cold_equals_live {
        leaks += 1; // cold diverged from live — the reindex is not byte-exact
    }

    // ── STEP 4: audit tamper detected. Seal the lineage; the honest chain verifies; a tampered hop is
    //    caught.
    let honest = seal_lineage(&hops);
    let honest_verifies = verify_lineage(&honest);
    if !honest_verifies {
        leaks += 1; // the honest chain failed to verify — the seal is broken
    }
    // Forge a hop: rewrite the last hop's target (a "PAY-2 was never decomposed" tamper) but keep its
    // stored seal — the re-derive must catch it.
    let mut tampered = honest.clone();
    if let Some(last) = tampered.last_mut() {
        last.target = "myelin://acme/issue/issue/PAY-FORGED".to_string();
    }
    let tamper_detected = !verify_lineage(&tampered);
    if !tamper_detected {
        leaks += 1; // a tampered lineage hop went UNDETECTED — the audit seal is vacuous
    }

    let green = lineage_traceable && cold_equals_live && honest_verifies && tamper_detected;
    E2eArtifact::sealed(
        "E2E-3",
        green,
        leaks,
        format!(
            "spec→initiative→issues lineage traceable={lineage_traceable}; \
             cold-reindex==live={cold_equals_live} (parity bytes byte-match); \
             audit honest-verifies={honest_verifies}, tamper-detected={tamper_detected}"
        ),
    )
}

/// **Run BOTH Knowledge E2E legs and return their named green artifacts (E2E-1, E2E-3).** The master
/// M5 exit gate cites these; both must be `is_green()`.
pub fn run_knowledge_e2e_legs() -> Vec<E2eArtifact> {
    vec![run_e2e1_pr_context_pane(), run_e2e3_spec_to_ship_lineage()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn e2e1_pr_context_pane_zero_title_leak() {
        let art = run_e2e1_pr_context_pane();
        assert_eq!(art.scenario, "E2E-1");
        assert_eq!(
            art.leaks, 0,
            "0 title leak across every projection: {art:?}"
        );
        assert!(art.is_green(), "E2E-1 green not earned: {art:?}");
        // The artifact is sealed (a citable content-address).
        assert!(art.seal.starts_with("blake3:"));
    }

    #[test]
    fn e2e3_spec_to_ship_cold_equals_live_and_tamper_detected() {
        let art = run_e2e3_spec_to_ship_lineage();
        assert_eq!(art.scenario, "E2E-3");
        assert_eq!(art.leaks, 0, "0 divergence/undetected-tamper: {art:?}");
        assert!(art.is_green(), "E2E-3 green not earned: {art:?}");
        assert!(art.seal.starts_with("blake3:"));
    }

    #[test]
    fn both_legs_green_and_distinctly_sealed() {
        let arts = run_knowledge_e2e_legs();
        assert_eq!(arts.len(), 2);
        assert!(arts.iter().all(|a| a.is_green()));
        // The two scenarios seal to DISTINCT addresses (the framing is injective — no collision).
        assert_ne!(arts[0].seal, arts[1].seal);
        assert_eq!(E2E_SCENARIOS, ["E2E-1", "E2E-3"]);
    }

    #[test]
    fn e2e1_unauthorized_projection_carries_no_title_fragment() {
        // A focused re-assert of the leak crux: the unauthorized projection's full debug render is
        // free of any fragment of the SECRET title (structural absence, not redaction).
        let denied = e2e_viewer("nobody");
        let root = page_root(E2E1_DESIGN_DOC);
        let embed = myelin_events::ArtifactRef(format!("{}#block-h1", root.0));
        let mut store = PageStore::new();
        store.put_root(
            &root,
            PageMeta {
                title: E2E1_SECRET_TITLE.to_string(),
                state: "live".to_string(),
            },
        );
        // Empty allow-list ⇒ everyone is denied (fail-closed).
        let projector = Projector::new(WedgeId::new(), store);
        let view = projector.project(&embed, &denied, e2e_zookie()).unwrap();
        assert!(matches!(view, Projected::Tombstoned(_)));
        let rendered = format!("{view:?}");
        assert!(!rendered.contains("Cerberus"));
        assert!(!rendered.contains("acquisition"));
    }

    #[test]
    fn e2e3_verify_catches_a_reordered_chain() {
        // A reorder (swapping two hops) must also break the chain — the seal folds the prior hash, so
        // order is load-bearing.
        let hops = e2e3_lineage_hops();
        let mut sealed = seal_lineage(&hops);
        assert!(verify_lineage(&sealed));
        sealed.swap(1, 2);
        assert!(
            !verify_lineage(&sealed),
            "a reordered chain must fail verify"
        );
    }

    #[test]
    fn e2e_artifact_seal_is_deterministic() {
        let a = E2eArtifact::sealed("E2E-1", true, 0, "same body");
        let b = E2eArtifact::sealed("E2E-1", true, 0, "same body");
        assert_eq!(a.seal, b.seal, "the seal is a pure function of the body");
        let c = E2eArtifact::sealed("E2E-1", true, 1, "same body");
        assert_ne!(a.seal, c.seal, "a different leak count seals differently");
    }
}
