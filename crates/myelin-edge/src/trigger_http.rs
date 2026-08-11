use std::sync::Arc;

use myelin_identity::{Literal, ObjectType};
use myelin_identity_service::{AgentRegistryError, PgAgentRegistry};
use myelin_notif::pg_inbox::{InboxReadScope, PgInboxStore};
use myelin_query::{CmpOp, EventMatcher, Expr, Predicate, QueryAst};
use myelin_storage::{
    AgentTraceAvailability, AgentTriggerApprovalDecision, AgentTriggerCapacityScope,
    AgentTriggerFiringState, AgentTriggerLifecycleAction, ChangeAgentTriggerApprovalOutcome,
    ChangeAgentTriggerLifecycleOutcome, CreateAgentTriggerBindingOutcome, DurableAgentTraceStore,
    DurableAgentTriggerBacking, DurableAgentTriggerBinding, DurableAgentTriggerFiring,
    EraseAgentTraceOutcome, NewAgentTriggerBinding, SubstrateProvider,
    MAX_AGENT_TRIGGER_BUDGET_MINOR_UNITS, MIN_AGENT_TRIGGER_BUDGET_MINOR_UNITS,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::types::Uuid;
use tokio::runtime::Handle;

use crate::catalogue::{page_envelope, Handler, HandlerCtx, MAX_PAGE_LIMIT};
use crate::error::EdgeError;
use crate::gateway::GatewayBuilder;
use crate::request::EdgeResponse;
use crate::runtime::drive_edge_future;
use crate::trigger_lifecycle::TriggerLifecycle;
use crate::Method;

const MAX_TRIGGER_JSON_BYTES: usize = 16 * 1024;
const MAX_TRIGGER_FILTER_BYTES: usize = 4 * 1024;

#[derive(Clone)]
struct TriggerHttpApi {
    backing: DurableAgentTriggerBacking,
    lifecycle: TriggerLifecycle,
    agents: PgAgentRegistry,
    traces: DurableAgentTraceStore,
    inbox: Arc<PgInboxStore>,
    runtime: Handle,
}

impl TriggerHttpApi {
    fn drive<F, T>(&self, future: F) -> Result<T, EdgeError>
    where
        F: std::future::Future<Output = T>,
    {
        drive_edge_future(&self.runtime, future, "trigger HTTP")
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateTriggerBody {
    event_type: String,
    #[serde(default)]
    subject_type: Option<String>,
    #[serde(default)]
    source_branch: Option<String>,
    #[serde(default)]
    filter: Option<String>,
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
        let agent = self
            .api
            .drive(self.api.agents.get(ctx.principal, &body.run_as_agent_id))?
            .map_err(map_agent_registry_error)?;
        validate_delegation_caveats(&body.delegation_caveats, &agent.grants)?;
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
            CreateAgentTriggerBindingOutcome::CapacityReached(scope) => {
                return Err(trigger_capacity_error(scope))
            }
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

struct TriggerGetHandler {
    api: TriggerHttpApi,
}

struct TriggerFiringListHandler {
    api: TriggerHttpApi,
}

struct TriggerRunResultHandler {
    api: TriggerHttpApi,
}

struct TriggerRunResultEraseHandler {
    api: TriggerHttpApi,
}

struct TriggerLifecycleHandler {
    api: TriggerHttpApi,
    action: AgentTriggerLifecycleAction,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TriggerApprovalBody {
    event_id: String,
}

struct TriggerApprovalHandler {
    api: TriggerHttpApi,
    decision: AgentTriggerApprovalDecision,
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

impl Handler for TriggerGetHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        if !ctx.request.query.is_empty() || !ctx.request.body.is_empty() {
            return Err(EdgeError::BadRequest(
                "trigger lookup accepts no query parameters or request body".into(),
            ));
        }
        let binding_id = parse_uuid(trigger_param(ctx)?)?;
        let binding = self
            .api
            .drive(self.api.backing.get_for_owner(
                &ctx.principal.tenant.0,
                &ctx.principal.principal_id.0,
                binding_id,
            ))?
            .map_err(|error| EdgeError::Internal(error.to_string()))?
            .ok_or_else(|| EdgeError::NotFound("trigger not found".into()))?;
        Ok(no_store(EdgeResponse::json(
            200,
            &json!({
                "trigger": binding_json(&ctx.principal.tenant.0, &binding),
            }),
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
        let run_ids = firings
            .iter()
            .filter_map(|firing| firing.run_id.clone())
            .collect::<Vec<_>>();
        let availability = self
            .api
            .drive(self.api.traces.availability_for_owner(
                &ctx.principal.tenant.0,
                &ctx.principal.principal_id.0,
                binding_id,
                &run_ids,
            ))?
            .map_err(|error| EdgeError::Internal(error.to_string()))?;
        let items = firings
            .iter()
            .map(|firing| {
                let result = firing
                    .run_id
                    .as_ref()
                    .and_then(|run_id| availability.get(run_id))
                    .copied();
                firing_json(&ctx.principal.tenant.0, firing, result)
            })
            .collect::<Vec<_>>();
        Ok(no_store(EdgeResponse::json(
            200,
            &page_envelope(json!(items), next, limit),
        )))
    }
}

impl Handler for TriggerRunResultHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        if !ctx.request.query.is_empty() || !ctx.request.body.is_empty() {
            return Err(EdgeError::BadRequest(
                "agent result lookup accepts no query parameters or request body".into(),
            ));
        }
        let binding_id = parse_uuid(trigger_param(ctx)?)?;
        let run_id = parse_uuid(run_param(ctx)?)?.to_string();
        let result = self
            .api
            .drive(self.api.traces.fetch_for_owner(
                &ctx.principal.tenant.0,
                &ctx.principal.principal_id.0,
                binding_id,
                &run_id,
            ))?
            .map_err(|error| EdgeError::Internal(error.to_string()))?
            .ok_or_else(|| EdgeError::NotFound("agent run result not found".into()))?;
        Ok(no_store(EdgeResponse::json(
            200,
            &json!({
                "result": {
                    "run_id": result.run_id,
                    "run_ref": format!(
                        "myelin://{}/agent/run/{}",
                        ctx.principal.tenant.0,
                        result.run_id,
                    ),
                    "trace_ref": result.artifact_ref,
                    "agent_principal": result.agent_principal,
                    "answer": result.answer,
                    "charged_micro": result.charged_micro,
                    "recorded_at": result.created_at,
                },
            }),
        )))
    }
}

impl Handler for TriggerRunResultEraseHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        if !ctx.request.query.is_empty() {
            return Err(EdgeError::BadRequest(
                "agent result erasure accepts no query parameters".into(),
            ));
        }
        crate::request::require_empty_json_object(
            &ctx.request.body,
            "agent result erasure",
            MAX_TRIGGER_JSON_BYTES,
        )?;
        let binding_id = parse_uuid(trigger_param(ctx)?)?;
        let run_id = parse_uuid(run_param(ctx)?)?.to_string();
        let receipt = match self
            .api
            .drive(self.api.traces.erase_for_owner(
                &ctx.principal.tenant.0,
                &ctx.principal.principal_id.0,
                binding_id,
                &run_id,
            ))?
            .map_err(|error| EdgeError::Internal(error.to_string()))?
        {
            EraseAgentTraceOutcome::Erased(receipt) => receipt,
            EraseAgentTraceOutcome::NotFound => {
                return Err(EdgeError::NotFound("agent run result not found".into()))
            }
        };
        Ok(no_store(EdgeResponse::json(
            200,
            &json!({
                "erasure": {
                    "run_id": run_id,
                    "run_ref": format!(
                        "myelin://{}/agent/run/{run_id}",
                        ctx.principal.tenant.0,
                    ),
                    "trace_ref": receipt.artifact_ref,
                    "erased": true,
                    "already_erased": receipt.already_erased,
                    "available_results": 0,
                    "recreation_blocked": true,
                },
            }),
        )))
    }
}

impl Handler for TriggerLifecycleHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        if !ctx.request.query.is_empty() {
            return Err(EdgeError::BadRequest(
                "trigger lifecycle accepts no query parameters".into(),
            ));
        }
        crate::request::require_empty_json_object(
            &ctx.request.body,
            "trigger lifecycle",
            MAX_TRIGGER_JSON_BYTES,
        )?;
        let binding_id = parse_uuid(trigger_param(ctx)?)?;
        let outcome = self
            .api
            .drive(self.api.lifecycle.change(
                &ctx.principal.tenant,
                &ctx.principal.principal_id.0,
                binding_id,
                self.action,
            ))?
            .map_err(|error| EdgeError::Internal(error.to_string()))?;
        let outcome = match outcome {
            ChangeAgentTriggerLifecycleOutcome::Complete(outcome) => outcome,
            ChangeAgentTriggerLifecycleOutcome::CapacityReached(scope) => {
                return Err(trigger_capacity_error(scope))
            }
            ChangeAgentTriggerLifecycleOutcome::NotFound => {
                return Err(EdgeError::NotFound("trigger not found".into()))
            }
            ChangeAgentTriggerLifecycleOutcome::InvalidTransition => {
                return Err(EdgeError::Conflict(
                    "trigger lifecycle transition is not allowed".into(),
                ))
            }
        };
        Ok(no_store(EdgeResponse::json(
            200,
            &json!({
                "action": lifecycle_action_token(self.action),
                "changed": outcome.changed,
                "canceled_firings": outcome.canceled_firings,
                "durable": true,
                "trigger": binding_json(&ctx.principal.tenant.0, &outcome.binding),
            }),
        )))
    }
}

impl Handler for TriggerApprovalHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        if !ctx.request.query.is_empty() {
            return Err(EdgeError::BadRequest(
                "trigger approval accepts no query parameters".into(),
            ));
        }
        let body = parse_approval_body(&ctx.request.body)?;
        let binding_id = parse_uuid(trigger_param(ctx)?)?;
        let outcome = self
            .api
            .drive(self.api.backing.change_firing_approval(
                &ctx.principal.tenant.0,
                &ctx.principal.principal_id.0,
                binding_id,
                &body.event_id,
                self.decision,
            ))?
            .map_err(|error| EdgeError::Internal(error.to_string()))?;
        let outcome = match outcome {
            ChangeAgentTriggerApprovalOutcome::Complete(outcome) => outcome,
            ChangeAgentTriggerApprovalOutcome::NotFound => {
                return Err(EdgeError::NotFound("trigger firing not found".into()))
            }
            ChangeAgentTriggerApprovalOutcome::InvalidTransition => {
                return Err(EdgeError::Conflict(
                    "trigger firing approval transition is not allowed".into(),
                ))
            }
        };
        let approval_item_id = myelin_notif::automation_approval_item_id(
            &ctx.principal.tenant,
            &ctx.principal.principal_id.0,
            &binding_id.to_string(),
            &body.event_id,
        );
        self.api
            .drive(self.api.inbox.complete_if_present(
                &InboxReadScope {
                    tenant: ctx.principal.tenant.clone(),
                    region: ctx.principal.region.clone(),
                    recipient: ctx.principal.principal_id.0.clone(),
                },
                &approval_item_id,
            ))?
            .map_err(|_| {
                EdgeError::Unavailable(
                    "automation approval inbox is temporarily unavailable".into(),
                )
            })?;
        Ok(no_store(EdgeResponse::json(
            200,
            &json!({
                "action": approval_action_token(self.decision),
                "changed": outcome.changed,
                "durable": true,
                "firing": firing_json(&ctx.principal.tenant.0, &outcome.firing, None),
            }),
        )))
    }
}

