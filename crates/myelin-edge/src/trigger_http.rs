use std::future::Future;
use std::sync::Arc;

use myelin_identity::{Literal, ObjectType};
use myelin_query::{CmpOp, EventMatcher, Expr, Predicate};
use myelin_storage::{
    AgentTriggerFiringState, CreateAgentTriggerBindingOutcome, DurableAgentTriggerBacking,
    DurableAgentTriggerBinding, DurableAgentTriggerFiring, NewAgentTriggerBinding,
    MAX_AGENT_TRIGGER_BUDGET_MINOR_UNITS, MIN_AGENT_TRIGGER_BUDGET_MINOR_UNITS,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::types::Uuid;
use tokio::runtime::{Handle, RuntimeFlavor};

use crate::catalogue::{page_envelope, Handler, HandlerCtx, MAX_PAGE_LIMIT};
use crate::error::EdgeError;
use crate::gateway::GatewayBuilder;
use crate::request::EdgeResponse;
use crate::Method;

const MAX_TRIGGER_JSON_BYTES: usize = 16 * 1024;

#[derive(Clone)]
struct TriggerHttpApi {
    backing: DurableAgentTriggerBacking,
    runtime: Handle,
}

impl TriggerHttpApi {
    fn drive<F, T>(&self, future: F) -> Result<T, EdgeError>
    where
        F: Future<Output = T>,
    {
        match Handle::try_current() {
            Ok(current) if current.runtime_flavor() == RuntimeFlavor::MultiThread => {
                Ok(tokio::task::block_in_place(|| {
                    self.runtime.block_on(future)
                }))
            }
            Ok(_) => Err(EdgeError::Internal(
                "trigger HTTP requires the Edge multi-thread runtime".into(),
            )),
            Err(_) => Ok(self.runtime.block_on(future)),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateTriggerBody {
    event_type: String,
    #[serde(default)]
    source_branch: Option<String>,
    run_as_agent_id: String,
    task: String,
    budget_minor_units: u64,
    max_firings: u64,
    #[serde(default = "default_max_causal_depth")]
    max_causal_depth: u32,
    #[serde(default)]
    delegation_caveats: Vec<String>,
    #[serde(default = "default_true")]
    require_no_personal_data: bool,
    #[serde(default)]
    require_human_approval: bool,
}

fn default_max_causal_depth() -> u32 {
    4
}

fn default_true() -> bool {
    true
}

struct TriggerCreateHandler {
    api: TriggerHttpApi,
}

impl Handler for TriggerCreateHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        if !ctx.request.query.is_empty() {
            return Err(EdgeError::BadRequest(
                "trigger creation accepts no query parameters".into(),
            ));
        }
        let body = parse_create_body(&ctx.request.body)?;
        let client_nonce = ctx
            .request
            .stable_idempotency_nonce(&ctx.principal.principal_id.0)?;
        let proposal = create_proposal(ctx, body, client_nonce)?;
        let outcome = self
            .api
            .drive(self.api.backing.create(&ctx.principal.tenant.0, proposal))?
            .map_err(|error| EdgeError::Internal(error.to_string()))?;
        let (status, created, binding) = match outcome {
            CreateAgentTriggerBindingOutcome::Created(binding) => (201, true, binding),
            CreateAgentTriggerBindingOutcome::Replayed(binding) => (200, false, binding),
            CreateAgentTriggerBindingOutcome::Conflict => {
                return Err(EdgeError::Conflict(
                    "idempotency key was already used for a different trigger".into(),
                ))
            }
            CreateAgentTriggerBindingOutcome::OwnerUnavailable => {
                return Err(EdgeError::Forbidden(
                    "only an active human can own an agent trigger".into(),
                ))
            }
            CreateAgentTriggerBindingOutcome::AgentUnavailable => {
                return Err(EdgeError::Conflict(
                    "the selected run-as agent is not active".into(),
                ))
            }
        };
        Ok(no_store(EdgeResponse::json(
            status,
            &json!({
                "created": created,
                "durable": true,
                "trigger": binding_json(&ctx.principal.tenant.0, &binding),
            }),
        )))
    }
}

struct TriggerListHandler {
    api: TriggerHttpApi,
}

struct TriggerFiringListHandler {
    api: TriggerHttpApi,
}

impl Handler for TriggerListHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        if !ctx.request.body.is_empty() {
            return Err(EdgeError::BadRequest(
                "trigger listing accepts no request body".into(),
            ));
        }
        let cursor = ctx.page.cursor.as_deref().map(parse_uuid).transpose()?;
        let limit = ctx.page.limit.min(MAX_PAGE_LIMIT);
        let mut bindings = self
            .api
            .drive(self.api.backing.list_for_owner(
                &ctx.principal.tenant.0,
                &ctx.principal.principal_id.0,
                cursor,
                limit as u32 + 1,
            ))?
            .map_err(|error| EdgeError::Internal(error.to_string()))?;
        let has_more = bindings.len() > limit;
        bindings.truncate(limit);
        let next = has_more
            .then(|| bindings.last().map(|binding| binding.binding_id.clone()))
            .flatten();
        let items = bindings
            .iter()
            .map(|binding| binding_json(&ctx.principal.tenant.0, binding))
            .collect::<Vec<_>>();
        Ok(no_store(EdgeResponse::json(
            200,
            &page_envelope(json!(items), next, limit),
        )))
    }
}

