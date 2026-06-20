//! The Agent-Fabric OLTP row types — the five tables of the data model (architecture §4.1..§4.5),
//! `(tenant, region)`-first and carrying the `#[personal_data(...)]` classification tags
//! (contract 10.2) on every PII-bearing column.
//!
//! **These are the frozen-shape tag-carriers + the column lists the migrations build.** The exact
//! field lists are §4.1 (`run`), §4.2 (`tool_def`), §4.3 (`proposed_effect`), §4.4 (`hitl_gate`),
//! §4.5 (`trace`). Every row LEADS with `(tenant, region)` (12.1, ADR-11) — the partition key from
//! the verified token, never the path. The PII-bearing free-text columns (the humanised
//! `risk_summary`, the per-run conversation/trace the run owns) are tagged Content / `CryptoShred`
//! under the **per-subject DEK** (contract 11.4) so the crypto-shred erase reaches them live + in
//! backups; the actor-identity columns (`agent_principal`, `on_behalf_of`) are tagged Identifier /
//! `Pseudonymise` (the actor is an opaque pseudonym resolved through Identity, contract 4.8 — never a
//! raw name/email).
//!
//! The attribute uses the canonical **multi-line six-tag** form frozen in P-GA-02 / P-050 + gdpr
//! §2.1 (`category | role | basis | retention | erasure | subject_locator`). The M0/M1 derive is a
//! no-op (the tag is the classification FACT a store applies today; the registry-emitting body is the
//! P-GA-07 floor; the holder REGISTRATION over these stores is AG-P3 → P-132).

use myelin_agent::EffectKind;
use myelin_gdpr::PersonalData;
use myelin_tenancy::{Region, TenantId};

/// The `run` row (architecture §4.1) — the unit of agent execution, a durable-workflow instance
/// (ADR-09). `(tenant, region)`-first. A run may pause for *days* on a HITL gate holding no thread.
///
/// Field list frozen by §4.1: `run_id`, `agent_principal`, `on_behalf_of`, `binding_id`,
/// `trigger_event`, `correlation_id`/`causation_id`/`depth`, `runtime_ref` (the strategy swap),
/// `state`, `reservation_id`, `budget` (integer minor-units, never floats), `trace_ref`.
#[derive(PersonalData)]
pub struct Run {
    /// `(tenant, region)` partition key — opaque routing keys, no tag (architecture §4 preamble).
    pub tenant: TenantId,
    /// `(tenant, region)` partition key — opaque routing keys, no tag.
    pub region: Region,
    /// the stable opaque run id (the durable-workflow instance id) — not personal data.
    pub run_id: u128,
    /// the AGENT principal the run executes as (`kind=agent`) — an OPAQUE pseudonym resolved through
    /// Identity (contract 4.8), never a raw name/email. Tagged Identifier / Pseudonymise: erased by
    /// deleting the Identity pseudonym map (the bytes then hold only the opaque pseudonym).
    #[personal_data(
        category = Identifier,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "agent_principal",
    )]
    pub agent_principal: String,
    /// the human the run acts ON BEHALF OF (the delegator) — an OPAQUE pseudonym (contract 4.8).
    /// Tagged Identifier / Pseudonymise (the attribution edge a DSR erases to a pseudonym).
    #[personal_data(
        category = Identifier,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "on_behalf_of",
    )]
    pub on_behalf_of: String,
    /// the agent BINDING this run was dispatched from (which trigger → which agent) — an opaque id.
    pub binding_id: u128,
    /// the triggering event id (envelope id) — an opaque routing id, no PII.
    pub trigger_event: String,
    /// the BUS-5 nested causality correlation id — opaque, no PII (the loop guards read it).
    pub correlation_id: String,
    /// the BUS-5 nested causality causation id — opaque, no PII.
    pub causation_id: String,
    /// the causal depth (the AG-6 depth-ceiling guard reads it; default ceiling 12) — not PII.
    pub depth: i32,
    /// the `runtime_ref` — WHICH runtime drives the brain (the strategy swap: skeleton | mock | llm).
    /// An opaque selector, no PII.
    pub runtime_ref: String,
    /// the run state (the durable-workflow state machine: running | parked-on-hitl | done | …) — not
    /// PII.
    pub state: String,
    /// the cost-gate RESERVATION id (the reserve/settle bookend, contract 11.7) — opaque, no PII.
    pub reservation_id: String,
    /// the run BUDGET in integer minor-units (NEVER floats — wholesale ≠ markup, C-1) — not PII.
    pub budget: i64,
    /// the `trace_ref` — the content-addressed `ArtifactRef` of this run's execution trace (§4.5).
    /// The pointer itself is opaque (a content hash); the TRACE DOCUMENT it points at is the
    /// `PersonalDataHolder` (its erasable body lands with Knowledge, AG-P19 → P-268).
    pub trace_ref: String,
}

