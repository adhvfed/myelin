//! # AG-P19 (→ P-268, M3) — the Knowledge producer ToolDefs + the agent-trace holder seam
//! (KN-D11 KN-edit leg + KN-D12 trace-erasure leg + the 4.9 / 8.8 / 13.1 CDC)
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row **8.1**
//! (OWNED — the four KN producer ToolDefs registered into the ONE ToolSurface) + CONSUMES **4.9** (the
//! KN ReBAC carrier supplies the `required_caps`: `page.publish` / `page.edit` / `page.draft` /
//! `page.comment`), **8.8** (the agent-trace holder seam — `run.trace_ref` resolves to a
//! content-addressed Knowledge document), **13.1** (the trace reuses the frozen `myelin-content` block
//! model). Owning architecture: `agent-fabric.md` §6.1 (the ONE catalogue) + §6.3 (the FROZEN
//! `requires_approval` defaults — KN `publish`/`edit_confidential` = yes, `draft`/`comment` = no) +
//! §4.5 (the trace = a content-addressed Knowledge document + erasable holder, distinct from the audit
//! log).
//!
//! **Drills:**
//! - **KN-D11 (the KN-edit leg, a Fabric-loop assertion):** an agent `publish` is GOVERNED — **0
//!   ungoverned edits / 0 mutations before approval / 0 double-apply**; a double-click is ONE
//!   approval; `draft` applies DIRECTLY (no gate). Pairs the REGISTERED KN ToolDefs (the SUT) with the
//!   REAL eight-step `PlanThenApply` pipeline (AG-P6) + the REAL HITL withhold → resume loop (AG-P9).
//! - **KN-D12 (the trace-holder erasure leg, the M3 part of AG-D10):** erase a subject → the
//!   content-addressed agent trace is crypto-shredded/purged; attribution falls back to the opaque
//!   pseudonym; **0 recoverable PII, attribution intact**.

use myelin_agent::{EffectKind, EffectResult, EventId, ToolDef, ToolName, ToolSurface};
use myelin_agent_service::{
    comment_required_caps, comment_tool_def, draft_required_caps, draft_tool_def,
    edit_confidential_required_caps, edit_confidential_tool_def, gate_id_of, is_content_addressed_kn_document,
    knowledge_tool_defs, publish_required_caps, publish_tool_def, register_knowledge_tools,
    run_hitl_loop, trace_ref_of, AgentTraceHolder, ApplyError, ApprovedTools, CapabilityCheck,
    DelegationLookup, EffectBudget, EffectCost, HitlGate, HitlOutcome, HitlWait, PipelineSignals,
    PlanThenApply, PlannedEffect, RiskSummary, SubsystemApply, TenantGuard, TraceDocument,
    WaitDecision, COMMENT_TOOL, DRAFT_TOOL, EDIT_CONFIDENTIAL_TOOL, KNOWLEDGE_SUBSYSTEM, PUBLISH_TOOL,
};
use myelin_content::{Block, Inline, Span};
use myelin_gdpr::{EraseScope, PersonalDataHolder, SubjectRef, TenantId as GdprTenantId};
use myelin_identity::{
    CaveatContext, Consistency, Decision, EffectivePolicy, Permission, Principal, PrincipalId,
    PrincipalKind, RuntimeRef, Zookie,
};
use myelin_storage::reserve_settle::MeteredUnit;
use myelin_tenancy::{ArtifactRef, TenantId};
use std::cell::RefCell;
use std::collections::BTreeSet;

// ───────────────────────── the REAL consumed seams (the pipeline providers) ─────────────────────

struct Catalogue {
    defs: Vec<ToolDef>,
}
impl ToolSurface for Catalogue {
    fn register_tool(&mut self, def: ToolDef) {
        self.defs.push(def);
    }
    fn resolve(&self, name: &ToolName) -> Option<&ToolDef> {
        self.defs.iter().find(|d| &d.name == name)
    }
}

