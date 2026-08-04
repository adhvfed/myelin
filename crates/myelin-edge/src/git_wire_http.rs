use crate::catalogue::{Handler, HandlerCtx, Method};
use crate::error::EdgeError;
use crate::gateway::GatewayBuilder;
use crate::git_durable::DurableGitBackend;
use crate::git_edge::{param, tenant_of};
use crate::repo_authz::RepoAccess;
use crate::request::EdgeResponse;
use myelin_git::core::{GitCore, GitCoreError, RepoLoc, Service};
use std::sync::Arc;

const UPLOAD_PACK_ADV: &str = "application/x-git-upload-pack-advertisement";
const UPLOAD_PACK_RESULT: &str = "application/x-git-upload-pack-result";

pub(crate) fn pkt_line(payload: &str) -> Vec<u8> {
    let mut v = format!("{:04x}", payload.len() + 4).into_bytes();
    v.extend_from_slice(payload.as_bytes());
    v
}

pub(crate) fn raw(status: u16, content_type: &str, body: Vec<u8>) -> EdgeResponse {
    EdgeResponse::Bytes {
        status,
        content_type: content_type.to_string(),
        headers: vec![
            (
                "cache-control".to_string(),
                "no-cache, max-age=0, must-revalidate".to_string(),
            ),
        ],
        body,
    }
}

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

fn map_wire_err(e: &GitCoreError) -> EdgeError {
    let msg = e.to_string();
    if msg.contains("not present")
        || msg.contains("stat-able")
        || msg.contains("not a directory")
        || msg.contains("No such file")
        || msg.contains("escapes the canonical")
        || msg.contains("path confinement refused")
    {
        EdgeError::NotFound("repository not found".into())
    } else {
        EdgeError::Internal(format!("git wire error: {msg}"))
    }
}

fn map_durable_to_wire(e: myelin_git::durable::DurableError) -> EdgeError {
    use myelin_git::durable::DurableError;
    match e {
        DurableError::NotFound(_) => EdgeError::NotFound("repository not found".into()),
        DurableError::Git(m)
            if m.contains("traversal") || m.contains("segment") || m.contains("slug") =>
        {
            EdgeError::BadRequest(m)
        }
        DurableError::Forbidden(m) => EdgeError::Forbidden(m),
        other => EdgeError::Internal(format!("git push error: {other}")),
    }
}

struct WireInfoRefs {
    be: Arc<DurableGitBackend>,
}
impl Handler for WireInfoRefs {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let service = ctx.request.query_param("service").unwrap_or_default();
        let loc = repo_loc(ctx)?;
        if service == "git-receive-pack" {
            if !self
                .be
                .repo_authorizer()
                .authorize_repo(ctx.principal, &loc, RepoAccess::Write)
            {
                return Err(EdgeError::Forbidden(
                    "no write grant for this repository".into(),
                ));
            }
            let refs = self
                .be
                .receive_pack_refs(loc.tenant.as_str(), loc.region.as_str(), loc.repo.as_str())
                .map_err(map_durable_to_wire)?;
            let mut body = pkt_line("# service=git-receive-pack\n");
            body.extend_from_slice(b"0000");
            body.extend_from_slice(&crate::git_receive_pack::build_receive_pack_refs(&refs));
            return Ok(raw(200, crate::git_receive_pack::RECEIVE_PACK_ADV, body));
        }
        if service != "git-upload-pack" {
            return Err(EdgeError::BadRequest(
                "only the smart git protocol is supported (expected ?service=git-upload-pack)"
                    .into(),
            ));
        }
        if !self
            .be
            .repo_authorizer()
            .authorize_repo(ctx.principal, &loc, RepoAccess::Read)
        {
            return Err(EdgeError::NotFound("repository not found".into()));
        }
        let adv = self
            .be
            .wire_serving(ctx.principal)
            .advertise_refs(&loc, Service::UploadPack)
            .map_err(|e| map_wire_err(&e))?;
        if adv.status != 0 {
            return Err(EdgeError::Internal(format!(
                "advertise_refs exited {} for the upload-pack service",
                adv.status
            )));
        }
        let mut body = pkt_line("# service=git-upload-pack\n");
        body.extend_from_slice(b"0000");
        body.extend_from_slice(&adv.stdout);
        Ok(raw(200, UPLOAD_PACK_ADV, body))
    }
}