/// The `tool_def` row (architecture §4.2) — the one permissioned registry. `(tenant, region)`-first.
/// Field list frozen by §4.2 / contract 8.1: `name`, `subsystem`, `version`, `input_schema`,
/// `required_caps`, `effect_kind`, `side_effecting`, `requires_approval`, `exposed_over_mcp`.
///
/// **No PII** — a tool definition is a catalogue entry (a name, a schema, caps, flags), not subject
/// data, so it deliberately does NOT `#[derive(PersonalData)]` (there is no subject to erase; the
/// `name` field is a TOOL name, not a person's). The `requires_approval` COLUMN exists here; its
/// per-subsystem **seed defaults** (CI deploy/secret = yes; Git merge = yes, open_pr = no; …) land
/// in AG-P8 (→ P-220), NOT here.
pub struct ToolDefRow {
    /// `(tenant, region)` partition key — opaque routing keys, no tag.
    pub tenant: TenantId,
    /// `(tenant, region)` partition key — opaque routing keys, no tag.
    pub region: Region,
    /// the tool name (the catalogue key half; `(subsystem, name, version)`) — not PII.
    pub name: String,
    /// the contributing subsystem (a bus §6.2 token) — not PII.
    pub subsystem: String,
    /// `ToolDef` is versioned (forward-only) — not PII.
    pub version: u32,
    /// JSON Schema for the tool's input, validated pre-apply — not PII (a schema, not subject data).
    pub input_schema: String,
    /// the `Permission`(s) the run must hold (the Id `check`, §5.2) — not PII.
    pub required_caps: Vec<String>,
    /// how the effect routes (`read | compute | mutate | external`) — the glue-crate value type.
    pub effect_kind: EffectKind,
    /// whether applying the tool has a side effect — not PII.
    pub side_effecting: bool,
    /// whether the tool is HITL-gated by default. The per-subsystem DEFAULT is SEEDED in AG-P8
    /// (→ P-220); here the column exists, no value is seeded — not PII.
    pub requires_approval: bool,
    /// whether the tool is exposed over the external MCP endpoint — not PII.
    pub exposed_over_mcp: bool,
}

/// The `proposed_effect` row (architecture §4.3) — the plan-then-apply audit row: EVERY proposed
/// effect recorded whether it was applied, gated, or denied (the trail that proves the agent only
/// ever *proposed*). `(tenant, region)`-first.
#[derive(PersonalData)]
pub struct ProposedEffectRow {
    /// `(tenant, region)` partition key — opaque routing keys, no tag.
    pub tenant: TenantId,
    /// `(tenant, region)` partition key — opaque routing keys, no tag.
    pub region: Region,
    /// the stable opaque proposed-effect id — not PII.
    pub effect_id: u128,
    /// the run this proposed effect belongs to (FK to `run.run_id`) — opaque, no PII.
    pub run_id: u128,
    /// the tool the effect invokes (FK to `tool_def.name`) — not PII.
    pub tool_name: String,
    /// the verdict: applied | gated | denied (the audit fact) — not PII.
    pub verdict: String,
    /// the effect's INPUT payload as proposed by the brain — may carry free-text PII (the brain
    /// authored it from tenant content). ENCRYPTED under the per-subject DEK (contract 11.4). Tagged
    /// Content / CryptoShred so the crypto-shred erase reaches it live + in backups.
    #[personal_data(
        category = Content,
        role = TenantContent,
        basis = Contract,
        retention = TenantPolicy,
        erasure = CryptoShred(subject_dek),
        subject_locator = "agent_principal",
    )]
    pub input_payload: Vec<u8>,
}