impl Handler for TriggerFiringListHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        if !ctx.request.body.is_empty() {
            return Err(EdgeError::BadRequest(
                "trigger firing listing accepts no request body".into(),
            ));
        }
        let binding_id = parse_uuid(trigger_param(ctx)?)?;
        let limit = ctx.page.limit.min(MAX_PAGE_LIMIT);
        let mut firings = self
            .api
            .drive(self.api.backing.list_firings_for_owner(
                &ctx.principal.tenant.0,
                &ctx.principal.principal_id.0,
                binding_id,
                ctx.page.cursor.as_deref(),
                limit as u32 + 1,
            ))?
            .map_err(|error| EdgeError::Internal(error.to_string()))?;
        let has_more = firings.len() > limit;
        firings.truncate(limit);
        let next = has_more
            .then(|| firings.last().map(|firing| firing.event_id.clone()))
            .flatten();
        let items = firings
            .iter()
            .map(|firing| firing_json(&ctx.principal.tenant.0, firing))
            .collect::<Vec<_>>();
        Ok(no_store(EdgeResponse::json(
            200,
            &page_envelope(json!(items), next, limit),
        )))
    }
}

pub fn register_triggers(
    builder: GatewayBuilder,
    backing: DurableAgentTriggerBacking,
    runtime: Handle,
) -> GatewayBuilder {
    let api = TriggerHttpApi { backing, runtime };
    builder
        .route(
            Method::Get,
            "/v1/triggers",
            "identity.triggers.list",
            Arc::new(TriggerListHandler { api: api.clone() }),
        )
        .route(
            Method::Post,
            "/v1/triggers",
            "identity.trigger.create",
            Arc::new(TriggerCreateHandler { api: api.clone() }),
        )
        .route(
            Method::Get,
            "/v1/triggers/{trigger}/firings",
            "identity.triggers.list",
            Arc::new(TriggerFiringListHandler { api }),
        )
}

fn create_proposal(
    ctx: &HandlerCtx<'_>,
    body: CreateTriggerBody,
    client_nonce: String,
) -> Result<NewAgentTriggerBinding, EdgeError> {
    let agent_id = parse_uuid(&body.run_as_agent_id)?;
    if body.task.is_empty() || body.task.len() > 4096 || body.task.trim() != body.task {
        return Err(EdgeError::BadRequest(
            "task must contain 1..=4096 bytes without surrounding whitespace".into(),
        ));
    }
    validate_budget_minor_units(body.budget_minor_units)?;
    if !(1..=1_000_000).contains(&body.max_firings) || body.max_causal_depth > 64 {
        return Err(EdgeError::BadRequest(
            "trigger budget or causal-depth limit is outside its supported bound".into(),
        ));
    }
    if body.delegation_caveats.len() > 128
        || body
            .delegation_caveats
            .iter()
            .any(|caveat| caveat.is_empty() || caveat.len() > 255)
    {
        return Err(EdgeError::BadRequest(
            "delegation caveats must be 0..=128 bounded non-empty tokens".into(),
        ));
    }

    let matcher = compile_matcher(&body.event_type, body.source_branch.as_deref())?;
    Ok(NewAgentTriggerBinding {
        binding_id: Uuid::new_v4(),
        owner_principal_id: ctx.principal.principal_id.0.clone(),
        run_as_agent_id: agent_id,
        client_nonce,
        event_type: body.event_type,
        matcher: serde_json::to_value(matcher)
            .map_err(|error| EdgeError::Internal(format!("serialize trigger matcher: {error}")))?,
        task: body.task,
        delegation_caveats: body.delegation_caveats,
        budget_minor_units: body.budget_minor_units,
        max_firings: body.max_firings,
        max_causal_depth: body.max_causal_depth,
        require_no_personal_data: body.require_no_personal_data,
        require_human_approval: body.require_human_approval,
        created_at: chrono::Utc::now(),
    })
}

