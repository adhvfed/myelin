use std::sync::Arc;

use myelin_agent::{is_canonical_tool_name, EffectKind, ToolDef};
use myelin_agent_service::{catalogue_cursor, tool_ref, PlatformToolCatalogue};
use myelin_identity_service::{Authority, CredentialPurpose, VerifiedCapabilityContext};
use myelin_tenancy::TenantId;
use serde_json::{json, Value};

use crate::catalogue::{page_envelope, Handler, HandlerCtx, DEFAULT_PAGE_LIMIT, MAX_PAGE_LIMIT};
use crate::error::EdgeError;
use crate::gateway::GatewayBuilder;
use crate::request::EdgeResponse;
use crate::Method;

const MAX_TOOL_QUERY_BYTES: usize = 512;

#[derive(Clone, Debug, PartialEq, Eq)]
enum ToolQuery {
    Page {
        limit: usize,
        cursor: Option<String>,
    },
    Mcp,
}

struct ToolListHandler {
    catalogue: Arc<PlatformToolCatalogue>,
}

impl Handler for ToolListHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_empty_body(ctx)?;
        let capability = ctx.identity.capability();
        let response = match parse_tool_query(&ctx.request.query)? {
            ToolQuery::Mcp => self
                .catalogue
                .mcp_manifest_for(|definition| is_tool_permitted(capability, definition)),
            ToolQuery::Page { limit, cursor } => catalogue_page(
                &self.catalogue,
                capability,
                &ctx.principal.tenant,
                limit,
                cursor.as_deref(),
            )?,
        };
        Ok(no_store(EdgeResponse::json(200, &response)))
    }
}

struct ToolGetHandler {
    catalogue: Arc<PlatformToolCatalogue>,
}

impl Handler for ToolGetHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_empty_body(ctx)?;
        if !ctx.request.query.is_empty() {
            return Err(EdgeError::BadRequest(
                "tool lookup accepts no query parameters".into(),
            ));
        }
        let name = tool_param(ctx)?;
        let definition = self
            .catalogue
            .resolve(name)
            .filter(|definition| is_tool_permitted(ctx.identity.capability(), definition))
            .ok_or_else(|| EdgeError::NotFound("tool not found".into()))?;
        Ok(no_store(EdgeResponse::json(
            200,
            &json!({ "tool": tool_json(&ctx.principal.tenant, definition) }),
        )))
    }
}

pub fn register_tools(builder: GatewayBuilder) -> GatewayBuilder {
    let catalogue = Arc::new(
        PlatformToolCatalogue::platform()
            .expect("the built-in platform ToolDef catalogue must be valid"),
    );
    builder
        .route(
            Method::Get,
            "/v1/tools",
            "agent.tools.list",
            Arc::new(ToolListHandler {
                catalogue: catalogue.clone(),
            }),
        )
        .route(
            Method::Get,
            "/v1/tools/{tool}",
            "agent.tool.view",
            Arc::new(ToolGetHandler { catalogue }),
        )
}

fn catalogue_page(
    catalogue: &PlatformToolCatalogue,
    capability: &VerifiedCapabilityContext,
    tenant: &TenantId,
    limit: usize,
    cursor: Option<&str>,
) -> Result<Value, EdgeError> {
    let visible = catalogue
        .latest_definitions()
        .into_iter()
        .filter(|definition| is_tool_permitted(capability, definition))
        .collect::<Vec<_>>();
    let start = match cursor {
        Some(cursor) => visible
            .iter()
            .position(|definition| catalogue_cursor(definition) == cursor)
            .map(|index| index + 1)
            .ok_or_else(|| EdgeError::BadRequest("tool cursor is invalid or unavailable".into()))?,
        None => 0,
    };
    let page = visible
        .into_iter()
        .skip(start)
        .take(limit + 1)
        .collect::<Vec<_>>();
    let has_more = page.len() > limit;
    let items = page
        .iter()
        .take(limit)
        .map(|definition| tool_json(tenant, definition))
        .collect::<Vec<_>>();
    let next_cursor = has_more.then(|| catalogue_cursor(page[limit - 1]));
    Ok(page_envelope(json!(items), next_cursor, limit))
}

fn is_tool_permitted(capability: &VerifiedCapabilityContext, definition: &ToolDef) -> bool {
    matches!(
        capability.purpose,
        CredentialPurpose::HumanSession | CredentialPurpose::OperatorBootstrap
    ) || holds_every(&capability.effective_authority, &definition.required_caps)
}

fn holds_every(authority: &Authority, required: &[String]) -> bool {
    required.iter().all(|grant| authority.holds(grant))
}

fn tool_json(tenant: &TenantId, definition: &ToolDef) -> Value {
    let schema = serde_json::from_str::<Value>(&definition.input_schema)
        .expect("the platform catalogue contains only validated input schemas");
    json!({
        "name": definition.canonical_name(),
        "ref": tool_ref(tenant, definition).0,
        "subsystem": definition.subsystem,
        "version": definition.version,
        "input_schema": schema,
        "required_capabilities": definition.required_caps,
        "effect_kind": effect_kind_name(definition.effect_kind),
        "side_effecting": definition.side_effecting,
        "requires_approval": definition.requires_approval,
        "exposed_over_mcp": definition.exposed_over_mcp,
    })
}

fn effect_kind_name(kind: EffectKind) -> &'static str {
    match kind {
        EffectKind::Read => "read",
        EffectKind::Compute => "compute",
        EffectKind::Mutate => "mutate",
        EffectKind::External => "external",
    }
}

