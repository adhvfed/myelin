use crate::catalogue::{HandlerCtx, Method, API_VERSION};
use crate::error::EdgeError;
use myelin_git::api::Method as GitMethod;

pub(crate) fn tenant_of<'a>(ctx: &'a HandlerCtx<'_>) -> &'a str {
    ctx.scope.tenant().0.as_str()
}

pub(crate) fn param<'a>(ctx: &'a HandlerCtx<'_>, name: &str) -> Result<&'a str, EdgeError> {
    ctx.params
        .get(name)
        .map(String::as_str)
        .ok_or_else(|| EdgeError::BadRequest(format!("missing path param `{name}`")))
}

pub(crate) fn pull_request_number_param(
    ctx: &HandlerCtx<'_>,
    name: &str,
) -> Result<u64, EdgeError> {
    let raw = param(ctx, name)?;
    myelin_git::coordinate::parse_positive_decimal(raw).ok_or_else(|| {
        EdgeError::BadRequest(format!(
            "path param `{name}` must be a canonical positive pull-request number"
        ))
    })
}

pub(crate) fn reroot(path: &str) -> String {
    let tail = path.strip_prefix("/api/git").unwrap_or(path);
    format!("/{API_VERSION}/git{tail}")
}

pub(crate) fn map_method(method: GitMethod) -> Method {
    match method {
        GitMethod::Get => Method::Get,
        GitMethod::Post => Method::Post,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalogue::test_request_identity;
    use crate::request::EdgeRequest;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_storage::TenantScope;
    use myelin_tenancy::TenantId;
    use std::collections::BTreeMap;

    fn route_pull_request_number(raw: &str) -> Result<u64, EdgeError> {
        let principal = Principal::stub(
            PrincipalId("reader".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        );
        let scope = TenantScope::from_verified_token(&principal, principal.region.clone());
        let identity = test_request_identity(&principal, &scope);
        let params = BTreeMap::from([("n".into(), raw.into())]);
        let request = EdgeRequest::new(
            "GET",
            format!("/v1/git/repos/api/prs/{raw}"),
            "",
            vec![],
            vec![],
        );
        let page = crate::catalogue::Page::from_request(&request);
        pull_request_number_param(
            &HandlerCtx {
                identity: &identity,
                principal: &principal,
                scope: &scope,
                params: &params,
                page: &page,
                request: &request,
            },
            "n",
        )
    }

    #[test]
    fn pull_request_routes_have_one_positive_decimal_identity() {
        assert_eq!(route_pull_request_number("42").unwrap(), 42);
        for alias in ["0", "00", "01", "+1", "1.0"] {
            assert!(
                matches!(
                    route_pull_request_number(alias),
                    Err(EdgeError::BadRequest(_))
                ),
                "route alias was admitted: {alias}"
            );
        }
    }

    #[test]
    fn subsystem_paths_are_rerooted_once() {
        assert_eq!(reroot("/api/git/repos"), "/v1/git/repos");
        assert_eq!(
            reroot("/api/git/repos/{repo}/prs/{n}/checks"),
            "/v1/git/repos/{repo}/prs/{n}/checks"
        );
    }
}