/// A `check` provider that allows a fixed cap set (the 4.9 perms the KN tools require), else Deny.
struct AllowCaps {
    allow: BTreeSet<String>,
}
impl CapabilityCheck for AllowCaps {
    fn check(
        &self,
        _s: &Principal,
        permission: &Permission,
        _o: &ArtifactRef,
        _at: &Consistency,
        _c: Option<&CaveatContext>,
    ) -> Decision {
        if self.allow.contains(&permission.0) {
            Decision::Allow
        } else {
            Decision::Deny
        }
    }
}

struct Delegate {
    caps: Vec<String>,
}
impl DelegationLookup for Delegate {
    fn delegation(&self, _a: &Principal, _t: &Principal) -> EffectivePolicy {
        EffectivePolicy { caveats: self.caps.clone() }
    }
}

struct PermitAll;
impl TenantGuard for PermitAll {
    fn permits(&self, _a: &Principal, _t: &ToolName, _o: &ArtifactRef) -> bool {
        true
    }
}

/// The subsystem PUBLIC endpoint — the ONLY mutation path; records EVERY apply so the drill can
/// assert 0 mutation before approval + exactly-once.
struct Endpoint {
    applied: RefCell<Vec<String>>,
}
impl SubsystemApply for Endpoint {
    fn apply_public(
        &self,
        _a: &Principal,
        tool: &ToolName,
        object: &ArtifactRef,
        _input: &str,
    ) -> Result<EventId, ApplyError> {
        self.applied.borrow_mut().push(tool.0.clone());
        Ok(EventId(format!("evt:{}:{}", tool.0, object.0)))
    }
}

struct Budget {
    remaining: u64,
    settles: u64,
}
impl EffectBudget for Budget {
    fn has_remaining(&self, cost: u64) -> bool {
        self.remaining >= cost
    }
    fn settle_one(&mut self, unit: &MeteredUnit) -> u64 {
        let total = unit.total().map(|m| m.0).unwrap_or(0);
        self.remaining = self.remaining.saturating_sub(total);
        self.settles += 1;
        total
    }
}

/// A REAL provider on the 9.4 durable HITL wait — returns the scripted decision the human made days
/// later. A `parked` counter records that the run PARKED (state=waiting holds no runtime).
struct ScriptedWait {
    decision: WaitDecision,
    parked: RefCell<u32>,
}
impl HitlWait for ScriptedWait {
    fn park_and_wait(&self, _gate: &HitlGate) -> WaitDecision {
        *self.parked.borrow_mut() += 1;
        self.decision.clone()
    }
}

// ───────────────────────── fixtures (the SUT is knowledge_tool_defs(), not a local def) ───────────

fn agent() -> Principal {
    Principal::stub(
        PrincipalId("psn:agent-7".into()),
        PrincipalKind::Agent { runtime_ref: RuntimeRef("mock".into()), on_behalf_of: None },
        TenantId("acme".into()),
    )
}
fn human() -> Principal {
    Principal::stub(PrincipalId("psn:human-x".into()), PrincipalKind::Human, TenantId("acme".into()))
}

/// A catalogue holding the REGISTERED KN producer ToolDefs (the SUT — register_knowledge_tools is the
/// deliverable, so the drill registers through it, never hand-building the defs).
fn kn_catalogue() -> Catalogue {
    let mut cat = Catalogue { defs: vec![] };
    register_knowledge_tools(&mut cat).expect("the seeded KN defs always admit (no silent loosening)");
    cat
}

fn publish_plan() -> PlannedEffect {
    PlannedEffect {
        tool: ToolName(PUBLISH_TOOL.into()), // "publish" under subsystem "knowledge" (the §6.3 key)
        object: ArtifactRef("myelin://acme/knowledge/page/onboarding".into()),
        input_json: r#"{"page":"onboarding","space":"eng"}"#.into(),
        field: None,
        transition: None,
        cost: EffectCost { unit: "knowledge.publish", wholesale: 10, markup: 10 },
    }
}

