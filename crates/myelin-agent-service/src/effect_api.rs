use myelin_agent::{
    EffectApi, EffectAuthority, EffectKind, EffectResult, EventId, GateId, ProposedEffect, RunCtx,
    ToolCall, ToolDef, ToolName, ToolSurface,
};
use myelin_identity::{
    CaveatContext, Consistency, ConsistencyMode, Decision, EffectivePolicy, FieldId, Permission,
    Principal, TransitionId, Zookie,
};
use myelin_identity_service::mint::RunTokenAuthorizer;
use myelin_storage::{
    reserve_settle::{MeteredUnit, MicroUsd},
    TenantScope,
};
use myelin_tenancy::{ArtifactRef, Region};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedEffect {
    pub tool: ToolName,
    pub object: ArtifactRef,
    pub input_json: String,
    pub field: Option<FieldId>,
    pub transition: Option<TransitionId>,
    pub cost: EffectCost,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectCost {
    pub unit: &'static str,
    pub wholesale: u64,
    pub markup: u64,
}

impl EffectCost {
    pub fn total(&self) -> u64 {
        self.wholesale.saturating_add(self.markup)
    }

    pub fn checked_total(&self) -> Option<u64> {
        self.wholesale.checked_add(self.markup)
    }

    fn as_metered_unit(&self) -> MeteredUnit {
        MeteredUnit {
            unit: self.unit,
            wholesale: MicroUsd(self.wholesale),
            markup: MicroUsd(self.markup),
        }
    }
}

pub fn encode_proposed(plan: &PlannedEffect) -> ProposedEffect {
    let field = plan.field.as_ref().map(|f| f.0.as_str()).unwrap_or("");
    let transition = plan.transition.as_ref().map(|t| t.0.as_str()).unwrap_or("");
    ProposedEffect(format!(
        "tool={}\u{1f}object={}\u{1f}field={}\u{1f}transition={}\u{1f}unit={}\u{1f}wholesale={}\u{1f}markup={}\u{1f}input={}",
        plan.tool.0,
        plan.object.0,
        field,
        transition,
        plan.cost.unit,
        plan.cost.wholesale,
        plan.cost.markup,
        plan.input_json,
    ))
}

pub fn effect_gate_key(tool: &ToolName, object: &ArtifactRef) -> String {
    effect_gate_key_str(&tool.0, &object.0)
}

pub fn effect_gate_key_str(tool: &str, object: &str) -> String {
    format!("gate:{tool}:{object}")
}

pub trait CapabilityCheck {
    fn check(
        &self,
        subject: &Principal,
        permission: &Permission,
        object: &ArtifactRef,
        at: &Consistency,
        caveat: Option<&CaveatContext>,
    ) -> Decision;
}

pub trait DelegationLookup {
    fn delegation(&self, agent: &Principal, trigger_actor: &Principal) -> EffectivePolicy;
}

pub trait TenantGuard {
    fn permits(&self, agent: &Principal, tool: &ToolName, object: &ArtifactRef) -> bool;
}

pub trait SubsystemApply {
    fn apply_public(
        &self,
        agent: &Principal,
        tool: &ToolName,
        object: &ArtifactRef,
        input_json: &str,
    ) -> Result<EventId, ApplyError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApplyError(pub String);

impl core::fmt::Display for ApplyError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "subsystem public-endpoint apply failed: {}", self.0)
    }
}

impl std::error::Error for ApplyError {}

pub trait EffectBudget {
    fn has_remaining(&self, cost: u64) -> bool;

