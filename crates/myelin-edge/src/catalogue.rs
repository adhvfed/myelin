use crate::error::EdgeError;
use crate::request::{EdgeRequest, EdgeResponse};
use myelin_identity::Principal;
use myelin_identity_service::RequestIdentity;
use myelin_storage::TenantScope;
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub const API_VERSION: &str = "v1";

pub const DEFAULT_PAGE_LIMIT: usize = 50;
pub const MAX_PAGE_LIMIT: usize = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl Method {
    pub fn is_write(self) -> bool {
        !matches!(self, Method::Get)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Patch => "PATCH",
            Method::Delete => "DELETE",
        }
    }

    pub fn parse(s: &str) -> Option<Method> {
        match s.to_uppercase().as_str() {
            "GET" => Some(Method::Get),
            "POST" => Some(Method::Post),
            "PUT" => Some(Method::Put),
            "PATCH" => Some(Method::Patch),
            "DELETE" => Some(Method::Delete),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page {
    pub limit: usize,
    pub cursor: Option<String>,
}

impl Page {
    pub fn from_request(req: &EdgeRequest) -> Page {
        let limit = req
            .query_param("limit")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(DEFAULT_PAGE_LIMIT)
            .clamp(1, MAX_PAGE_LIMIT);
        Page {
            limit,
            cursor: req.query_param("cursor"),
        }
    }
}

pub fn page_envelope(items: Value, next_cursor: Option<String>, limit: usize) -> Value {
    json!({
        "items": items,
        "page": { "next_cursor": next_cursor, "limit": limit },
    })
}

pub struct HandlerCtx<'a> {
    pub identity: &'a RequestIdentity,
    pub principal: &'a Principal,
    pub scope: &'a TenantScope,
    pub params: &'a BTreeMap<String, String>,
    pub page: &'a Page,
    pub request: &'a EdgeRequest,
}

#[cfg(test)]
pub(crate) fn test_request_identity(principal: &Principal, scope: &TenantScope) -> RequestIdentity {
    RequestIdentity {
        principal: principal.clone(),
        scope: scope.clone(),
        credential: myelin_identity_service::CredentialContext::Capability(
            myelin_identity_service::VerifiedCapabilityContext {
                purpose: myelin_identity_service::CredentialPurpose::OperatorBootstrap,
                audience: myelin_identity_service::CredentialAudience::Edge,
                jti: "test-handler-context".into(),
                effective_authority: myelin_identity_service::Authority::of(["edge.operator"]),
                expires_at_unix: i64::MAX,
                dpop: myelin_identity_service::DpopState::Unbound,
            },
        ),
    }
}

pub trait Handler: Send + Sync {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_write_split_and_parse() {
        assert!(!Method::Get.is_write());
        for m in [Method::Post, Method::Put, Method::Patch, Method::Delete] {
            assert!(m.is_write(), "{m:?} is a write");
        }
        assert_eq!(Method::parse("post"), Some(Method::Post));
        assert_eq!(Method::parse("TRACE"), None);
    }

    #[test]
    fn page_clamps_limit_to_the_cap_and_is_total() {
        let req = EdgeRequest::new("GET", "/", "limit=10000&cursor=abc", vec![], vec![]);
        let p = Page::from_request(&req);
        assert_eq!(p.limit, MAX_PAGE_LIMIT, "limit is clamped to the cap");
        assert_eq!(p.cursor, Some("abc".to_string()));
        let req2 = EdgeRequest::new("GET", "/", "limit=banana", vec![], vec![]);
        assert_eq!(Page::from_request(&req2).limit, DEFAULT_PAGE_LIMIT);
    }

    #[test]
    fn page_envelope_shape() {
        let env = page_envelope(json!([1, 2]), Some("nxt".into()), 50);
        assert_eq!(env["items"], json!([1, 2]));
        assert_eq!(env["page"]["next_cursor"], "nxt");
        assert_eq!(env["page"]["limit"], 50);
    }
}
