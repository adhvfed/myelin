//! # The git smart-HTTP transport server (the read side: clone/fetch) — CT-006c / GT-006
//!
//! This wires the git "dumb-URL, smart-protocol" HTTP endpoints over the edge, driving the production
//! [`myelin_git::core::RoutedGitCore`] the [`DurableGitBackend::wire_serving`] composes (sandboxed
//! `git upload-pack` in the hardened gVisor sandbox + the in-process read backend, over the SAME on-disk
//! root the durable store uses). A REAL `git clone`/`git fetch` against a Myelin server lands here.
//!
//! ## Endpoints (the git smart-HTTP grammar)
//! - `GET  /<tenant>/<region>/<repo>.git/info/refs?service=git-upload-pack`
//!   → [`GitCore::advertise_refs`]`(UploadPack)`, wrapped in the smart-HTTP service framing
//!     (`001e# service=git-upload-pack\n` + `0000` + the advertisement),
//!     `Content-Type: application/x-git-upload-pack-advertisement`.
//! - `POST /<tenant>/<region>/<repo>.git/git-upload-pack`
//!   → [`GitCore::serve`]`(UploadPack, body)`, `Content-Type: application/x-git-upload-pack-result`.
//! - `POST /<tenant>/<region>/<repo>.git/git-receive-pack` (push) → **403, not yet** — the durable
//!   quarantine-intake + one-tx ref-CAS push path over the wire is **CT-006d** (stated, not built here).
//!
//! ## AUTH / AUTHZ (the security floor — reused, never forked)
//! The gateway owns the lifecycle (authenticate → resolve tenant-from-token → reject+audit a
//! cross-tenant IDOR → re-authorize the action → dispatch), EXACTLY as for every durable git route.
//! These wire routes carry a `{tenant}`/`{region}` path segment (git's URL grammar requires it), so:
//!   - **unauthenticated** (no/invalid Bearer) → a uniform **401** (the real PASETO verifier);
//!   - **cross-tenant** (token tenant ≠ URL tenant) → the gateway's audited IDOR **reject** (403),
//!     fired BEFORE any repo lookup — so repo existence is NEVER leaked across tenants;
//!   - the per-action **authorize** seam gates the read (`git.wire.upload_pack`) — a denial is a 403.
//! The operating `(tenant, region)` for the actual lookup is taken from the VERIFIED token
//! (`ctx.scope`), NEVER from the URL (the GIT-D8 cardinal rule); the URL `{tenant}` is used only to
//! detect/reject the IDOR. The repo path is then resolver-validated + symlink-confined inside the
//! sandboxed launch (CT-006a/b: no `..`/separator/cross-tenant escape).

use crate::catalogue::{Handler, HandlerCtx, Method};
use crate::error::EdgeError;
use crate::gateway::GatewayBuilder;
use crate::git_durable::DurableGitBackend;
use crate::git_edge::{param, tenant_of};
use crate::request::EdgeResponse;
use myelin_git::core::{GitCore, GitCoreError, RepoLoc, Service};
use std::sync::Arc;

/// The smart-HTTP advertisement content-type for `git-upload-pack`.
const UPLOAD_PACK_ADV: &str = "application/x-git-upload-pack-advertisement";
/// The smart-HTTP result content-type for `git-upload-pack`.
const UPLOAD_PACK_RESULT: &str = "application/x-git-upload-pack-result";

/// pkt-line a payload: a 4-hex length prefix counting itself + the payload bytes (`001e# service…\n`).
pub(crate) fn pkt_line(payload: &str) -> Vec<u8> {
    let mut v = format!("{:04x}", payload.len() + 4).into_bytes();
    v.extend_from_slice(payload.as_bytes());
    v
}

/// A raw (non-JSON) byte response with an explicit content-type — the git wire bytes are NOT a JSON
/// view-model, so the smart-HTTP body is emitted verbatim.
pub(crate) fn raw(status: u16, content_type: &str, body: Vec<u8>) -> EdgeResponse {
    EdgeResponse::Bytes {
        status,
        content_type: content_type.to_string(),
        headers: vec![
            // Smart-HTTP responses must not be cached by intermediaries (the refs/pack are live).
            ("cache-control".to_string(), "no-cache, max-age=0, must-revalidate".to_string()),
        ],
        body,
    }
}