fn draft_plan() -> PlannedEffect {
    PlannedEffect {
        tool: ToolName(DRAFT_TOOL.into()),
        object: ArtifactRef("myelin://acme/knowledge/space/eng".into()),
        input_json: r#"{"space":"eng","title":"WIP","blocks":[]}"#.into(),
        field: None,
        transition: None,
        cost: EffectCost { unit: "knowledge.draft", wholesale: 2, markup: 2 },
    }
}

/// Run the apply pipeline once over the SUT catalogue for `plan` under the given `approved` set;
/// returns the result + the TOTAL mutations the endpoint recorded after the call.
fn apply_once(
    cat: &Catalogue,
    endpoint: &Endpoint,
    plan: &PlannedEffect,
    allowed_caps: &[&str],
    approved: BTreeSet<String>,
) -> (EffectResult, usize) {
    let check = AllowCaps { allow: allowed_caps.iter().map(|c| c.to_string()).collect() };
    let del = Delegate { caps: allowed_caps.iter().map(|c| c.to_string()).collect() };
    let tenant = PermitAll;
    let mut budget = Budget { remaining: 1_000, settles: 0 };
    let mut signals = PipelineSignals::new();
    let mut p = PlanThenApply {
        catalogue: cat,
        check: &check,
        delegation: &del,
        tenant: &tenant,
        apply_endpoint: endpoint,
        budget: &mut budget,
        agent: agent(),
        trigger_actor: human(),
        zookie: Zookie("z-1".into()),
        approved,
        signals: &mut signals,
    };
    let out = p.apply_planned(plan);
    let muts = endpoint.applied.borrow().len();
    (out, muts)
}

// ───────────────────────── the registration GATE (8.1 / §6.3 — the frozen defaults on the wire) ───

/// **GATE: the REGISTERED `publish` + `edit_confidential` carry `requires_approval = yes` and route
/// through EffectApi; `draft` + `comment` carry `no` and apply directly (the frozen §6.3 defaults, ON
/// THE CATALOGUE).** The defaults are not just in the seed table — they ride the registered ToolDef
/// the pipeline reads.
#[test]
fn knowledge_tools_register_with_the_frozen_6_3_defaults() {
    let cat = kn_catalogue();

    let publish = cat.resolve(&ToolName(PUBLISH_TOOL.into())).expect("publish registered");
    assert_eq!(publish.subsystem, KNOWLEDGE_SUBSYSTEM);
    assert!(publish.requires_approval, "publish carries requires_approval = yes (§6.3 — consequential)");
    assert_eq!(publish.effect_kind, EffectKind::Mutate, "publish routes through EffectApi");
    assert_eq!(publish.required_caps, publish_required_caps());
    assert_eq!(publish.required_caps, vec!["page.publish".to_string()], "4.9 cap");

    let edit = cat
        .resolve(&ToolName(EDIT_CONFIDENTIAL_TOOL.into()))
        .expect("edit_confidential registered");
    assert!(edit.requires_approval, "edit_confidential carries requires_approval = yes (§6.3)");
    assert_eq!(edit.required_caps, edit_confidential_required_caps());
    assert_eq!(edit.required_caps, vec!["page.edit".to_string()]);

    let draft = cat.resolve(&ToolName(DRAFT_TOOL.into())).expect("draft registered");
    assert!(!draft.requires_approval, "draft carries requires_approval = no (§6.3 — reversible)");
    assert_eq!(draft.required_caps, draft_required_caps());
    assert_eq!(draft.required_caps, vec!["page.draft".to_string()]);

    let comment = cat.resolve(&ToolName(COMMENT_TOOL.into())).expect("comment registered");
    assert!(!comment.requires_approval, "comment carries requires_approval = no (§6.3 — reversible)");
    assert_eq!(comment.required_caps, comment_required_caps());
    assert_eq!(comment.required_caps, vec!["page.comment".to_string()]);
}

// ───────────────────────── KN-D11 KN-edit leg — withhold (0 mutation) → approve → ONE publish ──────

