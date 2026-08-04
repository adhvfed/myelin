use myelin_agent::EffectKind;
use myelin_gdpr::PersonalData;
use myelin_tenancy::{Region, TenantId};

#[derive(PersonalData)]
pub struct Run {
    pub tenant: TenantId,
    pub region: Region,
    pub run_id: u128,
    #[personal_data(
        category = Identifier,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "agent_principal",
    )]
    pub agent_principal: String,
    #[personal_data(
        category = Identifier,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "on_behalf_of",
    )]
    pub on_behalf_of: String,
    pub binding_id: u128,
    pub trigger_event: String,
    pub correlation_id: String,
    pub causation_id: String,
    pub depth: i32,
    pub runtime_ref: String,
    pub state: String,
    pub reservation_id: String,
    pub budget: i64,
    pub trace_ref: String,
}

pub struct ToolDefRow {
    pub tenant: TenantId,
    pub region: Region,
    pub name: String,
    pub subsystem: String,
    pub version: u32,
    pub input_schema: String,
    pub required_caps: Vec<String>,
    pub effect_kind: EffectKind,
    pub side_effecting: bool,
    pub requires_approval: bool,
    pub exposed_over_mcp: bool,
}

#[derive(PersonalData)]
pub struct ProposedEffectRow {
    pub tenant: TenantId,
    pub region: Region,
    pub effect_id: u128,
    pub run_id: u128,
    pub tool_name: String,
    pub verdict: String,
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

#[derive(PersonalData)]
pub struct HitlGateRow {
    pub tenant: TenantId,
    pub region: Region,
    pub gate_id: u128,
    pub run_id: u128,
    pub effect_id: u128,
    #[personal_data(
        category = Content,
        role = TenantContent,
        basis = Contract,
        retention = TenantPolicy,
        erasure = CryptoShred(subject_dek),
        subject_locator = "agent_principal",
    )]
    pub risk_summary: Vec<u8>,
    pub cost_estimate: i64,
    #[personal_data(
        category = Identifier,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "approver_filter",
    )]
    pub approver_filter: Vec<String>,
    pub state: String,
    pub card_ref: String,
}

#[derive(PersonalData)]
pub struct TraceRow {
    pub tenant: TenantId,
    pub region: Region,
    pub artifact_ref: String,
    pub run_id: u128,
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
        assert_eq!(run.tenant, TenantId::from_token("acme"));
        assert_eq!(run.region, Region::new("fr-par"));
        assert_eq!(run.budget, 10_000);
        assert_eq!(run.runtime_ref, "skeleton");
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
        assert_eq!(gate.cost_estimate, 50);
        assert_eq!(gate.approver_filter, vec!["psn:lead".to_string()]);

        let trace = TraceRow {
            tenant: TenantId::from_token("acme"),
            region: Region::new("fr-par"),
            artifact_ref: "sha256:abcd".into(),
            run_id: 1,
            trace_body: b"system: you are an agent".to_vec(),
        };
        assert_eq!(trace.region, Region::new("fr-par"));
        assert_eq!(trace.artifact_ref, run.trace_ref);
    }
}