fn validate_budget_minor_units(value: u64) -> Result<(), EdgeError> {
    if !(MIN_AGENT_TRIGGER_BUDGET_MINOR_UNITS..=MAX_AGENT_TRIGGER_BUDGET_MINOR_UNITS)
        .contains(&value)
    {
        return Err(EdgeError::BadRequest(format!(
            "trigger budget must be {MIN_AGENT_TRIGGER_BUDGET_MINOR_UNITS}..=\
                 {MAX_AGENT_TRIGGER_BUDGET_MINOR_UNITS} integer minor-units"
        )));
    }
    Ok(())
}

fn compile_matcher(
    event_type: &str,
    source_branch: Option<&str>,
) -> Result<EventMatcher, EdgeError> {
    if event_type.len() > 255 || myelin_events::validate_event_type(event_type).is_err() {
        return Err(EdgeError::BadRequest(
            "event_type must be a bounded canonical Myelin event name".into(),
        ));
    }
    let mut predicates = vec![Predicate::Cmp {
        op: CmpOp::Eq,
        lhs: Expr::Var("event.type".into()),
        rhs: Expr::Lit(Literal::Str(event_type.into())),
    }];
    if let Some(branch) = source_branch {
        let source_ref = match branch {
            branch if branch.starts_with("refs/heads/") => branch.to_owned(),
            branch if branch.starts_with("refs/") => {
                return Err(EdgeError::BadRequest(
                    "source_branch must name a branch, not another kind of ref".into(),
                ))
            }
            branch => format!("refs/heads/{branch}"),
        };
        let reference = myelin_git::receive_pack::RefName::new(&source_ref);
        if reference.validate().is_err() || !source_ref.starts_with("refs/heads/") {
            return Err(EdgeError::BadRequest(
                "source_branch is not a canonical branch name".into(),
            ));
        }
        predicates.push(Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: Expr::Var("payload.source_ref".into()),
            rhs: Expr::Lit(Literal::Str(source_ref)),
        });
    }
    EventMatcher::compile(ObjectType("run".into()), Predicate::And(predicates))
        .map_err(|error| EdgeError::BadRequest(format!("invalid trigger matcher: {error}")))
}

fn parse_create_body(bytes: &[u8]) -> Result<CreateTriggerBody, EdgeError> {
    if bytes.len() > MAX_TRIGGER_JSON_BYTES {
        return Err(EdgeError::PayloadTooLarge(format!(
            "trigger request body exceeds {MAX_TRIGGER_JSON_BYTES} bytes"
        )));
    }
    serde_json::from_slice(bytes)
        .map_err(|error| EdgeError::BadRequest(format!("invalid trigger create body: {error}")))
}

fn parse_uuid(value: &str) -> Result<Uuid, EdgeError> {
    let parsed = Uuid::parse_str(value).map_err(|_| {
        EdgeError::BadRequest("agent and trigger ids must be lowercase UUIDs".into())
    })?;
    if parsed.to_string() == value {
        Ok(parsed)
    } else {
        Err(EdgeError::BadRequest(
            "agent and trigger ids must be lowercase UUIDs".into(),
        ))
    }
}

fn binding_json(tenant: &str, binding: &DurableAgentTriggerBinding) -> Value {
    json!({
        "id": binding.binding_id,
        "ref": format!("myelin://{tenant}/identity/trigger/{}", binding.binding_id),
        "owner_principal_id": binding.owner_principal_id,
        "run_as_agent_id": binding.run_as_agent_id,
        "event_type": binding.event_type,
        "matcher": binding.matcher,
        "task": binding.task,
        "delegation_caveats": binding.delegation_caveats,
        "budget_minor_units": binding.budget_minor_units,
        "max_firings": binding.max_firings,
        "firings_used": binding.firings_used,
        "max_causal_depth": binding.max_causal_depth,
        "require_no_personal_data": binding.require_no_personal_data,
        "require_human_approval": binding.require_human_approval,
        "state": binding.state,
        "created_at": binding.created_at,
    })
}

fn firing_json(tenant: &str, firing: &DurableAgentTriggerFiring) -> Value {
    json!({
        "event_id": firing.event_id,
        "event_type": firing.event_type,
        "trigger_ref": format!(
            "myelin://{tenant}/identity/trigger/{}",
            firing.binding_id,
        ),
        "state": firing_state_token(firing.state),
        "run_id": firing.run_id,
        "run_ref": firing.run_id.as_ref().map(|run_id| {
            format!("myelin://{tenant}/agent/run/{run_id}")
        }),
        "outcome": firing.outcome.map(|outcome| outcome.token()),
        "created_at": firing.created_at,
    })
}