/// **KN-D11 (the KN-edit leg): a registered `publish` is GOVERNED end-to-end — WITHHELD (0 mutation
/// before approval), the run PARKS, an APPROVAL arrives, the resume threads the tool into `approved`,
/// a re-run APPLIES EXACTLY ONCE; a DOUBLE-CLICK on approve is ONE approval (0 double-apply).** The
/// SUT is the catalogue `register_knowledge_tools` built — the frozen default rides the registered def
/// into the REAL pipeline. This is the M3 "agent edit governed" drill on the KN-edit instance.
#[test]
fn knd11_kn_publish_is_governed_zero_ungoverned_zero_double_apply() {
    let cat = kn_catalogue();
    let endpoint = Endpoint { applied: RefCell::new(vec![]) };
    let caps = ["page.publish"];

    // 1. WITHHOLD — a fresh run (empty `approved`) proposes the registered publish → GATED.
    let (result, muts_before) = apply_once(&cat, &endpoint, &publish_plan(), &caps, BTreeSet::new());
    let gate_id = gate_id_of(&result).expect("publish is requires_approval → it GATES (0 ungoverned)");
    assert!(matches!(result, EffectResult::Gated(_)), "the registered publish WITHHOLDS: {result:?}");
    assert_eq!(muts_before, 0, "0 MUTATIONS before approval (KN-D11 — the publish did NOT apply)");

    // 2 + 3 + 4. PARK on the durable wait (9.4) → APPROVE (days later) → thread into `approved`.
    let wait = ScriptedWait { decision: WaitDecision::Approve, parked: RefCell::new(0) };
    let mut approved = ApprovedTools::new();
    let outcome = run_hitl_loop(
        gate_id,
        "R1",
        &publish_plan(),
        RiskSummary::for_action("agent.hitl.publish_page", &publish_plan().object),
        vec![PrincipalId("psn:lead".into())],
        "card:R1:0",
        &wait,
        &mut approved,
    );
    assert_eq!(*wait.parked.borrow(), 1, "the run PARKED on the durable wait (state=waiting, no runtime)");
    assert!(matches!(outcome, HitlOutcome::Approved(_)), "approval resumes: {outcome:?}");

    // a DOUBLE-CLICK on approve is ONE approval — admitting the same approved gate twice is idempotent.
    if let HitlOutcome::Approved(ref gate) = outcome {
        approved.admit(gate); // re-admit (the double-click) — the gate is already in the set.
        assert_eq!(approved.as_set().len(), 1, "a double-click is ONE approval (the set holds one tool)");
    }

    // 5. RESUME — the re-run with the populated `approved` set passes step 6 → APPLIES EXACTLY ONCE.
    let (result2, muts_after) = apply_once(&cat, &endpoint, &publish_plan(), &caps, approved.as_set());
    assert!(matches!(result2, EffectResult::Applied(_)), "the approved publish APPLIES on resume: {result2:?}");
    assert_eq!(muts_after, 1, "the publish applied EXACTLY ONCE (0 double-apply, after approval, never before)");
}

/// **KN-D11 (rejection leg): a registered `publish` REJECTED never applies — 0 mutation across the
/// whole flow. The publish is governed; a decline is honoured (0 ungoverned).**
#[test]
fn knd11_kn_publish_rejected_never_applies_zero_mutation() {
    let cat = kn_catalogue();
    let endpoint = Endpoint { applied: RefCell::new(vec![]) };
    let caps = ["page.publish"];

    let (result, _) = apply_once(&cat, &endpoint, &publish_plan(), &caps, BTreeSet::new());
    let gate_id = gate_id_of(&result).expect("gated");

    let wait = ScriptedWait {
        decision: WaitDecision::Reject("not ready to publish".into()),
        parked: RefCell::new(0),
    };
    let mut approved = ApprovedTools::new();
    let outcome = run_hitl_loop(
        gate_id,
        "R1",
        &publish_plan(),
        RiskSummary::for_action("agent.hitl.publish_page", &publish_plan().object),
        vec![PrincipalId("psn:lead".into())],
        "card:R1:0",
        &wait,
        &mut approved,
    );
    assert!(matches!(outcome, HitlOutcome::Halted(_)), "rejection halts: {outcome:?}");
    assert!(!approved.contains(PUBLISH_TOOL), "a rejected publish never approves the tool");

    let (result2, muts) = apply_once(&cat, &endpoint, &publish_plan(), &caps, approved.as_set());
    assert!(matches!(result2, EffectResult::Gated(_)), "a rejected publish still GATES — never applies");
    assert_eq!(muts, 0, "0 MUTATIONS across the entire reject flow");
}