pub fn register_triggers(
    builder: GatewayBuilder,
    provider: SubstrateProvider,
    backing: DurableAgentTriggerBacking,
    agents: PgAgentRegistry,
    traces: DurableAgentTraceStore,
    inbox: Arc<PgInboxStore>,
    runtime: Handle,
) -> GatewayBuilder {
    let lifecycle =
        TriggerLifecycle::new(provider, backing.clone(), inbox.clone(), runtime.clone());
    let api = TriggerHttpApi {
        backing,
        lifecycle,
        agents,
        traces,
        inbox,
        runtime,
    };
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
            "/v1/triggers/{trigger}",
            "identity.triggers.list",
            Arc::new(TriggerGetHandler { api: api.clone() }),
        )
        .route(
            Method::Get,
            "/v1/triggers/{trigger}/firings",
            "identity.triggers.list",
            Arc::new(TriggerFiringListHandler { api: api.clone() }),
        )
        .route(
            Method::Get,
            "/v1/triggers/{trigger}/runs/{run}/result",
            "identity.triggers.list",
            Arc::new(TriggerRunResultHandler { api: api.clone() }),
        )
        .route(
            Method::Post,
            "/v1/triggers/{trigger}/runs/{run}/result/erase",
            "identity.trigger.result.erase",
            Arc::new(TriggerRunResultEraseHandler { api: api.clone() }),
        )
        .route(
            Method::Post,
            "/v1/triggers/{trigger}/pause",
            "identity.trigger.pause",
            Arc::new(TriggerLifecycleHandler {
                api: api.clone(),
                action: AgentTriggerLifecycleAction::Pause,
            }),
        )
        .route(
            Method::Post,
            "/v1/triggers/{trigger}/resume",
            "identity.trigger.resume",
            Arc::new(TriggerLifecycleHandler {
                api: api.clone(),
                action: AgentTriggerLifecycleAction::Resume,
            }),
        )
        .route(
            Method::Post,
            "/v1/triggers/{trigger}/disable",
            "identity.trigger.disable",
            Arc::new(TriggerLifecycleHandler {
                api: api.clone(),
                action: AgentTriggerLifecycleAction::Disable,
            }),
        )
        .route(
            Method::Post,
            "/v1/triggers/{trigger}/firings/approve",
            "identity.trigger.firing.approve",
            Arc::new(TriggerApprovalHandler {
                api: api.clone(),
                decision: AgentTriggerApprovalDecision::Approve,
            }),
        )
        .route(
            Method::Post,
            "/v1/triggers/{trigger}/firings/reject",
            "identity.trigger.firing.reject",
            Arc::new(TriggerApprovalHandler {
                api,
                decision: AgentTriggerApprovalDecision::Reject,
            }),
        )
}