fn parse_tool_query(query: &str) -> Result<ToolQuery, EdgeError> {
    if query.len() > MAX_TOOL_QUERY_BYTES {
        return Err(EdgeError::BadRequest("tool query is too large".into()));
    }
    let mut limit = None;
    let mut cursor = None;
    let mut format = None;
    if !query.is_empty() {
        for pair in query.split('&') {
            let (name, value) = pair
                .split_once('=')
                .ok_or_else(|| EdgeError::BadRequest("malformed tool query parameter".into()))?;
            let slot = match name {
                "limit" => &mut limit,
                "cursor" => &mut cursor,
                "format" => &mut format,
                other => {
                    return Err(EdgeError::BadRequest(format!(
                        "unknown tool query parameter `{other}`"
                    )))
                }
            };
            if slot.replace(value).is_some() {
                return Err(EdgeError::BadRequest(format!(
                    "duplicate tool query parameter `{name}`"
                )));
            }
        }
    }
    if let Some(format) = format {
        if format != "mcp" {
            return Err(EdgeError::BadRequest(
                "tool format must be exactly `mcp`".into(),
            ));
        }
        if limit.is_some() || cursor.is_some() {
            return Err(EdgeError::BadRequest(
                "tool MCP format cannot be combined with pagination".into(),
            ));
        }
        return Ok(ToolQuery::Mcp);
    }
    let limit = limit
        .map(|value| {
            value
                .parse::<usize>()
                .ok()
                .filter(|parsed| parsed.to_string() == value)
                .filter(|parsed| (1..=MAX_PAGE_LIMIT).contains(parsed))
                .ok_or_else(|| {
                    EdgeError::BadRequest(
                        "tool limit must be a canonical integer between 1 and 100".into(),
                    )
                })
        })
        .transpose()?
        .unwrap_or(DEFAULT_PAGE_LIMIT);
    Ok(ToolQuery::Page {
        limit,
        cursor: cursor.map(str::to_string),
    })
}

fn tool_param<'a>(ctx: &'a HandlerCtx<'_>) -> Result<&'a str, EdgeError> {
    let name = ctx
        .params
        .get("tool")
        .map(String::as_str)
        .ok_or_else(|| EdgeError::BadRequest("route did not bind a tool name".into()))?;
    if is_canonical_tool_name(name) {
        Ok(name)
    } else {
        Err(EdgeError::BadRequest(
            "tool name must be canonical `<subsystem>.<name>`".into(),
        ))
    }
}

fn require_empty_body(ctx: &HandlerCtx<'_>) -> Result<(), EdgeError> {
    if ctx.request.body.is_empty() {
        Ok(())
    } else {
        Err(EdgeError::BadRequest(
            "tool discovery accepts no request body".into(),
        ))
    }
}

fn no_store(response: EdgeResponse) -> EdgeResponse {
    response.with_header("Cache-Control", "no-store")
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity_service::{CredentialAudience, DpopState};

    fn capability(purpose: CredentialPurpose, grants: &[&str]) -> VerifiedCapabilityContext {
        VerifiedCapabilityContext {
            purpose,
            audience: CredentialAudience::Edge,
            jti: "tool-http-test".into(),
            effective_authority: Authority::of(grants.iter().copied()),
            expires_at_unix: i64::MAX,
            dpop: DpopState::Unbound,
        }
    }

    #[test]
    fn tool_queries_are_exact_bounded_and_unambiguous() {
        assert_eq!(
            parse_tool_query("").unwrap(),
            ToolQuery::Page {
                limit: 50,
                cursor: None
            }
        );
        assert_eq!(parse_tool_query("format=mcp").unwrap(), ToolQuery::Mcp);
        for invalid in [
            "limit=0",
            "limit=01",
            "limit=101",
            "limit=2&limit=3",
            "format=mcp&limit=2",
            "format=json",
            "tenant=other",
            "cursor",
        ] {
            assert!(parse_tool_query(invalid).is_err(), "accepted `{invalid}`");
        }
    }

    #[test]
    fn delegated_catalogues_disclose_only_tools_whose_every_capability_is_held() {
        let catalogue = PlatformToolCatalogue::platform().unwrap();
        let delegated = capability(
            CredentialPurpose::AgentRun {
                run_id: "run:one".into(),
                delegation_snapshot: Some(7),
            },
            &["repo.push", "run.view"],
        );
        let page =
            catalogue_page(&catalogue, &delegated, &TenantId("acme".into()), 100, None).unwrap();
        let names = page["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(names.contains(&"git.open_pr"));
        assert!(names.contains(&"ci.read_run"));
        assert!(!names.contains(&"git.merge"));
        assert!(!names.contains(&"chat.post"));
        assert!(page["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["ref"]
                .as_str()
                .unwrap()
                .starts_with("myelin://acme/agent/tool/")));

        let unavailable = catalogue_cursor(catalogue.resolve("git.merge").unwrap());
        assert!(catalogue_page(
            &catalogue,
            &delegated,
            &TenantId("acme".into()),
            10,
            Some(&unavailable),
        )
        .is_err());
    }

    #[test]
    fn human_discovery_and_mcp_projection_share_the_same_catalogue_contract() {
        let catalogue = PlatformToolCatalogue::platform().unwrap();
        let human = capability(CredentialPurpose::HumanSession, &[]);
        let page = catalogue_page(&catalogue, &human, &TenantId("acme".into()), 1, None).unwrap();
        let first = &page["items"][0];
        assert!(is_canonical_tool_name(first["name"].as_str().unwrap()));
        assert!(first["input_schema"].is_object());
        assert!(page["page"]["next_cursor"].as_str().is_some());

        let manifest =
            catalogue.mcp_manifest_for(|definition| is_tool_permitted(&human, definition));
        assert!(manifest["tools"].as_array().unwrap().iter().all(|tool| {
            tool["inputSchema"].is_object() && tool["annotations"]["requiresApproval"].is_boolean()
        }));
    }
}