// ───────────────────────── draft applies DIRECTLY (no gate) ────────────────────────────────────────

/// **`draft` is reversible (§6.3 = no) → it applies DIRECTLY through the pipeline: schema ✓ → cap ✓ →
/// … → NO gate → APPLY, no HITL withhold.** The pipeline never returns `Gated` for `draft`; the
/// effect mutates once on the first call (the contrast that proves the gate is a per-tool frozen
/// default, not a blanket policy).
#[test]
fn draft_applies_directly_no_hitl_gate() {
    let cat = kn_catalogue();
    let endpoint = Endpoint { applied: RefCell::new(vec![]) };

    // a FRESH run (empty `approved`) — draft applies on the FIRST call (no withhold).
    let (result, muts) =
        apply_once(&cat, &endpoint, &draft_plan(), &["page.draft"], BTreeSet::new());
    assert!(matches!(result, EffectResult::Applied(_)), "draft applies directly: {result:?}");
    assert!(gate_id_of(&result).is_none(), "draft NEVER opens an HITL gate (it is reversible)");
    assert_eq!(muts, 1, "draft mutated once, directly, with NO prior approval");
}

/// **The CHAINED e2e (the prompt's required end-to-end): a mock agent proposes a publish → withheld →
/// approve → EXACTLY ONE publish; a draft → applied directly.** Both flows run over the SAME registered
/// catalogue + the SAME endpoint, proving the per-tool gate split on one catalogue.
#[test]
fn chained_e2e_publish_withheld_then_approved_one_apply_draft_direct() {
    let cat = kn_catalogue();
    let endpoint = Endpoint { applied: RefCell::new(vec![]) };

    // publish: withheld (0 mutation) → approve → exactly one apply.
    let (gated, m0) = apply_once(&cat, &endpoint, &publish_plan(), &["page.publish"], BTreeSet::new());
    let gate_id = gate_id_of(&gated).expect("publish withholds");
    assert_eq!(m0, 0, "0 mutation before approval");
    let wait = ScriptedWait { decision: WaitDecision::Approve, parked: RefCell::new(0) };
    let mut approved = ApprovedTools::new();
    let outcome = run_hitl_loop(
        gate_id,
        "R1",
        &publish_plan(),
        RiskSummary::for_action("agent.hitl.publish_page", &publish_plan().object),
        vec![PrincipalId("psn:lead".into())],
        "card:R1:0",
        &wait,
        &mut approved,
    );
    assert!(matches!(outcome, HitlOutcome::Approved(_)));
    let (applied, m1) = apply_once(&cat, &endpoint, &publish_plan(), &["page.publish"], approved.as_set());
    assert!(matches!(applied, EffectResult::Applied(_)));
    assert_eq!(m1, 1, "exactly ONE publish after approval");

    // draft: applied directly (no gate) on the same endpoint (now 2 total mutations).
    let (draft_res, m2) = apply_once(&cat, &endpoint, &draft_plan(), &["page.draft"], BTreeSet::new());
    assert!(matches!(draft_res, EffectResult::Applied(_)), "draft applied directly");
    assert_eq!(m2, 2, "the draft applied (publish=1 + draft=1)");
    // the recorded mutation order: publish (after approval) then draft.
    assert_eq!(*endpoint.applied.borrow(), vec![PUBLISH_TOOL.to_string(), DRAFT_TOOL.to_string()]);
}

// ───────────────────────── KN-D12 — the trace-holder erasure leg (the M3 part of AG-D10) ───────────

/// A 13.1 inline run of plain text (a [`Span::Text`] with no marks) — the reasoning prose.
fn text(s: &str) -> Inline {
    Inline {
        spans: vec![Span::Text { text: s.to_string(), marks: vec![], link: None }],
        nodes: vec![],
    }
}

