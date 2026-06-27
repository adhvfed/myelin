//! # The API conventions every subsystem follows + the "how a subsystem plugs in" contract
//!
//! This module is the headline of MR-014 — the conventions that make the edge coherent, modelled on
//! Git's `api.rs` endpoint grammar (`Method`/`Endpoint`/`Handler`) so a subsystem declares its
//! surface the SAME way everywhere:
//!
//! - **HTTP method semantics** — [`Method`] (with [`Method::is_write`]); the write/read split drives
//!   the per-subsystem `Id.check`-then-state-change discipline (git's BUS-2 invariant) the subsystem
//!   handler upholds.
//! - **Versioning** — every route is registered under a version prefix (`/v1/...`); [`API_VERSION`].
//! - **Pagination** — a UNIFORM cursor/limit convention ([`Page`] + [`page_envelope`]): a list
//!   response is `{ items: [...], page: { next_cursor, limit } }`; `limit` is capped
//!   ([`MAX_PAGE_LIMIT`]) so a client cannot ask for an unbounded page.
//! - **The JSON view-model/data contract** — the edge serves the existing ViewModels' DATA as JSON
//!   (`serde_json::Value`), not HTML: the UI renders, the edge provides the projection. (git's
//!   `web.rs` ViewModels are the DATA the git handler will project in MR-015.)
//! - **The handler seam** — [`Handler`] + [`HandlerCtx`]: a subsystem registers handlers; the gateway
//!   calls `handle(ctx)` AFTER authentication + tenant-scope + authorization, so a handler receives a
//!   VERIFIED [`myelin_identity::Principal`] + the set [`myelin_storage::TenantScope`] and never sees
//!   a raw credential or a client-supplied tenant.
//!
//! ## How a subsystem plugs into the edge (the contract MR-015+ follow)
//! A subsystem (Git first, MR-015) gives the gateway, for each endpoint:
//!   1. a [`Method`] + a route pattern (`/v1/git/repos/{repo}/prs/{n}`) registered via
//!      [`crate::GatewayBuilder::route`]; `{name}` segments are path params, `{tenant}` is the
//!      special path-tenant the IDOR guard checks (NEVER the source of the operating tenant);
//!   2. an authorize ACTION string (re-checked through the [`Authorizer`](myelin_substrate::Authorizer)
//!      seam on every call — "internal = safe" is never presumed);
//!   3. a [`Handler`] whose `handle(ctx)` runs the already-built subsystem handler over `ctx.scope`
//!      (its writes go through `with_tenant_tx` / `OutboxTx::emit`; its reads project the ViewModel
//!      DATA into a JSON [`EdgeResponse`](crate::EdgeResponse)).
//! The gateway owns authentication, tenant-from-token, the IDOR reject, authorization, the error
//! envelope, versioning, pagination parsing, and SSE — so a subsystem adds ONLY its routes + handlers.

use crate::error::EdgeError;
use crate::request::{EdgeRequest, EdgeResponse};
use myelin_identity::Principal;
use myelin_storage::TenantScope;
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// The API version prefix every route lives under (`/v1`). A breaking change ships a new prefix
/// (`/v2`) so old clients keep working — the edge never silently changes a `/v1` contract.
pub const API_VERSION: &str = "v1";

/// The default page size when a client does not specify one.
pub const DEFAULT_PAGE_LIMIT: usize = 50;
/// The hard cap on a page size — a client cannot request an unbounded page (the bounded-everything
/// floor: a list endpoint never returns more than this per call, §7.1).
pub const MAX_PAGE_LIMIT: usize = 100;

/// An HTTP method on the edge (the per-subsystem declaration grammar, modelled on git `api.rs`). The
/// `is_write` split drives the subsystem's `Id.check`-then-state-change+outbox discipline (BUS-2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// A read — projects a ViewModel as JSON (cell-local, per-viewer 0-leak).
    Get,
    /// A create/replace/mutate write — the handler runs `Id.check` → state-change + `OutboxTx::emit`
    /// in ONE transaction (BUS-2). `Post`/`Put`/`Patch`/`Delete` are all writes.
    Post,
    /// A replace write.
    Put,
    /// A partial-update write.
    Patch,
    /// A delete write.
    Delete,
}

impl Method {
    /// Is this a write method (the `Id.check` → state-change + outbox-emit gate applies)?
    pub fn is_write(self) -> bool {
        !matches!(self, Method::Get)
    }

    /// The uppercase HTTP token.
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Patch => "PATCH",
            Method::Delete => "DELETE",
        }
    }

    /// Parse an HTTP method token (case-insensitive). `None` for an unsupported method.
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

/// **The uniform pagination request** parsed from the query string (`?limit=&cursor=`). `limit` is
/// clamped to [`MAX_PAGE_LIMIT`] (a client cannot exceed the cap) and defaults to
/// [`DEFAULT_PAGE_LIMIT`]; `cursor` is an opaque continuation token a subsystem mints/reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page {
    /// The requested page size, clamped to `1..=MAX_PAGE_LIMIT`.
    pub limit: usize,
    /// The opaque continuation cursor (subsystem-defined), if any.
    pub cursor: Option<String>,
}

impl Page {
    /// Parse the pagination params from a request, clamping `limit` to the cap (total over a
    /// malformed `limit` — a non-numeric value falls back to the default, never a panic).
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

/// Build the uniform list envelope `{ items, page: { next_cursor, limit } }` (the convention every
/// list endpoint returns). `items` is the JSON array of view-model projections.
pub fn page_envelope(items: Value, next_cursor: Option<String>, limit: usize) -> Value {
    json!({
        "items": items,
        "page": { "next_cursor": next_cursor, "limit": limit },
    })
}

/// The context a [`Handler`] receives — AFTER authentication + tenant-scope + authorization. It
/// carries the VERIFIED principal, the SET tenant scope (the only tenant a handler may touch), the
/// extracted path params, the parsed page, and the raw request (for the body). A handler NEVER sees a
/// raw credential or a client-supplied tenant.
pub struct HandlerCtx<'a> {
    /// The verified principal the gateway resolved (tenant/region are authoritative).
    pub principal: &'a Principal,
    /// The set `(tenant, region)` scope (built from the verified token — the ONLY scope the handler
    /// may query under; the IDOR floor).
    pub scope: &'a TenantScope,
    /// The extracted path params (`{repo}` → "core", `{n}` → "42"). `{tenant}` is consumed by the
    /// gateway's IDOR guard before dispatch — a handler reads resource params, never the tenant.
    pub params: &'a BTreeMap<String, String>,
    /// The parsed pagination request (for list endpoints).
    pub page: &'a Page,
    /// The raw request (for the body / extra headers).
    pub request: &'a EdgeRequest,
}

/// **The handler seam every subsystem implements.** The gateway calls `handle(ctx)` after it has
/// authenticated the request, resolved + set the tenant scope, and re-authorized the action. The
/// handler returns the JSON view-model response or a typed [`EdgeError`]. Object-safe so a
/// heterogeneous set of subsystem handlers lives in one router.
pub trait Handler: Send + Sync {
    /// Run the already-built subsystem handler over the verified scope, returning a view-model
    /// response or a typed error (mapped to the `{error:{message}}` envelope by the gateway).
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
        // a non-numeric / absent limit → the default; never a panic.
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