/// The `hitl_gate` row (architecture §4.4) — the approval state: a durable-workflow wait surfaced as
/// a chat approval card. `(tenant, region)`-first. Field list frozen by §4.4: `gate_id`, `run_id`,
/// `effect_id`, `risk_summary` (humanised), `cost_estimate`, `approver_filter` (a
/// `list_subjects`-derived set), `state`, `card_ref`.
#[derive(PersonalData)]
pub struct HitlGateRow {
    /// `(tenant, region)` partition key — opaque routing keys, no tag.
    pub tenant: TenantId,
    /// `(tenant, region)` partition key — opaque routing keys, no tag.
    pub region: Region,
    /// the stable opaque gate id — not PII.
    pub gate_id: u128,
    /// the run this gate belongs to (FK to `run.run_id`) — opaque, no PII.
    pub run_id: u128,
    /// the proposed effect this gate withholds (FK to `proposed_effect.effect_id`) — opaque, no PII.
    pub effect_id: u128,
    /// the HUMANISED risk summary the card shows (the pending action + risk). Per C9 this is a
    /// `(template_key, args)` rendered through Notif `humanise` — its rendered text may carry
    /// free-text / personal content, so it is ENCRYPTED under the per-subject DEK (contract 11.4).
    /// Tagged Content / CryptoShred.
    #[personal_data(
        category = Content,
        role = TenantContent,
        basis = Contract,
        retention = TenantPolicy,
        erasure = CryptoShred(subject_dek),
        subject_locator = "agent_principal",
    )]
    pub risk_summary: Vec<u8>,
    /// the LIVE cost estimate the card shows, integer minor-units (never floats) — not PII.
    pub cost_estimate: i64,
    /// the APPROVER set = `list_subjects(object, approve_perm)` (contract 4.4) — a set of OPAQUE
    /// principal pseudonyms (contract 4.8), never raw names. Tagged Identifier / Pseudonymise.
    #[personal_data(
        category = Identifier,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "approver_filter",
    )]
    pub approver_filter: Vec<String>,
    /// the gate state (pending | approved | rejected | expired) — not PII.
    pub state: String,
    /// the `card_ref` — the chat approval card this gate surfaces as — an opaque ref, no PII.
    pub card_ref: String,
}