    fn settle_one(&mut self, unit: &MeteredUnit) -> u64;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PipelineStep {
    Schema,
    Capability,
    Delegation,
    Tenant,
    Budget,
    HitlGate,
    Apply,
    Applied,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanVerdict {
    WouldApply,
    WouldGate(GateId),
    WouldDeny(PipelineStep, String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PipelineSignals {
    applied: u64,
    denied: u64,
    gated: u64,
    privileged_fallback: u64,
    metered_total: u64,
}

impl PipelineSignals {
    pub fn new() -> PipelineSignals {
        PipelineSignals::default()
    }
    pub fn applied(&self) -> u64 {
        self.applied
    }
    pub fn denied(&self) -> u64 {
        self.denied
    }
    pub fn gated(&self) -> u64 {
        self.gated
    }
    pub fn privileged_fallback(&self) -> u64 {
        self.privileged_fallback
    }
    pub fn metered_total(&self) -> u64 {
        self.metered_total
    }
}

pub struct PlanThenApply<'a, S, C, D, T, A, B>
where
    S: ToolSurface,
    C: CapabilityCheck,
    D: DelegationLookup,
    T: TenantGuard,
    A: SubsystemApply,
    B: EffectBudget,
{
    pub catalogue: &'a S,
    pub check: &'a C,
    pub delegation: &'a D,
    pub tenant: &'a T,
    pub apply_endpoint: &'a A,
    pub budget: &'a mut B,
    pub agent: Principal,
    pub trigger_actor: Principal,
    pub zookie: Zookie,
    pub approved: std::collections::BTreeSet<String>,
    pub signals: &'a mut PipelineSignals,
}

impl<'a, S, C, D, T, A, B> PlanThenApply<'a, S, C, D, T, A, B>
where
    S: ToolSurface,
    C: CapabilityCheck,
    D: DelegationLookup,
    T: TenantGuard,
    A: SubsystemApply,
    B: EffectBudget,
{
    pub fn apply_planned(&mut self, plan: &PlannedEffect) -> EffectResult {
        match self.plan_through_gate(plan) {
            PlanVerdict::WouldDeny(step, reason) => return self.deny(step, reason),
            PlanVerdict::WouldGate(gate_id) => {
                self.signals.gated = self.signals.gated.saturating_add(1);
                return EffectResult::Gated(gate_id);
            }
            PlanVerdict::WouldApply => {}
        }

        let event_id = match self.apply_endpoint.apply_public(
            &self.agent,
            &plan.tool,
            &plan.object,
            &plan.input_json,
        ) {
            Ok(id) => id,
            Err(e) => return self.deny(PipelineStep::Apply, e.to_string()),
        };

        let billed = self.budget.settle_one(&plan.cost.as_metered_unit());
        self.signals.applied = self.signals.applied.saturating_add(1);
        self.signals.metered_total = self.signals.metered_total.saturating_add(billed);
        EffectResult::Applied(event_id)
    }

    pub fn plan_through_gate(&self, plan: &PlannedEffect) -> PlanVerdict {
        let def: &ToolDef = match self.catalogue.resolve(&plan.tool) {
            Some(d) => d,
            None => {
                return PlanVerdict::WouldDeny(
                    PipelineStep::Schema,
                    format!("unknown tool {}", plan.tool.0),
                )
            }
        };
        if let Err(reason) = validate_schema(&def.input_schema, &plan.input_json) {
            return PlanVerdict::WouldDeny(PipelineStep::Schema, reason);
        }

        if !matches!(def.effect_kind, EffectKind::Mutate | EffectKind::External) {
            return PlanVerdict::WouldDeny(
                PipelineStep::Schema,
                format!(
                    "tool {} is {:?}, not mutate/external - it does not route through EffectApi (§5.0)",
                    plan.tool.0, def.effect_kind
                ),
            );
        }

        let caveat = CaveatContext {
            object: plan.object.clone(),
            field: plan.field.clone(),
            transition: plan.transition.clone(),
            attrs: std::collections::BTreeMap::new(),
        };
        let at = Consistency {
            at_least: self.zookie.clone(),
            mode: ConsistencyMode::Strong,
        };

        for cap in &def.required_caps {
            let permission = Permission(cap.clone());
            match self
                .check
                .check(&self.agent, &permission, &plan.object, &at, Some(&caveat))
            {
                Decision::Allow => {}
                Decision::Deny | Decision::Conditional => {
                    return PlanVerdict::WouldDeny(
                        PipelineStep::Capability,
                        format!("capability check denied for {cap}"),
                    );
                }
            }
        }

        let policy: EffectivePolicy = self.delegation.delegation(&self.agent, &self.trigger_actor);
        for cap in &def.required_caps {
            if !policy.caveats.iter().any(|c| c == cap) {
                return PlanVerdict::WouldDeny(
                    PipelineStep::Delegation,
                    format!(
                        "{cap} is outside the delegation intersection \
                         (agent.policy ∩ delegation ∩ tenant.policy) - attenuation never up"
                    ),
                );
            }
        }

        if !self.tenant.permits(&self.agent, &plan.tool, &plan.object) {
            return PlanVerdict::WouldDeny(
                PipelineStep::Tenant,
                format!(
                    "tenant guardrails forbid {} on {}",
                    plan.tool.0, plan.object.0
                ),
            );
        }

        let Some(cost) = plan.cost.checked_total() else {
            return PlanVerdict::WouldDeny(
                PipelineStep::Budget,
                "metered cost exceeds the supported minor-unit range".into(),
            );
        };
        if !self.budget.has_remaining(cost) {
            return PlanVerdict::WouldDeny(
                PipelineStep::Budget,
                format!("reserve has no remaining balance for cost {cost} minor-units"),
            );
        }

        let gate_key = effect_gate_key(&plan.tool, &plan.object);
        if def.requires_approval && !self.approved.contains(&gate_key) {
            return PlanVerdict::WouldGate(GateId(gate_key));
        }

        PlanVerdict::WouldApply
    }

    fn deny(&mut self, _step: PipelineStep, reason: String) -> EffectResult {
        self.signals.denied = self.signals.denied.saturating_add(1);
        EffectResult::Denied(reason)
    }
}

pub struct EffectApiBridge<'a, S, C, D, T, A, B>(
    core::cell::RefCell<PlanThenApply<'a, S, C, D, T, A, B>>,
    Option<std::sync::Arc<RunTokenAuthorizer>>,
)
where
    S: ToolSurface,
    C: CapabilityCheck,
    D: DelegationLookup,
    T: TenantGuard,
    A: SubsystemApply,
    B: EffectBudget;

impl<'a, S, C, D, T, A, B> EffectApiBridge<'a, S, C, D, T, A, B>
where
    S: ToolSurface,
    C: CapabilityCheck,
    D: DelegationLookup,
    T: TenantGuard,
    A: SubsystemApply,
    B: EffectBudget,
{
    pub fn new(pipeline: PlanThenApply<'a, S, C, D, T, A, B>) -> Self {
        EffectApiBridge(core::cell::RefCell::new(pipeline), None)
    }

    pub fn with_run_token_authorizer(
        pipeline: PlanThenApply<'a, S, C, D, T, A, B>,
        authorizer: std::sync::Arc<RunTokenAuthorizer>,
    ) -> Self {
        EffectApiBridge(core::cell::RefCell::new(pipeline), Some(authorizer))
    }
}

impl<S, C, D, T, A, B> EffectApi for EffectApiBridge<'_, S, C, D, T, A, B>
where
    S: ToolSurface,
    C: CapabilityCheck,
    D: DelegationLookup,
    T: TenantGuard,
    A: SubsystemApply,
    B: EffectBudget,
{
    fn apply(&self, _run: &RunCtx, _effect: ProposedEffect) -> EffectResult {
        EffectResult::Denied(
            "external plan-then-apply requires the signed run-token authority entry - direct apply denied"
                .into(),
        )
    }

    fn apply_authorized(
        &self,
        _run: &RunCtx,
        authority: &EffectAuthority,
        effect: ProposedEffect,
    ) -> EffectResult {
        match decode_proposed(&effect) {
            Ok(plan) => {
                if authority.tool != plan.tool.0 {
                    return EffectResult::Denied(format!(
                        "run-token authority is bound to `{}`, not proposed tool `{}`",
                        authority.tool, plan.tool.0
                    ));
                }
                let Some(authorizer) = &self.1 else {
                    return EffectResult::Denied(
                        "plan-then-apply bridge has no final-boundary run-token authorizer - denied"
                            .into(),
                    );
                };
                let pipeline = self.0.borrow();
                if authority.principal_id != pipeline.agent.principal_id {
                    return EffectResult::Denied(
                        "run-token authority principal does not match the plan-then-apply principal"
                            .into(),
                    );
                }
                let Some(def) = pipeline.catalogue.resolve(&plan.tool) else {
                    return EffectResult::Denied(format!("unknown tool {}", plan.tool.0));
                };
                let scope = TenantScope::from_verified_token(
                    &pipeline.agent,
                    Region(pipeline.agent.region.0.clone()),
                );
                if let Err(reason) = authorizer.authorize(
                    &scope,
                    &pipeline.agent.principal_id,
                    &authority.run_token,
                    &def.required_caps,
                ) {
                    return EffectResult::Denied(reason);
                }
                drop(pipeline);
                self.0.borrow_mut().apply_planned(&plan)
            }
            Err(reason) => {
                let mut p = self.0.borrow_mut();
                p.signals.denied = p.signals.denied.saturating_add(1);
                EffectResult::Denied(format!("malformed proposed effect: {reason}"))
            }
        }
    }
}

pub fn validate_schema(input_schema: &str, input_json: &str) -> Result<(), String> {
    let input: serde_json::Value =
        serde_json::from_str(input_json).map_err(|e| format!("input is not valid JSON: {e}"))?;
    let schema: serde_json::Value = serde_json::from_str(input_schema)
        .map_err(|e| format!("tool input_schema is not valid JSON: {e}"))?;

    let Some(schema_obj) = schema.as_object() else {
        return Ok(());
    };

    if schema_obj.get("type").and_then(|t| t.as_str()) == Some("object") && !input.is_object() {
        return Err("schema requires an object input".into());
    }

    if let Some(required) = schema_obj.get("required").and_then(|r| r.as_array()) {
        let input_obj = input
            .as_object()
            .ok_or_else(|| "schema has `required` but input is not an object".to_string())?;
        for req in required {
            if let Some(name) = req.as_str() {
                if !input_obj.contains_key(name) {
                    return Err(format!("required field `{name}` is missing"));
                }
            }
        }
    }

    if let Some(props) = schema_obj.get("properties").and_then(|p| p.as_object()) {
        if let Some(input_obj) = input.as_object() {
            for (name, prop_schema) in props {
                let Some(value) = input_obj.get(name) else {
                    continue;
                };
                if let Some(want) = prop_schema.get("type").and_then(|t| t.as_str()) {
                    if !json_type_matches(want, value) {
                        return Err(format!(
                            "field `{name}` must be of type `{want}`, got `{}`",
                            json_type_name(value)
                        ));
                    }
                }
            }
        }
    }

    Ok(())
}

pub fn validate_tool_arguments(def: &ToolDef, arguments: &serde_json::Value) -> Result<(), String> {
    let input_json = serde_json::to_string(arguments)
        .map_err(|e| format!("tool arguments are not serialisable JSON: {e}"))?;
    validate_schema(&def.input_schema, &input_json)
}

pub fn validate_call<S: ToolSurface + ?Sized>(catalogue: &S, call: &ToolCall) -> Result<(), String> {
    let def = catalogue
        .resolve(&call.name)
        .ok_or_else(|| format!("tool `{}` is not registered in the catalogue", call.name.0))?;
    validate_tool_arguments(def, &call.arguments)
}

fn json_type_matches(want: &str, value: &serde_json::Value) -> bool {
    match want {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "number" => value.is_number(),
        "integer" => value.is_i64() || value.is_u64(),
        "null" => value.is_null(),
        _ => true,
    }
}

fn json_type_name(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

pub fn decode_proposed(effect: &ProposedEffect) -> Result<PlannedEffect, String> {
    let mut tool = None;
    let mut object = None;
    let mut field = None;
    let mut transition = None;
    let mut unit: Option<&'static str> = None;
    let mut wholesale = None;
    let mut markup = None;
    let mut input = None;

    for part in effect.0.split('\u{1f}') {
        let Some((key, val)) = part.split_once('=') else {
            return Err(format!("malformed segment (no `=`): {part:?}"));
        };
        match key {
            "tool" => tool = Some(ToolName(val.to_string())),
            "object" => object = Some(ArtifactRef(val.to_string())),
            "field" => {
                field = if val.is_empty() {
                    None
                } else {
                    Some(FieldId(val.to_string()))
                }
            }
            "transition" => {
                transition = if val.is_empty() {
                    None
                } else {
                    Some(TransitionId(val.to_string()))
                }
            }
            "unit" => unit = Some(intern_unit(val)?),
            "wholesale" => {
                wholesale = Some(
                    val.parse::<u64>()
                        .map_err(|e| format!("bad wholesale: {e}"))?,
                )
            }
            "markup" => markup = Some(val.parse::<u64>().map_err(|e| format!("bad markup: {e}"))?),
            "input" => input = Some(val.to_string()),
            other => return Err(format!("unknown segment key: {other}")),
        }
    }

    Ok(PlannedEffect {
        tool: tool.ok_or("missing tool")?,
        object: object.ok_or("missing object")?,
        input_json: input.ok_or("missing input")?,
        field,
        transition,
        cost: EffectCost {
            unit: unit.ok_or("missing unit")?,
            wholesale: wholesale.ok_or("missing wholesale")?,
            markup: markup.ok_or("missing markup")?,
        },
    })
}

fn intern_unit(unit: &str) -> Result<&'static str, String> {
    match unit {
        "agent.effect" => Ok("agent.effect"),
        "issue.transition" => Ok("issue.transition"),
        "git.merge" => Ok("git.merge"),
        "external.call" => Ok("external.call"),
        other => Err(format!("unknown metered-unit dimension: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalId, PrincipalKind};
    use myelin_tenancy::TenantId;
    use std::cell::RefCell;
    use std::collections::BTreeSet;

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

    struct Checker {
        allow: BTreeSet<String>,
        conditional_on_transition: BTreeSet<String>,
    }
    impl CapabilityCheck for Checker {
        fn check(
            &self,
            _subject: &Principal,
            permission: &Permission,
            _object: &ArtifactRef,
            _at: &Consistency,
            caveat: Option<&CaveatContext>,
        ) -> Decision {
            if self.conditional_on_transition.contains(&permission.0)
                && caveat.map(|c| c.transition.is_some()).unwrap_or(false)
            {
                return Decision::Conditional;
            }
            if self.allow.contains(&permission.0) {
                Decision::Allow
            } else {
                Decision::Deny
            }
        }
    }

    struct Delegator {
        policy: Vec<String>,
    }
    impl DelegationLookup for Delegator {
        fn delegation(&self, _agent: &Principal, _trigger: &Principal) -> EffectivePolicy {
            EffectivePolicy {
                caveats: self.policy.clone(),
            }
        }
    }

    struct Tenant {
        forbid: BTreeSet<String>,
    }
    impl TenantGuard for Tenant {
        fn permits(&self, _agent: &Principal, tool: &ToolName, _object: &ArtifactRef) -> bool {
            !self.forbid.contains(&tool.0)
        }
    }

    struct Endpoint {
        fail: bool,
        applied: std::cell::RefCell<Vec<(String, String)>>,
    }
    impl SubsystemApply for Endpoint {
        fn apply_public(
            &self,
            _agent: &Principal,
            tool: &ToolName,
            object: &ArtifactRef,
            _input: &str,
        ) -> Result<EventId, ApplyError> {
            if self.fail {
                return Err(ApplyError("endpoint unavailable".into()));
            }
            self.applied
                .borrow_mut()
                .push((tool.0.clone(), object.0.clone()));
            Ok(EventId(format!("evt:{}:{}", tool.0, object.0)))
        }
    }

    struct Budget {
        remaining: u64,
        billed: u64,
        settles: u64,
    }
    impl EffectBudget for Budget {
        fn has_remaining(&self, cost: u64) -> bool {
            self.remaining >= cost
        }
        fn settle_one(&mut self, unit: &MeteredUnit) -> u64 {
            let total = unit.total().map(|m| m.0).unwrap_or(0);
            self.remaining = self.remaining.saturating_sub(total);
            self.billed = self.billed.saturating_add(total);
            self.settles += 1;
            total
        }
    }

    fn agent() -> Principal {
        Principal::stub(
            PrincipalId("psn:agent-7".into()),
            PrincipalKind::Agent {
                runtime_ref: myelin_identity::RuntimeRef("mock".into()),
                on_behalf_of: None,
            },
            TenantId("acme".into()),
        )
    }
    fn human() -> Principal {
        Principal::stub(
            PrincipalId("psn:human-x".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
    }

    fn tool_def(name: &str, caps: &[&str], requires_approval: bool, kind: EffectKind) -> ToolDef {
        ToolDef {
            name: ToolName(name.into()),
            subsystem: "issues".into(),
            version: 1,
            input_schema:
                r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#
                    .into(),
            required_caps: caps.iter().map(|c| c.to_string()).collect(),
            effect_kind: kind,
            side_effecting: true,
            requires_approval,
            exposed_over_mcp: false,
        }
    }

    fn plan(tool: &str, input: &str) -> PlannedEffect {
        PlannedEffect {
            tool: ToolName(tool.into()),
            object: ArtifactRef("myelin://acme/issues/i-1".into()),
            input_json: input.into(),
            field: None,
            transition: None,
            cost: EffectCost {
                unit: "issue.transition",
                wholesale: 3,
                markup: 1,
            },
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn pipeline<'a>(
        catalogue: &'a Catalogue,
        check: &'a Checker,
        delegation: &'a Delegator,
        tenant: &'a Tenant,
        endpoint: &'a Endpoint,
        budget: &'a mut Budget,
        approved: BTreeSet<String>,
        signals: &'a mut PipelineSignals,
    ) -> PlanThenApply<'a, Catalogue, Checker, Delegator, Tenant, Endpoint, Budget> {
        PlanThenApply {
            catalogue,
            check,
            delegation,
            tenant,
            apply_endpoint: endpoint,
            budget,
            agent: agent(),
            trigger_actor: human(),
            zookie: Zookie("z-1".into()),
            approved,
            signals,
        }
    }

    fn allow_caps(caps: &[&str]) -> Checker {
        Checker {
            allow: caps.iter().map(|c| c.to_string()).collect(),
            conditional_on_transition: BTreeSet::new(),
        }
    }

    #[test]
    fn pipeline_applies_an_allowed_effect_and_meters_it() {
        let cat = Catalogue {
            defs: vec![tool_def(
                "issue.create",
                &["issue.write"],
                false,
                EffectKind::Mutate,
            )],
        };
        let check = allow_caps(&["issue.write"]);
        let del = Delegator {
            policy: vec!["issue.write".into()],
        };
        let tenant = Tenant {
            forbid: BTreeSet::new(),
        };
        let endpoint = Endpoint {
            fail: false,
            applied: RefCell::new(vec![]),
        };
        let mut budget = Budget {
            remaining: 100,
            billed: 0,
            settles: 0,
        };
        let mut signals = PipelineSignals::new();
        let mut p = pipeline(
            &cat,
            &check,
            &del,
            &tenant,
            &endpoint,
            &mut budget,
            BTreeSet::new(),
            &mut signals,
        );

        let out = p.apply_planned(&plan("issue.create", r#"{"title":"bug"}"#));
        assert!(
            matches!(out, EffectResult::Applied(EventId(ref id)) if id == "evt:issue.create:myelin://acme/issues/i-1")
        );

        assert_eq!(
            endpoint.applied.borrow().len(),
            1,
            "exactly one apply via the public endpoint"
        );
        assert_eq!(
            budget.settles, 1,
            "exactly one cost event settled (the METER step)"
        );
        assert_eq!(budget.billed, 4, "billed wholesale 3 + markup 1");
        assert_eq!(budget.remaining, 96, "the reserve debited the bill");
        assert_eq!(signals.applied(), 1);
        assert_eq!(signals.metered_total(), 4);
        assert_eq!(signals.denied(), 0);
        assert_eq!(
            signals.privileged_fallback(),
            0,
            "AG-D2: NO privileged fallback EVER fires"
        );
    }

    #[test]
    fn step1_schema_reject_denies_before_any_mutation() {
        let cat = Catalogue {
            defs: vec![tool_def(
                "issue.create",
                &["issue.write"],
                false,
                EffectKind::Mutate,
            )],
        };
        let check = allow_caps(&["issue.write"]);
        let del = Delegator {
            policy: vec!["issue.write".into()],
        };
        let tenant = Tenant {
            forbid: BTreeSet::new(),
        };
        let endpoint = Endpoint {
            fail: false,
            applied: RefCell::new(vec![]),
        };
        let mut budget = Budget {
            remaining: 100,
            billed: 0,
            settles: 0,
        };
        let mut signals = PipelineSignals::new();
        let mut p = pipeline(
            &cat,
            &check,
            &del,
            &tenant,
            &endpoint,
            &mut budget,
            BTreeSet::new(),
            &mut signals,
        );

        let out = p.apply_planned(&plan("issue.create", r#"{"body":"x"}"#));
        assert!(
            matches!(out, EffectResult::Denied(ref r) if r.contains("title")),
            "{out:?}"
        );
        assert_eq!(
            endpoint.applied.borrow().len(),
            0,
            "schema reject → 0 mutation"
        );
        assert_eq!(budget.settles, 0, "a denied effect is never metered");
        assert_eq!(
            signals.denied(),
            1,
            "the denial counter incremented (AG-D2)"
        );
        assert_eq!(signals.privileged_fallback(), 0);
    }

    #[test]
    fn step2_capability_deny_and_caveat_conditional_deny() {
        let cat = Catalogue {
            defs: vec![tool_def(
                "issue.transition",
                &["issue.transition"],
                false,
                EffectKind::Mutate,
            )],
        };
        let check = allow_caps(&[]);
        let del = Delegator {
            policy: vec!["issue.transition".into()],
        };
        let tenant = Tenant {
            forbid: BTreeSet::new(),
        };
        let endpoint = Endpoint {
            fail: false,
            applied: RefCell::new(vec![]),
        };
        let mut budget = Budget {
            remaining: 100,
            billed: 0,
            settles: 0,
        };
        let mut signals = PipelineSignals::new();
        {
            let mut p = pipeline(
                &cat,
                &check,
                &del,
                &tenant,
                &endpoint,
                &mut budget,
                BTreeSet::new(),
                &mut signals,
            );
            let out = p.apply_planned(&plan("issue.transition", r#"{"title":"x"}"#));
            assert!(
                matches!(out, EffectResult::Denied(ref r) if r.contains("capability")),
                "{out:?}"
            );
        }
        assert_eq!(
            endpoint.applied.borrow().len(),
            0,
            "capability deny → 0 mutation"
        );

        let check2 = Checker {
            allow: ["issue.transition".to_string()].into_iter().collect(),
            conditional_on_transition: ["issue.transition".to_string()].into_iter().collect(),
        };
        let mut budget2 = Budget {
            remaining: 100,
            billed: 0,
            settles: 0,
        };
        let mut signals2 = PipelineSignals::new();
        let mut p2 = pipeline(
            &cat,
            &check2,
            &del,
            &tenant,
            &endpoint,
            &mut budget2,
            BTreeSet::new(),
            &mut signals2,
        );
        let mut plan_t = plan("issue.transition", r#"{"title":"x"}"#);
        plan_t.transition = Some(TransitionId("to_done".into()));
        let out = p2.apply_planned(&plan_t);
        assert!(
            matches!(out, EffectResult::Denied(_)),
            "Conditional (caveat unmet) is a DENY, never a silent allow: {out:?}"
        );
        assert_eq!(
            endpoint.applied.borrow().len(),
            0,
            "the SLA-bound transition did NOT mutate"
        );
    }

    #[test]
    fn step3_delegation_intersection_confines_over_privilege() {
        let cat = Catalogue {
            defs: vec![tool_def(
                "issue.delete",
                &["issue.delete"],
                false,
                EffectKind::Mutate,
            )],
        };
        let check = allow_caps(&["issue.delete"]);
        let del = Delegator {
            policy: vec!["issue.write".into()],
        };
        let tenant = Tenant {
            forbid: BTreeSet::new(),
        };
        let endpoint = Endpoint {
            fail: false,
            applied: RefCell::new(vec![]),
        };
        let mut budget = Budget {
            remaining: 100,
            billed: 0,
            settles: 0,
        };
        let mut signals = PipelineSignals::new();
        let mut p = pipeline(
            &cat,
            &check,
            &del,
            &tenant,
            &endpoint,
            &mut budget,
            BTreeSet::new(),
            &mut signals,
        );

        let out = p.apply_planned(&plan("issue.delete", r#"{"title":"x"}"#));
        assert!(
            matches!(out, EffectResult::Denied(ref r) if r.contains("intersection")),
            "{out:?}"
        );
        assert_eq!(
            endpoint.applied.borrow().len(),
            0,
            "over-privilege confined → 0 mutation (AG-D3)"
        );
        assert_eq!(signals.denied(), 1);
    }

    #[test]
    fn step4_tenant_guard_denies() {
        let cat = Catalogue {
            defs: vec![tool_def(
                "issue.create",
                &["issue.write"],
                false,
                EffectKind::Mutate,
            )],
        };
        let check = allow_caps(&["issue.write"]);
        let del = Delegator {
            policy: vec!["issue.write".into()],
        };
        let tenant = Tenant {
            forbid: ["issue.create".to_string()].into_iter().collect(),
        };
        let endpoint = Endpoint {
            fail: false,
            applied: RefCell::new(vec![]),
        };
        let mut budget = Budget {
            remaining: 100,
            billed: 0,
            settles: 0,
        };
        let mut signals = PipelineSignals::new();
        let mut p = pipeline(
            &cat,
            &check,
            &del,
            &tenant,
            &endpoint,
            &mut budget,
            BTreeSet::new(),
            &mut signals,
        );

        let out = p.apply_planned(&plan("issue.create", r#"{"title":"x"}"#));
        assert!(
            matches!(out, EffectResult::Denied(ref r) if r.contains("tenant")),
            "{out:?}"
        );
        assert_eq!(
            endpoint.applied.borrow().len(),
            0,
            "tenant deny → 0 mutation"
        );
    }

    #[test]
    fn step5_budget_refusal_denies_with_no_fallback() {
        let cat = Catalogue {
            defs: vec![tool_def(
                "issue.create",
                &["issue.write"],
                false,
                EffectKind::Mutate,
            )],
        };
        let check = allow_caps(&["issue.write"]);
        let del = Delegator {
            policy: vec!["issue.write".into()],
        };
        let tenant = Tenant {
            forbid: BTreeSet::new(),
        };
        let endpoint = Endpoint {
            fail: false,
            applied: RefCell::new(vec![]),
        };
        let mut budget = Budget {
            remaining: 1,
            billed: 0,
            settles: 0,
        };
        let mut signals = PipelineSignals::new();
        let mut p = pipeline(
            &cat,
            &check,
            &del,
            &tenant,
            &endpoint,
            &mut budget,
            BTreeSet::new(),
            &mut signals,
        );

        let out = p.apply_planned(&plan("issue.create", r#"{"title":"x"}"#));
        assert!(
            matches!(out, EffectResult::Denied(ref r) if r.contains("balance")),
            "{out:?}"
        );
        assert_eq!(
            endpoint.applied.borrow().len(),
            0,
            "no balance → no mutation"
        );
        assert_eq!(budget.settles, 0, "a budget-denied effect is never metered");
        assert_eq!(
            signals.privileged_fallback(),
            0,
            "AG-D2: 0 privileged fallback"
        );
    }

    #[test]
    fn step5_unrepresentable_cost_denies_before_mutation_or_metering() {
        let cat = Catalogue {
            defs: vec![tool_def(
                "issue.create",
                &["issue.write"],
                false,
                EffectKind::Mutate,
            )],
        };
        let check = allow_caps(&["issue.write"]);
        let del = Delegator {
            policy: vec!["issue.write".into()],
        };
        let tenant = Tenant {
            forbid: BTreeSet::new(),
        };
        let endpoint = Endpoint {
            fail: false,
            applied: RefCell::new(vec![]),
        };
        let mut budget = Budget {
            remaining: u64::MAX,
            billed: 0,
            settles: 0,
        };
        let mut signals = PipelineSignals::new();
        let mut effect = plan("issue.create", r#"{"title":"x"}"#);
        effect.cost = EffectCost {
            unit: "issue.create",
            wholesale: u64::MAX,
            markup: 1,
        };
        let mut pipeline = pipeline(
            &cat,
            &check,
            &del,
            &tenant,
            &endpoint,
            &mut budget,
            BTreeSet::new(),
            &mut signals,
        );

        let outcome = pipeline.apply_planned(&effect);
        assert!(
            matches!(outcome, EffectResult::Denied(ref reason) if reason.contains("minor-unit range")),
            "{outcome:?}"
        );
        assert!(endpoint.applied.borrow().is_empty(), "overflowing cost must not mutate");
        assert_eq!(budget.settles, 0, "overflowing cost must not reach settlement");
        assert_eq!(budget.billed, 0);
    }

    #[test]
    fn step6_hitl_gate_withholds_then_resumes() {
        let cat = Catalogue {
            defs: vec![tool_def(
                "git.merge",
                &["git.merge"],
                true,
                EffectKind::Mutate,
            )],
        };
        let check = allow_caps(&["git.merge"]);
        let del = Delegator {
            policy: vec!["git.merge".into()],
        };
        let tenant = Tenant {
            forbid: BTreeSet::new(),
        };
        let endpoint = Endpoint {
            fail: false,
            applied: RefCell::new(vec![]),
        };

        let mut budget = Budget {
            remaining: 100,
            billed: 0,
            settles: 0,
        };
        let mut signals = PipelineSignals::new();
        {
            let mut p = pipeline(
                &cat,
                &check,
                &del,
                &tenant,
                &endpoint,
                &mut budget,
                BTreeSet::new(),
                &mut signals,
            );
            let out = p.apply_planned(&plan("git.merge", r#"{"title":"x"}"#));
            assert!(
                matches!(out, EffectResult::Gated(GateId(ref g)) if g.starts_with("gate:git.merge")),
                "{out:?}"
            );
        }
        assert_eq!(
            endpoint.applied.borrow().len(),
            0,
            "a gated effect does NOT mutate (AG-8)"
        );
        assert_eq!(
            budget.settles, 0,
            "a gated effect is never metered (it didn't apply)"
        );
        assert_eq!(signals.gated(), 1);

        let mut budget2 = Budget {
            remaining: 100,
            billed: 0,
            settles: 0,
        };
        let mut signals2 = PipelineSignals::new();
        let the_plan = plan("git.merge", r#"{"title":"x"}"#);
        let approved: BTreeSet<String> =
            [effect_gate_key(&the_plan.tool, &the_plan.object)].into_iter().collect();
        let mut p2 = pipeline(
            &cat,
            &check,
            &del,
            &tenant,
            &endpoint,
            &mut budget2,
            approved,
            &mut signals2,
        );
        let out = p2.apply_planned(&the_plan);
        assert!(
            matches!(out, EffectResult::Applied(_)),
            "an approved gated effect Applies: {out:?}"
        );
        assert_eq!(
            endpoint.applied.borrow().len(),
            1,
            "the approved effect mutated once"
        );
    }

    #[test]
    fn step6_approval_is_per_effect_never_per_tool_name() {
        let cat = Catalogue {
            defs: vec![tool_def(
                "git.merge",
                &["git.merge"],
                true,
                EffectKind::Mutate,
            )],
        };
        let check = allow_caps(&["git.merge"]);
        let del = Delegator {
            policy: vec!["git.merge".into()],
        };
        let tenant = Tenant {
            forbid: BTreeSet::new(),
        };
        let endpoint = Endpoint {
            fail: false,
            applied: RefCell::new(vec![]),
        };

        let mut plan_a = plan("git.merge", r#"{"title":"a"}"#);
        plan_a.object = ArtifactRef("myelin://acme/git/pr/40".into());
        let mut plan_b = plan("git.merge", r#"{"title":"b"}"#);
        plan_b.object = ArtifactRef("myelin://acme/git/pr/41".into());

        let approved: BTreeSet<String> =
            [effect_gate_key(&plan_a.tool, &plan_a.object)].into_iter().collect();
        let mut budget = Budget { remaining: 100, billed: 0, settles: 0 };
        let mut signals = PipelineSignals::new();
        let mut p = pipeline(
            &cat, &check, &del, &tenant, &endpoint, &mut budget, approved, &mut signals,
        );
        assert!(
            matches!(p.apply_planned(&plan_a), EffectResult::Applied(_)),
            "the approved effect (pr 40) applies"
        );
        assert!(
            matches!(p.apply_planned(&plan_b), EffectResult::Gated(_)),
            "the sibling (pr 41) sharing the tool name still GATES - approval never transfers"
        );
        assert_eq!(endpoint.applied.borrow().len(), 1, "exactly the approved effect mutated");

        let by_name: BTreeSet<String> = ["git.merge".to_string()].into_iter().collect();
        let mut budget2 = Budget { remaining: 100, billed: 0, settles: 0 };
        let mut signals2 = PipelineSignals::new();
        let mut p2 = pipeline(
            &cat, &check, &del, &tenant, &endpoint, &mut budget2, by_name, &mut signals2,
        );
        assert!(
            matches!(p2.apply_planned(&plan_b), EffectResult::Gated(_)),
            "a bare tool name in the approved set clears NO gate (the old bypass shape)"
        );
    }

    #[test]
    fn step7_apply_failure_is_loud_and_unmetered() {
        let cat = Catalogue {
            defs: vec![tool_def(
                "issue.create",
                &["issue.write"],
                false,
                EffectKind::Mutate,
            )],
        };
        let check = allow_caps(&["issue.write"]);
        let del = Delegator {
            policy: vec!["issue.write".into()],
        };
        let tenant = Tenant {
            forbid: BTreeSet::new(),
        };
        let endpoint = Endpoint {
            fail: true,
            applied: RefCell::new(vec![]),
        };
        let mut budget = Budget {
            remaining: 100,
            billed: 0,
            settles: 0,
        };
        let mut signals = PipelineSignals::new();
        let mut p = pipeline(
            &cat,
            &check,
            &del,
            &tenant,
            &endpoint,
            &mut budget,
            BTreeSet::new(),
            &mut signals,
        );

        let out = p.apply_planned(&plan("issue.create", r#"{"title":"x"}"#));
        assert!(
            matches!(out, EffectResult::Denied(ref r) if r.contains("apply failed")),
            "{out:?}"
        );
        assert_eq!(budget.settles, 0, "a failed apply is NOT metered");
        assert_eq!(signals.applied(), 0);
        assert_eq!(signals.denied(), 1);
    }

    #[test]
    fn chained_e2e_allowed_then_disallowed_in_one_session() {
        let cat = Catalogue {
            defs: vec![
                tool_def("issue.create", &["issue.write"], false, EffectKind::Mutate),
                tool_def("issue.delete", &["issue.delete"], false, EffectKind::Mutate),
            ],
        };
        let check = allow_caps(&["issue.write", "issue.delete"]);
        let del = Delegator {
            policy: vec!["issue.write".into()],
        };
        let tenant = Tenant {
            forbid: BTreeSet::new(),
        };
        let endpoint = Endpoint {
            fail: false,
            applied: RefCell::new(vec![]),
        };
        let mut budget = Budget {
            remaining: 100,
            billed: 0,
            settles: 0,
        };
        let mut signals = PipelineSignals::new();
        let mut p = pipeline(
            &cat,
            &check,
            &del,
            &tenant,
            &endpoint,
            &mut budget,
            BTreeSet::new(),
            &mut signals,
        );

        let a = p.apply_planned(&plan("issue.create", r#"{"title":"new"}"#));
        assert!(
            matches!(a, EffectResult::Applied(_)),
            "the allowed effect applies: {a:?}"
        );
        let d = p.apply_planned(&plan("issue.delete", r#"{"title":"x"}"#));
        assert!(
            matches!(d, EffectResult::Denied(_)),
            "the disallowed effect is denied: {d:?}"
        );

        assert_eq!(signals.applied(), 1, "exactly one applied");
        assert_eq!(signals.denied(), 1, "exactly one denied");
        assert_eq!(
            endpoint.applied.borrow().len(),
            1,
            "exactly one mutation reached a subsystem"
        );
        assert_eq!(
            signals.privileged_fallback(),
            0,
            "AG-D2: 0 privileged fallback across the session"
        );
    }

    #[test]
    fn glue_effect_api_bridge_denies_the_unbound_entry() {
        let cat = Catalogue {
            defs: vec![tool_def(
                "issue.create",
                &["issue.write"],
                false,
                EffectKind::Mutate,
            )],
        };
        let check = allow_caps(&["issue.write"]);
        let del = Delegator {
            policy: vec!["issue.write".into()],
        };
        let tenant = Tenant {
            forbid: BTreeSet::new(),
        };
        let endpoint = Endpoint {
            fail: false,
            applied: RefCell::new(vec![]),
        };
        let mut budget = Budget {
            remaining: 100,
            billed: 0,
            settles: 0,
        };
        let mut signals = PipelineSignals::new();
        let p = pipeline(
            &cat,
            &check,
            &del,
            &tenant,
            &endpoint,
            &mut budget,
            BTreeSet::new(),
            &mut signals,
        );
        let bridge = EffectApiBridge::new(p);

        let carrier = encode_proposed(&plan("issue.create", r#"{"title":"x"}"#));
        let out = bridge.apply(&RunCtx::default(), carrier);
        assert!(
            matches!(out, EffectResult::Denied(ref reason) if reason.contains("signed run-token")),
            "the unbound bridge must deny: {out:?}"
        );
        assert!(endpoint.applied.borrow().is_empty());

        let bad = bridge.apply(
            &RunCtx::default(),
            ProposedEffect("garbage-no-fields".into()),
        );
        assert!(
            matches!(bad, EffectResult::Denied(ref r) if r.contains("signed run-token")),
            "{bad:?}"
        );
    }

    #[test]
    fn proposed_effect_carrier_round_trips_deterministically() {
        let mut original = plan("issue.transition", r#"{"title":"close it"}"#);
        original.field = Some(FieldId("status".into()));
        original.transition = Some(TransitionId("to_done".into()));
        let c1 = encode_proposed(&original);
        let c2 = encode_proposed(&original);
        assert_eq!(
            c1, c2,
            "the encoding is deterministic (byte-identical across calls)"
        );
        let back = decode_proposed(&c1).expect("round-trips");
        assert_eq!(back, original, "decode is the exact inverse of encode");
    }

    #[test]
    fn schema_validator_forces_each_failure() {
        let schema = r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"},"count":{"type":"integer"}}}"#;
        assert!(
            validate_schema(schema, r#"{"title":"ok"}"#).is_ok(),
            "a valid input passes"
        );
        assert!(
            validate_schema(schema, r#"{"title":"ok","count":3}"#).is_ok(),
            "a typed optional field passes"
        );
        assert!(
            validate_schema(schema, r#"{"count":3}"#).is_err(),
            "a missing required field is rejected"
        );
        assert!(
            validate_schema(schema, r#"{"title":5}"#).is_err(),
            "a mistyped required field is rejected"
        );
        assert!(
            validate_schema(schema, r#"{"title":"ok","count":"x"}"#).is_err(),
            "a mistyped optional field is rejected"
        );
        assert!(
            validate_schema(schema, r#"[1,2,3]"#).is_err(),
            "a non-object input under a type:object schema is rejected"
        );
        assert!(
            validate_schema(schema, r#"not json"#).is_err(),
            "a non-JSON input is rejected"
        );
        assert!(
            validate_schema("{}", r#"{"anything":true}"#).is_ok(),
            "an empty schema admits any valid JSON"
        );
    }

    #[test]
    fn effect_cost_total_is_saturating_and_exact() {
        assert_eq!(
            EffectCost {
                unit: "x",
                wholesale: 3,
                markup: 1
            }
            .total(),
            4
        );
        assert_eq!(
            EffectCost {
                unit: "x",
                wholesale: u64::MAX,
                markup: 1
            }
            .total(),
            u64::MAX,
            "saturates, never wraps"
        );
        assert_eq!(
            EffectCost {
                unit: "x",
                wholesale: 0,
                markup: 0
            }
            .total(),
            0
        );
    }

    #[test]
    fn pipeline_signals_accessors_are_exact() {
        let mut s = PipelineSignals::new();
        assert_eq!(s.applied(), 0);
        assert_eq!(s.denied(), 0);
        assert_eq!(s.gated(), 0);
        assert_eq!(s.privileged_fallback(), 0);
        assert_eq!(s.metered_total(), 0);
        s.applied = 2;
        s.denied = 3;
        s.gated = 4;
        s.metered_total = 11;
        assert_eq!(s.applied(), 2, "applied returns its field (kills -> 0/1)");
        assert_eq!(s.denied(), 3, "denied returns its field");
        assert_eq!(s.gated(), 4, "gated returns its field");
        assert_eq!(s.metered_total(), 11, "metered_total returns its field");
        assert_eq!(
            s.privileged_fallback(),
            0,
            "privileged_fallback is ALWAYS 0 (no fallback path)"
        );
    }

    #[test]
    fn intern_unit_rejects_unknown_dimension() {
        assert_eq!(intern_unit("issue.transition").unwrap(), "issue.transition");
        assert!(
            intern_unit("made.up.unit").is_err(),
            "an unknown dimension is rejected"
        );
    }

    #[test]
    fn bare_type_object_schema_rejects_non_object_input() {
        let schema = r#"{"type":"object"}"#;
        assert!(
            validate_schema(schema, r#"{"any":1}"#).is_ok(),
            "a type:object schema admits an object"
        );
        assert!(
            validate_schema(schema, r#"[1,2,3]"#).is_err(),
            "a type:object schema rejects an array (line-604 check)"
        );
        let no_type = r#"{"description":"free"}"#;
        assert!(
            validate_schema(no_type, r#"[1,2,3]"#).is_ok(),
            "a schema without type:object admits an array"
        );
    }

    #[test]
    fn validate_tool_arguments_enforces_the_schema_before_dispatch() {
        use myelin_agent::{ToolCall, ToolCallId};
        use serde_json::json;

        let def = tool_def("create_issue", &["issue.write"], false, EffectKind::Mutate);

        assert!(validate_tool_arguments(&def, &json!({"title": "CI is red"})).is_ok());
        assert!(validate_tool_arguments(&def, &json!({})).is_err());
        assert!(validate_tool_arguments(&def, &json!({"title": 7})).is_err());
        assert!(validate_tool_arguments(&def, &serde_json::Value::Null).is_err());

        struct Cat {
            defs: Vec<ToolDef>,
        }
        impl ToolSurface for Cat {
            fn register_tool(&mut self, d: ToolDef) {
                self.defs.push(d);
            }
            fn resolve(&self, name: &ToolName) -> Option<&ToolDef> {
                self.defs.iter().find(|d| &d.name == name)
            }
        }
        let cat = Cat { defs: vec![def] };
        let good = ToolCall {
            id: ToolCallId("c1".into()),
            name: ToolName("create_issue".into()),
            arguments: json!({"title": "ok"}),
        };
        assert!(validate_call(&cat, &good).is_ok());
        let unknown = ToolCall {
            id: ToolCallId("c2".into()),
            name: ToolName("no_such_tool".into()),
            arguments: json!({}),
        };
        assert!(validate_call(&cat, &unknown).is_err());
    }

    #[test]
    fn json_type_matches_is_exact_per_type() {
        use serde_json::json;
        assert!(json_type_matches("object", &json!({"a":1})));
        assert!(json_type_matches("array", &json!([1, 2])));
        assert!(json_type_matches("string", &json!("s")));
        assert!(json_type_matches("boolean", &json!(true)));
        assert!(json_type_matches("number", &json!(1.5)));
        assert!(json_type_matches("integer", &json!(7)));
        assert!(json_type_matches("null", &json!(null)));
        assert!(
            !json_type_matches("object", &json!([1])),
            "object arm rejects an array"
        );
        assert!(
            !json_type_matches("array", &json!({"a":1})),
            "array arm rejects an object"
        );
        assert!(
            !json_type_matches("boolean", &json!(1)),
            "boolean arm rejects a number"
        );
        assert!(
            !json_type_matches("number", &json!("x")),
            "number arm rejects a string"
        );
        assert!(
            !json_type_matches("null", &json!(0)),
            "null arm rejects a number"
        );
        assert!(
            json_type_matches("integer", &json!(-3)),
            "a negative i64 is an integer"
        );
        assert!(
            json_type_matches("integer", &json!(u64::MAX)),
            "a large u64 is an integer"
        );
        assert!(
            !json_type_matches("integer", &json!(1.5)),
            "a float is NOT an integer"
        );
        assert!(json_type_matches("made-up-type", &json!("anything")));
    }

    #[test]
    fn json_type_name_is_exact_per_value() {
        use serde_json::json;
        assert_eq!(json_type_name(&json!(null)), "null");
        assert_eq!(json_type_name(&json!(true)), "boolean");
        assert_eq!(json_type_name(&json!(3)), "number");
        assert_eq!(json_type_name(&json!("s")), "string");
        assert_eq!(json_type_name(&json!([1])), "array");
        assert_eq!(json_type_name(&json!({"a":1})), "object");
    }

    #[test]
    fn privileged_fallback_stays_zero_across_every_outcome() {
        let cat = Catalogue {
            defs: vec![
                tool_def("issue.create", &["issue.write"], false, EffectKind::Mutate),
                tool_def("git.merge", &["git.merge"], true, EffectKind::Mutate),
                tool_def("issue.delete", &["issue.delete"], false, EffectKind::Mutate),
            ],
        };
        let check = allow_caps(&["issue.write", "git.merge", "issue.delete"]);
        let del = Delegator {
            policy: vec!["issue.write".into(), "git.merge".into()],
        };
        let tenant = Tenant {
            forbid: BTreeSet::new(),
        };
        let endpoint = Endpoint {
            fail: false,
            applied: RefCell::new(vec![]),
        };
        let mut budget = Budget {
            remaining: 100,
            billed: 0,
            settles: 0,
        };
        let mut signals = PipelineSignals::new();
        let mut p = pipeline(
            &cat,
            &check,
            &del,
            &tenant,
            &endpoint,
            &mut budget,
            BTreeSet::new(),
            &mut signals,
        );

        let _ = p.apply_planned(&plan("issue.create", r#"{"title":"a"}"#));
        let _ = p.apply_planned(&plan("git.merge", r#"{"title":"b"}"#));
        let _ = p.apply_planned(&plan("issue.delete", r#"{"title":"c"}"#));
        assert_eq!(signals.applied(), 1);
        assert_eq!(signals.gated(), 1);
        assert_eq!(signals.denied(), 1);
        assert_eq!(
            signals.privileged_fallback(),
            0,
            "AG-D2: 0 privileged fallback - there is NO fallback code path"
        );
    }
}