fn firing_state_token(state: AgentTriggerFiringState) -> &'static str {
    match state {
        AgentTriggerFiringState::Queued => "queued",
        AgentTriggerFiringState::AwaitingApproval => "awaiting_approval",
        AgentTriggerFiringState::Claimed => "claimed",
        AgentTriggerFiringState::Started => "started",
        AgentTriggerFiringState::Terminal => "terminal",
    }
}

fn trigger_param<'a>(ctx: &'a HandlerCtx<'_>) -> Result<&'a str, EdgeError> {
    ctx.params
        .get("trigger")
        .map(String::as_str)
        .ok_or_else(|| EdgeError::BadRequest("route did not bind a trigger id".into()))
}

fn no_store(response: EdgeResponse) -> EdgeResponse {
    response.with_header("Cache-Control", "no-store")
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{
        Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventId,
        EventType, Region, TenantId, Timestamp, Visibility,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind, SetExpr};
    use myelin_query::RelMembership;

    fn no_relation(_: &RelMembership) -> bool {
        false
    }

    fn ci_event(event_type: &str, source_ref: &str) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId("01K2FIREONCE".into()),
            type_: EventType(event_type.into()),
            schema_ver: 1,
            tenant: TenantId("acme".into()),
            region: Region("eu-north".into()),
            actor: Actor(Principal::stub(
                PrincipalId("ci-controlplane".into()),
                PrincipalKind::Service,
                TenantId("acme".into()),
            )),
            subject: ArtifactRef("myelin://acme/ci/run/42".into()),
            aggregate: AggregateKey("ci-run-42".into()),
            causation_id: None,
            correlation_id: CorrelationId("push-41".into()),
            caused_by: None,
            depth: 1,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-08-10T08:00:00Z".into()),
            recorded_at: Timestamp("2026-08-10T08:00:01Z".into()),
            payload: json!({ "source_ref": source_ref }),
        }
    }

    #[test]
    fn red_mainline_trigger_is_exactly_red_and_exactly_mainline() {
        let matcher = compile_matcher("ci.run.failed", Some("main")).unwrap();

        assert_eq!(
            matcher.matches(
                &ci_event("ci.run.failed", "refs/heads/main"),
                &SetExpr::All,
                &no_relation,
            ),
            Ok(true),
            "a failed mainline run wakes the selected agent"
        );
        assert_eq!(
            matcher.matches(
                &ci_event("ci.run.succeeded", "refs/heads/main"),
                &SetExpr::All,
                &no_relation,
            ),
            Ok(false),
            "green mainline is quiet"
        );
        assert_eq!(
            matcher.matches(
                &ci_event("ci.run.failed", "refs/heads/feature/safer-parser"),
                &SetExpr::All,
                &no_relation,
            ),
            Ok(false),
            "a red feature branch does not consume the mainline budget"
        );
    }

    #[test]
    fn branch_inputs_are_canonicalized_or_refused_before_persistence() {
        assert_eq!(
            compile_matcher("ci.run.failed", Some("main")).unwrap(),
            compile_matcher("ci.run.failed", Some("refs/heads/main")).unwrap(),
            "friendly and fully-qualified mainline names compile to one durable intent"
        );
        assert!(compile_matcher("ci.run.failed", Some("refs/tags/release")).is_err());
        assert!(compile_matcher("ci.run.failed", Some("feature/../main")).is_err());
    }

    #[test]
    fn event_names_share_the_platform_taxonomy_instead_of_a_trigger_only_dialect() {
        assert!(compile_matcher("ci.result", None).is_ok());
        assert!(compile_matcher("CI.run.failed", None).is_err());
        assert!(compile_matcher("unknown.run.failed", None).is_err());
    }

    #[test]
    fn every_automation_names_a_positive_bounded_spend_budget() {
        assert!(validate_budget_minor_units(1).is_ok());
        assert!(validate_budget_minor_units(250_000).is_ok());
        assert!(validate_budget_minor_units(0).is_err());
        assert!(validate_budget_minor_units(MAX_AGENT_TRIGGER_BUDGET_MINOR_UNITS + 1).is_err());

        let missing_budget = br#"{
            "event_type":"ci.run.failed",
            "run_as_agent_id":"20000000-0000-4000-8000-000000000002",
            "task":"Triage the failure.",
            "max_firings":1
        }"#;
        assert!(parse_create_body(missing_budget).is_err());
    }
}