struct WireUploadPack {
    be: Arc<DurableGitBackend>,
}
impl Handler for WireUploadPack {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let loc = repo_loc(ctx)?;
        if !self
            .be
            .repo_authorizer()
            .authorize_repo(ctx.principal, &loc, RepoAccess::Read)
        {
            return Err(EdgeError::NotFound("repository not found".into()));
        }
        let body = ctx.request.body.clone();
        let served = self
            .be
            .wire_serving(ctx.principal)
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

struct WireReceivePack {
    be: Arc<DurableGitBackend>,
}
impl Handler for WireReceivePack {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let loc = repo_loc(ctx)?;
        if !self
            .be
            .repo_authorizer()
            .authorize_repo(ctx.principal, &loc, RepoAccess::Write)
        {
            return Err(EdgeError::Forbidden(
                "no write grant for this repository".into(),
            ));
        }
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

pub fn register_git_wire(mut b: GatewayBuilder, be: Arc<DurableGitBackend>) -> GatewayBuilder {
    b = b.route(
        Method::Get,
        "/{tenant}/{region}/{repo}/info/refs",
        "git.wire.upload_pack",
        Arc::new(WireInfoRefs { be: be.clone() }),
    );
    b = b.route(
        Method::Post,
        "/{tenant}/{region}/{repo}/git-upload-pack",
        "git.wire.upload_pack",
        Arc::new(WireUploadPack { be: be.clone() }),
    );
    b = b.route(
        Method::Post,
        "/{tenant}/{region}/{repo}/git-receive-pack",
        "git.wire.receive_pack",
        Arc::new(WireReceivePack { be: be.clone() }),
    );
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalogue::Page;
    use crate::repo_authz::{DenyAllRepos, GrantBackedRepos};
    use crate::request::EdgeRequest;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_storage::TenantScope;
    use myelin_tenancy::TenantId;
    use std::collections::BTreeMap;

    fn principal() -> Principal {
        Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Service,
            TenantId("acme".into()),
        )
    }

    fn backend(authz: Arc<dyn crate::repo_authz::RepoAuthorizer>) -> Arc<DurableGitBackend> {
        let root = std::env::temp_dir().join(format!(
            "myelin-r03-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        Arc::new(DurableGitBackend::rooted_inmem_for_test(root).with_repo_authorizer(authz))
    }

    fn run(
        handler: &dyn Handler,
        be_principal: &Principal,
        scope: &TenantScope,
        repo: &str,
        query: &str,
    ) -> Result<EdgeResponse, EdgeError> {
        let mut params = BTreeMap::new();
        params.insert("repo".to_string(), repo.to_string());
        let req = EdgeRequest::new(
            "GET",
            "/acme/acme-home/widgets/info/refs",
            query,
            vec![],
            vec![],
        );
        let page = Page::from_request(&req);
        let identity = crate::catalogue::test_request_identity(be_principal, scope);
        let ctx = HandlerCtx {
            identity: &identity,
            principal: be_principal,
            scope,
            params: &params,
            page: &page,
            request: &req,
        };
        handler.handle(&ctx)
    }

    fn deny_status(r: Result<EdgeResponse, EdgeError>) -> u16 {
        match r {
            Ok(_) => panic!("expected a denial, got a served response"),
            Err(e) => e.status(),
        }
    }

    #[test]
    fn upload_pack_advert_denied_is_zero_leak_404() {
        let p = principal();
        let scope = TenantScope::from_verified_token(&p, p.region.clone());
        let be = backend(Arc::new(DenyAllRepos));
        let h = WireInfoRefs { be };
        let st = deny_status(run(
            &h,
            &p,
            &scope,
            "widgets.git",
            "service=git-upload-pack",
        ));
        assert_eq!(st, 404, "an un-granted READ leaks nothing (0-leak 404)");
    }

    #[test]
    fn receive_pack_advert_denied_is_403() {
        let p = principal();
        let scope = TenantScope::from_verified_token(&p, p.region.clone());
        let be = backend(Arc::new(DenyAllRepos));
        let h = WireInfoRefs { be };
        let st = deny_status(run(
            &h,
            &p,
            &scope,
            "widgets.git",
            "service=git-receive-pack",
        ));
        assert_eq!(st, 403, "an un-granted WRITE is a fail-closed 403");
    }

    #[test]
    fn receive_pack_push_denied_is_403() {
        let p = principal();
        let scope = TenantScope::from_verified_token(&p, p.region.clone());
        let be = backend(Arc::new(DenyAllRepos));
        let h = WireReceivePack { be };
        let st = deny_status(run(&h, &p, &scope, "widgets.git", ""));
        assert_eq!(st, 403);
    }

    #[test]
    fn upload_pack_serve_denied_is_zero_leak_404() {
        let p = principal();
        let scope = TenantScope::from_verified_token(&p, p.region.clone());
        let be = backend(Arc::new(DenyAllRepos));
        let h = WireUploadPack { be };
        let st = deny_status(run(&h, &p, &scope, "widgets.git", ""));
        assert_eq!(st, 404);
    }

    #[test]
    fn write_grant_admits_receive_pack_advert_past_the_seam() {
        let p = principal();
        let scope = TenantScope::from_verified_token(&p, p.region.clone());
        let be = backend(Arc::new(
            GrantBackedRepos::new().grant_write("p", "acme", "widgets"),
        ));
        let h = WireInfoRefs { be };
        let st = deny_status(run(
            &h,
            &p,
            &scope,
            "widgets.git",
            "service=git-receive-pack",
        ));
        assert_eq!(
            st, 404,
            "a WRITE grant admits the seam; the absent repo is then a 404 (not a 403 deny)"
        );
    }
}