/// Resolve the `(tenant, region, repo)` for the lookup from the VERIFIED token scope (never the URL —
/// GIT-D8) + the URL's `{repo}` segment with its `.git` suffix stripped (the resolver re-appends `.git`,
/// so a raw `widgets.git` would otherwise resolve to `widgets.git.git`).
pub(crate) fn repo_loc(ctx: &HandlerCtx<'_>) -> Result<RepoLoc, EdgeError> {
    let repo_seg = param(ctx, "repo")?;
    let slug = repo_seg.strip_suffix(".git").unwrap_or(repo_seg);
    if slug.is_empty() {
        return Err(EdgeError::BadRequest("empty repo slug".into()));
    }
    Ok(RepoLoc::new(
        tenant_of(ctx),
        ctx.scope.region().0.as_str(),
        slug,
    ))
}

/// Map a wire-path [`GitCoreError`] to an edge status. An absent/unstat-able repo (the resolver/symlink
/// confinement could not find a real bare repo under the verified tenant) is a **0-leak 404** — exactly
/// the durable read posture (a cross-tenant repo "simply is not found" under this tenant's path). Any
/// other wire failure (a sandbox/runtime fault) is a 500 — never a silent empty 200.
fn map_wire_err(e: &GitCoreError) -> EdgeError {
    let msg = e.to_string();
    if msg.contains("not present")
        || msg.contains("stat-able")
        || msg.contains("not a directory")
        || msg.contains("No such file")
        || msg.contains("escapes the canonical")
        // A path-confinement refusal (a `..`/separator/non-allowlisted slug) is also a 0-leak 404 — a
        // traversal attempt reveals nothing about what does/does not exist under any tenant.
        || msg.contains("path confinement refused")
    {
        EdgeError::NotFound("repository not found".into())
    } else {
        EdgeError::Internal(format!("git wire error: {msg}"))
    }
}

/// Map a durable-backend error from a push path to an edge status. A repo absent under the verified
/// tenant is the 0-leak 404 (a cross-tenant/non-existent repo "is not found"); a traversal-rejected
/// slug is a 400; anything else is a 500 — never a silent empty/200.
fn map_durable_to_wire(e: myelin_git::durable::DurableError) -> EdgeError {
    use myelin_git::durable::DurableError;
    match e {
        DurableError::NotFound(_) => EdgeError::NotFound("repository not found".into()),
        DurableError::Git(m) if m.contains("traversal") || m.contains("segment") || m.contains("slug") => {
            EdgeError::BadRequest(m)
        }
        other => EdgeError::Internal(format!("git push error: {other}")),
    }
}

/// `GET /<tenant>/<region>/<repo>.git/info/refs?service=git-upload-pack` — the smart-HTTP ref
/// advertisement (the first half of a clone/fetch handshake).
struct WireInfoRefs {
    be: Arc<DurableGitBackend>,
}
impl Handler for WireInfoRefs {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let service = ctx.request.query_param("service").unwrap_or_default();
        // The push (receive-pack) ref advertisement (CT-006d): built IN-PROCESS from the durable repo's
        // refs with a restricted capability set (no side-band / report-status-v2 / atomic). A pure read
        // of our own tenant-scoped repo — no sandbox needed.
        if service == "git-receive-pack" {
            let loc = repo_loc(ctx)?;
            let refs = self
                .be
                .receive_pack_refs(loc.tenant.as_str(), loc.region.as_str(), loc.repo.as_str())
                .map_err(map_durable_to_wire)?;
            // Smart-HTTP framing: pkt-line("# service=git-receive-pack\n") + flush + the advertisement.
            let mut body = pkt_line("# service=git-receive-pack\n");
            body.extend_from_slice(b"0000");
            body.extend_from_slice(&crate::git_receive_pack::build_receive_pack_refs(&refs));
            return Ok(raw(200, crate::git_receive_pack::RECEIVE_PACK_ADV, body));
        }
        if service != "git-upload-pack" {
            // A dumb-HTTP client (no `?service=`) is unsupported — Myelin serves smart-HTTP only.
            return Err(EdgeError::BadRequest(
                "only the smart git protocol is supported (expected ?service=git-upload-pack)".into(),
            ));
        }
        let loc = repo_loc(ctx)?;
        let adv = self
            .be
            .wire_serving()
            .advertise_refs(&loc, Service::UploadPack)
            .map_err(|e| map_wire_err(&e))?;
        if adv.status != 0 {
            return Err(EdgeError::Internal(format!(
                "advertise_refs exited {} for the upload-pack service",
                adv.status
            )));
        }
        // Smart-HTTP framing: pkt-line("# service=git-upload-pack\n") + flush-pkt + the advertisement.
        let mut body = pkt_line("# service=git-upload-pack\n");
        body.extend_from_slice(b"0000");
        body.extend_from_slice(&adv.stdout);
        Ok(raw(200, UPLOAD_PACK_ADV, body))
    }
}