fn validate_delegation_caveats(
    caveats: &[String],
    delegated_capabilities: &[String],
) -> Result<(), EdgeError> {
    let mut distinct = std::collections::BTreeSet::new();
    for caveat in caveats {
        if !distinct.insert(caveat) {
            return Err(EdgeError::BadRequest(format!(
                "delegation caveat `{caveat}` is duplicated"
            )));
        }
        if delegated_capabilities
            .iter()
            .any(|capability| capability == caveat)
        {
            continue;
        }
        if let Some(repository) = caveat.strip_prefix("repo:") {
            if !repository.contains('#')
                && myelin_git::gix_backend::validate_repo_slug(repository).is_ok()
            {
                continue;
            }
        }
        return Err(EdgeError::BadRequest(format!(
            "delegation caveat `{caveat}` must name one of the agent's delegated capabilities or a `repo:<slug>` scope"
        )));
    }
    Ok(())
}

fn map_agent_registry_error(error: AgentRegistryError) -> EdgeError {
    match error {
        AgentRegistryError::BadInput(reason) => EdgeError::BadRequest(reason),
        AgentRegistryError::NotFound => {
            EdgeError::Conflict("the selected run-as agent is unavailable".into())
        }
        AgentRegistryError::Conflict(reason) | AgentRegistryError::Policy(reason) => {
            EdgeError::Conflict(reason)
        }
        AgentRegistryError::Storage(_) => {
            EdgeError::Unavailable("the run-as agent could not be verified".into())
        }
    }
}