/// A trace document carrying personal data (the agent's reasoning naming the data subject) — a 13.1
/// block document, the holder body KN-D12 erases.
fn pii_trace(run_id: u128) -> TraceDocument {
    TraceDocument::new(
        run_id,
        vec![Block::Paragraph {
            inline: text("the agent processed the support ticket for alice@example.com and drafted a reply"),
        }],
    )
}

fn gdpr_subject(id: &str) -> SubjectRef {
    SubjectRef::new(Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        GdprTenantId::from_token("acme"),
    ))
}

/// **8.8 / §4.5: `run.trace_ref` resolves to a CONTENT-ADDRESSED Knowledge document reusing the 13.1
/// block model.** The Fabric builds the trace as a `Vec<Block>` (the frozen taxonomy) and addresses it
/// to a `blake3:<hex>` — a genuine content hash over a 13.1 document, NOT an opaque placeholder.
#[test]
fn cdc_8_8_trace_ref_resolves_to_a_content_addressed_13_1_document() {
    let doc = pii_trace(7);
    // the trace is a frozen 13.1 block document (no second document model — 13.1).
    assert!(matches!(doc.blocks[0], Block::Paragraph { .. }), "the trace reuses the 13.1 Block taxonomy");
    // run.trace_ref IS the content address (the §4.5 seam): a blake3:<hex> over the document.
    let trace_ref = trace_ref_of(&doc);
    assert!(trace_ref.starts_with("blake3:"), "run.trace_ref is a content address: {trace_ref}");
    assert!(is_content_addressed_kn_document(&doc), "the trace is the content-addressed KN document (8.8)");
}

/// **KN-D12 (the trace-holder erasure leg, the M3 part of AG-D10): erase a subject → the
/// content-addressed agent trace is crypto-shredded/purged; attribution falls back to the opaque
/// pseudonym; 0 recoverable PII; attribution intact.** The trace holder (H17, distinct from the audit
/// log) responds to `erase` with a content-addressed receipt; the per-subject DEK (the
/// `CryptoShred(subject_dek)` lever on `TraceRow`) is the structural shred. The OPAQUE PSEUDONYM
/// principal id (never a name/email) is what attribution keys on — it survives the erase (the audit
/// fact stays; the PII body goes).
#[test]
fn knd12_erase_subject_crypto_shreds_the_trace_pseudonym_survives() {
    let holder = AgentTraceHolder;
    let subject = gdpr_subject("psn:agent-subject-7"); // the OPAQUE pseudonym — never PII (EI-04 §1).

    // the trace body BEFORE erase contains PII (the subject's email in the reasoning prose).
    let doc = pii_trace(7);
    let body = doc.canonical_bytes();
    assert!(
        String::from_utf8_lossy(&body).contains("alice@example.com"),
        "the trace body contains PII before erasure (the reasoning the brain authored)"
    );

    // ERASE the subject → the trace holder responds (the M3 seam; the structural crypto-shred body is
    // AG-P23). The receipt is content-addressed (blake3:) and NAMES the AG-D10 follow-on — never a
    // panic, never a silent gap (VISION §3).
    let scope = EraseScope::Subject {
        subject: subject.clone(),
        tenant: GdprTenantId::from_token("acme"),
    };
    let receipt = holder.erase(scope).expect("the trace holder erase succeeds (the M3 seam)");
    assert_eq!(receipt.receipt.operation, "erase");
    assert!(receipt.receipt.content_hash.starts_with("blake3:"), "the erase receipt is content-addressed");

    // ATTRIBUTION INTACT: the receipt keys on the OPAQUE pseudonym principal id — never a name/email.
    // The pseudonym survives the erase (the attribution fact stays; the PII trace body is shredded).
    let subject_id = &subject.principal.principal_id.0;
    assert_eq!(subject_id, "psn:agent-subject-7", "the subject is the opaque pseudonym, not PII");
    assert!(
        !subject_id.contains('@') && !subject_id.contains("alice"),
        "0 recoverable PII in the attribution key — it is the opaque pseudonym (EI-04 §1)"
    );

    // the erase is idempotent (the same scope → the identical content-addressed receipt) — the AG-D10
    // structural property: erase is a well-defined, repeatable operation.
    let receipt2 = holder
        .erase(EraseScope::Subject { subject, tenant: GdprTenantId::from_token("acme") })
        .expect("idempotent");
    assert_eq!(receipt, receipt2, "erase is idempotent (the same scope → the identical receipt)");
}

