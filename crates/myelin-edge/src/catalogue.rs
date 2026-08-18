use crate::error::EdgeError;
use crate::request::{decode_form_query_component, EdgeRequest, EdgeResponse};
use myelin_identity::Principal;
use myelin_identity_service::RequestIdentity;
use myelin_storage::TenantScope;
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub const API_VERSION: &str = "v1";

pub const DEFAULT_PAGE_LIMIT: usize = 50;
pub const MAX_PAGE_LIMIT: usize = 100;
const MAX_PAGE_QUERY_BYTES: usize = 16 * 1024;
const MAX_PAGE_CURSOR_BYTES: usize = 8 * 1024;

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
    pub fn parse(query: &str, subject: &str) -> Result<Page, EdgeError> {
        if query.len() > MAX_PAGE_QUERY_BYTES {
            return Err(EdgeError::BadRequest(format!(
                "{subject} query exceeds {MAX_PAGE_QUERY_BYTES} bytes"
            )));
        }
        let mut limit = None;
        let mut cursor = None;
        if !query.is_empty() {
            for pair in query.split('&') {
                let (raw_name, raw_value) = pair.split_once('=').ok_or_else(|| {
                    EdgeError::BadRequest(format!("malformed {subject} query parameter"))
                })?;
                let name = decode_form_query_component(raw_name, subject)?;
                let value = decode_form_query_component(raw_value, subject)?;
                match name.as_str() {
                    "limit" if limit.is_none() => {
                        let parsed = value.parse::<usize>().ok().filter(|parsed| {
                            value == parsed.to_string() && (1..=MAX_PAGE_LIMIT).contains(parsed)
                        });
                        limit = Some(parsed.ok_or_else(|| {
                            EdgeError::BadRequest(format!(
                                "{subject} limit must be a canonical integer between 1 and \
                                 {MAX_PAGE_LIMIT}"
                            ))
                        })?);
                    }
                    "cursor" if cursor.is_none() => {
                        if value.is_empty()
                            || value.len() > MAX_PAGE_CURSOR_BYTES
                            || value.chars().any(char::is_control)
                        {
                            return Err(EdgeError::BadRequest(format!(
                                "{subject} cursor must be nonempty printable text of at most \
                                 {MAX_PAGE_CURSOR_BYTES} bytes"
                            )));
                        }
                        cursor = Some(value);
                    }
                    "limit" | "cursor" => {
                        return Err(EdgeError::BadRequest(format!(
                            "duplicate {subject} query parameter `{name}`"
                        )))
                    }
                    "" => {
                        return Err(EdgeError::BadRequest(format!(
                            "empty {subject} query parameter name"
                        )))
                    }
                    _ => {
                        return Err(EdgeError::BadRequest(format!(
                            "unknown {subject} query parameter `{name}`"
                        )))
                    }
                }
            }
        }
        Ok(Page {
            limit: limit.unwrap_or(DEFAULT_PAGE_LIMIT),
            cursor,
        })
    }

    pub fn offset(&self, maximum: usize, subject: &str) -> Result<usize, EdgeError> {
        match self.cursor.as_deref() {
            None => Ok(0),
            Some(cursor) => cursor
                .parse::<usize>()
                .ok()
                .filter(|offset| cursor == offset.to_string() && *offset <= maximum)
                .ok_or_else(|| EdgeError::BadRequest(format!("invalid {subject} cursor"))),
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
    fn pagination_is_explicit_canonical_and_bounded() {
        assert_eq!(
            Page::parse("", "activity").unwrap(),
            Page {
                limit: DEFAULT_PAGE_LIMIT,
                cursor: None
            }
        );
        assert_eq!(
            Page::parse("limit=100&cursor=next%3Apage", "activity").unwrap(),
            Page {
                limit: MAX_PAGE_LIMIT,
                cursor: Some("next:page".into())
            }
        );

        for query in [
            "limit=0",
            "limit=01",
            "limit=101",
            "limit=banana",
            "limit=1&limit=2",
            "cursor=",
            "cursor=%00",
            "cursor=a&cursor=b",
            "limt=1",
            "limit",
            "limit=%GG",
        ] {
            assert!(
                matches!(
                    Page::parse(query, "activity"),
                    Err(EdgeError::BadRequest(_))
                ),
                "ambiguous page query was admitted: {query}"
            );
        }
    }

    #[test]
    fn page_envelope_shape() {
        let env = page_envelope(json!([1, 2]), Some("nxt".into()), 50);
        assert_eq!(env["items"], json!([1, 2]));
        assert_eq!(env["page"]["next_cursor"], "nxt");
        assert_eq!(env["page"]["limit"], 50);
    }
}