fn trigger_capacity_error(scope: AgentTriggerCapacityScope) -> EdgeError {
    let message = match scope {
        AgentTriggerCapacityScope::OwnerEvent => {
            "active automation limit reached for this event; pause or disable one of your automations before retrying"
        }
        AgentTriggerCapacityScope::TenantEvent => {
            "tenant automation limit reached for this event; an owner must pause or disable an automation before retrying"
        }
    };
    EdgeError::Conflict(message.into())
}

fn lifecycle_action_token(action: AgentTriggerLifecycleAction) -> &'static str {
    match action {
        AgentTriggerLifecycleAction::Pause => "pause",
        AgentTriggerLifecycleAction::Resume => "resume",
        AgentTriggerLifecycleAction::Disable => "disable",
    }
}

fn approval_action_token(decision: AgentTriggerApprovalDecision) -> &'static str {
    match decision {
        AgentTriggerApprovalDecision::Approve => "approve",
        AgentTriggerApprovalDecision::Reject => "reject",
    }
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

    let matcher = compile_matcher(
        &body.event_type,
        body.subject_type.as_deref(),
        body.source_branch.as_deref(),
        body.filter.as_deref(),
    )?;
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
    subject_type: Option<&str>,
    source_branch: Option<&str>,
    filter: Option<&str>,
) -> Result<EventMatcher, EdgeError> {
    if event_type.len() > 255 || myelin_events::validate_event_type(event_type).is_err() {
        return Err(EdgeError::BadRequest(
            "event_type must be a bounded canonical Myelin event name".into(),
        ));
    }
    let subject_type = resolve_subject_type(event_type, subject_type)?;
    let mut condition_sources = vec![format!("event.type == {}", quote_query_string(event_type))];
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
        condition_sources.push(format!(
            "payload.source_ref == {}",
            quote_query_string(&source_ref)
        ));
        predicates.push(Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: Expr::Var("payload.source_ref".into()),
            rhs: Expr::Lit(Literal::Str(source_ref)),
        });
    }
    if let Some(filter) = filter {
        if filter.is_empty() || filter.len() > MAX_TRIGGER_FILTER_BYTES || filter.trim() != filter {
            return Err(EdgeError::BadRequest(format!(
                "filter must contain 1..={MAX_TRIGGER_FILTER_BYTES} bytes without surrounding whitespace"
            )));
        }
        predicates.push(myelin_query::parse_predicate(filter).map_err(|error| {
            EdgeError::BadRequest(format!("filter is not a valid Myelin query: {error}"))
        })?);
        condition_sources.push(format!("({filter})"));
    }
    let predicate = Predicate::And(predicates);
    QueryAst::validate(&predicate)
        .map_err(|error| EdgeError::BadRequest(format!("invalid trigger matcher: {error}")))?;
    Ok(EventMatcher::new(
        ObjectType(subject_type),
        QueryAst::compiled_with_source(predicate, condition_sources.join(" AND ")),
    ))
}