/// The `trace` row (architecture §4.5) — the content-addressed execution-trace pointer.
/// `(tenant, region)`-first, **residency-pinned**. `run.trace_ref` is its `ArtifactRef`. The trace
/// IS a `PersonalDataHolder` (the conversation/tool-transcript the run owns is platform data,
/// residency-pinned, crypto-shred-capable).
///
/// **Floor (named, cross-referenced):** the holder BODY (the content-addressed write of the trace
/// document into Knowledge, + its erasure) lands with Knowledge in M3 (AG-P19 → P-268, KN-D11/KN-D12).
/// Here the column + the residency pin exist; the full DSR fan-out is AG-P23.
#[derive(PersonalData)]
pub struct TraceRow {
    /// `(tenant, region)` partition key — opaque routing keys, no tag. `region` IS the residency pin
    /// (no cross-region agent run on personal data, §8) — the trace never leaves its region.
    pub tenant: TenantId,
    /// `(tenant, region)` partition key — the RESIDENCY PIN (architecture §4.5 / §8).
    pub region: Region,
    /// the content-addressed `ArtifactRef` (the content hash of the trace document) — opaque, no PII
    /// (the addressable hash; the DOCUMENT it points at is the holder's erasable body).
    pub artifact_ref: String,
    /// the run this trace belongs to (FK to `run.run_id`) — opaque, no PII.
    pub run_id: u128,
    /// the trace BODY: the platform-owned conversation history (system context, prior tool results,
    /// the running transcript — architecture §2.1). It is tenant content the brain read/authored, so
    /// it is ENCRYPTED under the per-subject DEK (contract 11.4). Tagged Content / CryptoShred so the
    /// AG-D10 "erasure reaches the trace" drill's crypto-shred reaches it live + in backups.
    #[personal_data(
        category = Content,
        role = TenantContent,
        basis = Contract,
        retention = TenantPolicy,
        erasure = CryptoShred(subject_dek),
        subject_locator = "run_id",
    )]
    pub trace_body: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// All five row types compile with their `#[derive(PersonalData)]` + `#[personal_data(...)]`
    /// tags (contract 10.2) and lead with `(tenant, region)` (12.1). The structs being constructable
    /// with their fields readable proves the no-op derive left the items unchanged — and that a
    /// Fabric store CAN tag its PII fields today against the frozen classification (it will not
    /// compile against drift later). This is the compile-surface gate.
    #[test]
    fn the_five_tables_compile_tenant_region_first_with_tags() {
        let run = Run {
            tenant: TenantId::from_token("acme"),
            region: Region::new("fr-par"),
            run_id: 1,
            agent_principal: "psn:agent-7".into(),
            on_behalf_of: "psn:alice".into(),
            binding_id: 9,
            trigger_event: "evt:42".into(),
            correlation_id: "corr:1".into(),
            causation_id: "cause:1".into(),
            depth: 3,
            runtime_ref: "skeleton".into(),
            state: "running".into(),
            reservation_id: "rsv:1".into(),
            budget: 10_000,
            trace_ref: "sha256:abcd".into(),
        };
        // (tenant, region) FIRST — the partition key, from the verified token.
        assert_eq!(run.tenant, TenantId::from_token("acme"));
        assert_eq!(run.region, Region::new("fr-par"));
        assert_eq!(run.budget, 10_000); // integer minor-units, never a float.
        assert_eq!(run.runtime_ref, "skeleton"); // the strategy swap.
        assert_eq!(run.trace_ref, "sha256:abcd");

        let tool = ToolDefRow {
            tenant: TenantId::from_token("acme"),
            region: Region::new("fr-par"),
            name: "git.merge".into(),
            subsystem: "git".into(),
            version: 1,
            input_schema: "{}".into(),
            required_caps: vec!["git.merge".into()],
            effect_kind: EffectKind::Mutate,
            side_effecting: true,
            // The COLUMN exists; the per-subsystem DEFAULT is seeded in AG-P8 (→ P-220).
            requires_approval: false,
            exposed_over_mcp: false,
        };
        assert_eq!(tool.effect_kind, EffectKind::Mutate);
        assert!(tool.side_effecting);

        let effect = ProposedEffectRow {
            tenant: TenantId::from_token("acme"),
            region: Region::new("fr-par"),
            effect_id: 5,
            run_id: 1,
            tool_name: "git.merge".into(),
            verdict: "gated".into(),
            input_payload: b"{\"pr\":42}".to_vec(),
        };
        assert_eq!(effect.verdict, "gated");
        assert_eq!(effect.run_id, run.run_id);

        let gate = HitlGateRow {
            tenant: TenantId::from_token("acme"),
            region: Region::new("fr-par"),
            gate_id: 7,
            run_id: 1,
            effect_id: 5,
            risk_summary: b"merge PR #42 into main".to_vec(),
            cost_estimate: 50,
            approver_filter: vec!["psn:lead".into()],
            state: "pending".into(),
            card_ref: "card:1".into(),
        };
        assert_eq!(gate.effect_id, effect.effect_id);
        assert_eq!(gate.cost_estimate, 50); // integer minor-units.
        assert_eq!(gate.approver_filter, vec!["psn:lead".to_string()]);

        let trace = TraceRow {
            tenant: TenantId::from_token("acme"),
            region: Region::new("fr-par"),
            artifact_ref: "sha256:abcd".into(),
            run_id: 1,
            trace_body: b"system: you are an agent".to_vec(),
        };
        // The trace's region IS the residency pin (architecture §4.5 / §8).
        assert_eq!(trace.region, Region::new("fr-par"));
        assert_eq!(trace.artifact_ref, run.trace_ref); // run.trace_ref is the trace's ArtifactRef.
    }
}