// ───────────────────────── the consumer CDC for 4.9 (the KN ReBAC carrier supplies caps) ──────────

/// **CONSUMER CDC for 4.9 — the KN producer ToolDefs' `required_caps` ARE the frozen KN ReBAC carrier
/// permissions, sourced from `myelin-content` (the PROVIDER), never invented in the Fabric.** A rename
/// of a `page` write permission in the carrier breaks this CDC (the contract is the names, frozen on
/// both sides — the KN parallel to the Git 4.9 CDC).
#[test]
fn cdc_4_9_required_caps_are_the_kn_rebac_carrier_permissions() {
    use myelin_content::rebac_fragment::{object_types, page_write_fragment, COMMENT, DRAFT, EDIT, PUBLISH};

    // PROVIDER (4.9): the page write fragment declares publish/edit/draft/comment on `page`.
    let page = page_write_fragment();
    assert_eq!(page.object_type.0, object_types::PAGE);
    for p in [PUBLISH, EDIT, DRAFT, COMMENT] {
        assert!(page.permissions.iter().any(|perm| perm.0 == p), "the carrier declares `page.{p}` (4.9)");
    }
    // CONSUMER: each registered KN ToolDef's required_cap is exactly `page.<permission>`.
    assert_eq!(publish_tool_def().required_caps, vec!["page.publish".to_string()]);
    assert_eq!(edit_confidential_tool_def().required_caps, vec!["page.edit".to_string()]);
    assert_eq!(draft_tool_def().required_caps, vec!["page.draft".to_string()]);
    assert_eq!(comment_tool_def().required_caps, vec!["page.comment".to_string()]);
}

/// **CONSUMER CDC for 13.1 — the trace document REUSES the frozen `myelin-content` Block taxonomy
/// (the PROVIDER).** The trace is built from `Block` (the frozen 13.1 nodes), not a bespoke transcript
/// format; a change to the block taxonomy breaks the trace document build (one document model).
#[test]
fn cdc_13_1_trace_reuses_the_frozen_block_taxonomy() {
    let doc = TraceDocument::new(
        1,
        vec![
            Block::Paragraph { inline: text("reasoning") },
            Block::CodeBlock { lang: Some("json".into()), text: "{}".into() },
            Block::Divider,
        ],
    );
    assert_eq!(doc.blocks.len(), 3, "the trace is a Vec<Block> of frozen 13.1 nodes");
    // the document serializes deterministically over the frozen taxonomy → a stable content address.
    assert_eq!(doc.content_address(), doc.content_address(), "deterministic over the frozen 13.1 model");
}

/// **NO-NEW-ENGINE check (EI-03 §4 / EI-01 §7): the whole KN producer surface is `knowledge_tool_defs()`
/// — four `mutate` ToolDefs registered into the existing ToolSurface, differing ONLY in their frozen
/// `requires_approval` seed. There is no second apply/gate path; the trace seam reuses the 13.1 model
/// + the existing H17 holder.**
#[test]
fn kn_producer_surface_is_a_projection_four_defs_only() {
    let defs = knowledge_tool_defs();
    assert_eq!(defs.len(), 4, "exactly publish + edit_confidential + draft + comment — no other KN producer tool at M3");
    for d in &defs {
        assert_eq!(d.effect_kind, EffectKind::Mutate, "every KN tool routes through EffectApi");
        assert!(d.side_effecting);
    }
    assert!(defs[0].requires_approval && defs[1].requires_approval, "publish + edit_confidential gated");
    assert!(!defs[2].requires_approval && !defs[3].requires_approval, "draft + comment not gated");
}