fn quote_query_string(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}

fn resolve_subject_type(event_type: &str, explicit: Option<&str>) -> Result<String, EdgeError> {
    myelin_events::resolve_automation_subject_type(event_type, explicit)
        .map_err(|error| EdgeError::BadRequest(error.to_string()))
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

fn parse_approval_body(bytes: &[u8]) -> Result<TriggerApprovalBody, EdgeError> {
    if bytes.len() > MAX_TRIGGER_JSON_BYTES {
        return Err(EdgeError::PayloadTooLarge(format!(
            "trigger approval body exceeds {MAX_TRIGGER_JSON_BYTES} bytes"
        )));
    }
    let body: TriggerApprovalBody = serde_json::from_slice(bytes).map_err(|error| {
        EdgeError::BadRequest(format!("invalid trigger approval body: {error}"))
    })?;
    if body.event_id.is_empty()
        || body.event_id.len() > 255
        || body.event_id.trim() != body.event_id
        || body.event_id.chars().any(char::is_control)
    {
        return Err(EdgeError::BadRequest(
            "event_id must contain 1..=255 bytes without surrounding whitespace or control characters"
                .into(),
        ));
    }
    Ok(body)
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
        "subject_type": binding.matcher.get("object_type").and_then(Value::as_str),
        "condition": binding
            .matcher
            .pointer("/predicate/raw")
            .and_then(Value::as_str)
            .filter(|condition| !condition.is_empty()),
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

fn firing_json(
    tenant: &str,
    firing: &DurableAgentTriggerFiring,
    result: Option<AgentTraceAvailability>,
) -> Value {
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
        "result_state": result.map(AgentTraceAvailability::token),
        "terminal_reason": firing.terminal_reason,
        "approval": firing.approval_decision.map(|decision| json!({
            "decision": decision.token(),
            "decided_by": firing.approval_decided_by,
            "decided_at": firing.approval_decided_at,
        })),
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

fn run_param<'a>(ctx: &'a HandlerCtx<'_>) -> Result<&'a str, EdgeError> {
    ctx.params
        .get("run")
        .map(String::as_str)
        .ok_or_else(|| EdgeError::BadRequest("route did not bind a run id".into()))
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
    fn automation_capacity_conflicts_name_the_safe_next_action() {
        assert_eq!(
            trigger_capacity_error(AgentTriggerCapacityScope::OwnerEvent),
            EdgeError::Conflict(
                "active automation limit reached for this event; pause or disable one of your automations before retrying".into()
            )
        );
        assert_eq!(
            trigger_capacity_error(AgentTriggerCapacityScope::TenantEvent),
            EdgeError::Conflict(
                "tenant automation limit reached for this event; an owner must pause or disable an automation before retrying".into()
            )
        );
    }

    #[test]
    fn red_mainline_trigger_is_exactly_red_and_exactly_mainline() {
        let matcher = compile_matcher("ci.run.failed", None, Some("main"), None).unwrap();

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
            compile_matcher("ci.run.failed", None, Some("main"), None).unwrap(),
            compile_matcher("ci.run.failed", None, Some("refs/heads/main"), None).unwrap(),
            "friendly and fully-qualified mainline names compile to one durable intent"
        );
        assert!(compile_matcher("ci.run.failed", None, Some("refs/tags/release"), None).is_err());
        assert!(compile_matcher("ci.run.failed", None, Some("feature/../main"), None).is_err());
    }

    #[test]
    fn event_names_share_the_platform_taxonomy_instead_of_a_trigger_only_dialect() {
        assert!(compile_matcher("ci.result", Some("run"), None, None).is_ok());
        assert!(compile_matcher("ci.result", None, None, None).is_err());
        assert!(compile_matcher("CI.run.failed", None, None, None).is_err());
        assert!(compile_matcher("unknown.run.failed", None, None, None).is_err());
    }

    #[test]
    fn matcher_subjects_follow_the_event_artifact_instead_of_silently_assuming_ci() {
        let issue = compile_matcher("issue.issue.updated", None, None, None).unwrap();
        assert_eq!(issue.object_type().0, "issue");
        assert!(compile_matcher("issue.issue.updated", Some("run"), None, None).is_err());
        assert!(compile_matcher("ci.deployment.finished", None, None, None).is_err());
    }

    #[test]
    fn one_bounded_query_language_filters_every_event_domain() {
        let matcher = compile_matcher(
            "issue.issue.updated",
            None,
            None,
            Some("payload.change_kind == 'ownership' AND event.depth <= 2"),
        )
        .unwrap();
        let mut event = ci_event("issue.issue.updated", "refs/heads/main");
        event.subject = ArtifactRef("myelin://acme/issue/issue/ENG-41".into());
        event.payload = json!({ "change_kind": "ownership" });

        assert_eq!(
            matcher.matches(&event, &SetExpr::All, &no_relation),
            Ok(true),
            "the shared QueryAst can express useful issue intent without an issue-only DSL"
        );
        event.payload = json!({ "change_kind": "title" });
        assert_eq!(
            matcher.matches(&event, &SetExpr::All, &no_relation),
            Ok(false)
        );
        assert_eq!(
            matcher.predicate().source(),
            concat!(
                "event.type == 'issue.issue.updated' AND ",
                "(payload.change_kind == 'ownership' AND event.depth <= 2)"
            ),
            "operators can rediscover the complete effective condition"
        );

        assert!(compile_matcher(
            "issue.issue.updated",
            None,
            None,
            Some(" payload.change_kind == 'ownership'")
        )
        .is_err());
        assert!(compile_matcher(
            "issue.issue.updated",
            None,
            None,
            Some("payload.change_kind = 'ownership'")
        )
        .is_err());
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

    #[test]
    fn automation_caveats_name_real_capabilities_or_exact_repositories() {
        let grants = vec!["issue.create".into(), "pull_request.merge".into()];
        assert!(validate_delegation_caveats(
            &["issue.create".into(), "repo:platform/api".into()],
            &grants,
        )
        .is_ok());

        for caveats in [
            vec!["issue:create".into()],
            vec!["repo:../payroll".into()],
            vec!["run:another-run".into()],
            vec!["issue.create".into(), "issue.create".into()],
        ] {
            assert!(
                validate_delegation_caveats(&caveats, &grants).is_err(),
                "accepted misleading caveats {caveats:?}",
            );
        }
    }

    #[test]
    fn approval_names_one_exact_event_without_accepting_shape_drift() {
        assert_eq!(
            parse_approval_body(br#"{"event_id":"ci-failed:one/two"}"#)
                .unwrap()
                .event_id,
            "ci-failed:one/two"
        );
        assert!(parse_approval_body(br#"{"event_id":" ci-failed"}"#).is_err());
        assert!(parse_approval_body(br#"{"event_id":"ci-failed","approve":true}"#).is_err());
    }
}