/// `POST /<tenant>/<region>/<repo>.git/git-upload-pack` — serve the negotiated packfile (the second
/// half of a clone/fetch). The request body is the client's want/have/done negotiation; the response is
/// the raw upload-pack result (NAK + the packfile, possibly side-band-multiplexed).
struct WireUploadPack {
    be: Arc<DurableGitBackend>,
}
impl Handler for WireUploadPack {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let loc = repo_loc(ctx)?;
        let body = ctx.request.body.clone();
        let served = self
            .be
            .wire_serving()
            .serve(&loc, Service::UploadPack, body)
            .map_err(|e| map_wire_err(&e))?;
        if served.status != 0 {
            return Err(EdgeError::Internal(format!(
                "upload-pack serve exited {}",
                served.status
            )));
        }
        Ok(raw(200, UPLOAD_PACK_RESULT, served.stdout))
    }
}

/// `POST /<tenant>/<region>/<repo>.git/git-receive-pack` — the PUSH (CT-006d). Drives
/// [`DurableGitBackend::receive_pack`]: parse the ref-update commands + packfile, ingest the untrusted
/// pack in the hardened sandbox, run the in-process policy + the one-tx ref-CAS + `git.ref.updated`
/// outbox emit, and return the `report-status`. The gateway has already authenticated + authorized the
/// WRITE action (`git.wire.receive_pack`) + rejected any cross-tenant IDOR; the operating `(tenant,
/// region)` is the VERIFIED token's (never the URL).
struct WireReceivePack {
    be: Arc<DurableGitBackend>,
}
impl Handler for WireReceivePack {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let loc = repo_loc(ctx)?;
        let body = self
            .be
            .receive_pack(
                loc.tenant.as_str(),
                loc.region.as_str(),
                loc.repo.as_str(),
                ctx.principal,
                &ctx.request.body,
            )
            .map_err(map_durable_to_wire)?;
        Ok(raw(200, crate::git_receive_pack::RECEIVE_PACK_RESULT, body))
    }
}

/// **Register the git smart-HTTP wire endpoints (read side) on the gateway.** The clone/fetch routes
/// drive [`DurableGitBackend::wire_serving`]; the gateway owns auth/tenant-from-token/IDOR/authorize.
/// The receive-pack route is registered as a LOUD 403 (push is CT-006d). The routes use git's literal
/// URL grammar (`/<tenant>/<region>/<repo>.git/...`) so a real `git` client clones/fetches directly.
pub fn register_git_wire(mut b: GatewayBuilder, be: Arc<DurableGitBackend>) -> GatewayBuilder {
    // GET .../info/refs — the ref advertisement (read; gated by the upload-pack read action).
    b = b.route(
        Method::Get,
        "/{tenant}/{region}/{repo}/info/refs",
        "git.wire.upload_pack",
        Arc::new(WireInfoRefs { be: be.clone() }),
    );
    // POST .../git-upload-pack — the packfile serve (read; same read action).
    b = b.route(
        Method::Post,
        "/{tenant}/{region}/{repo}/git-upload-pack",
        "git.wire.upload_pack",
        Arc::new(WireUploadPack { be: be.clone() }),
    );
    // POST .../git-receive-pack — push (CT-006d): a distinct write action; 403 not-yet.
    b = b.route(
        Method::Post,
        "/{tenant}/{region}/{repo}/git-receive-pack",
        "git.wire.receive_pack",
        Arc::new(WireReceivePack { be: be.clone() }),
    );
    b
}
