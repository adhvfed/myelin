//! HTTP shapes for password sign-in and invitation links. Every response here is `no-store`: they
//! carry session tokens, invitation details, or the fact that an account exists.

use myelin_identity::{Principal, PrincipalKind};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::json;

use super::{Acceptance, AccountError, AccountService, MemberInvitation};
use crate::catalogue::{Handler, HandlerCtx};
use crate::error::EdgeError;
use crate::gateway::{no_store, route_segments};
use crate::request::{EdgeRequest, EdgeResponse};

/// Room for a 1024-character password even when every character is escaped in JSON. The server
/// applies it while reading the body, so a larger one is never buffered.
pub(crate) const MAX_ACCOUNT_REQUEST_BYTES: usize = 16 * 1024;

/// The endpoints whose bodies are capped at `MAX_ACCOUNT_REQUEST_BYTES`.
pub(crate) fn is_account_path(path: &str) -> bool {
    let segments: Vec<&str> = route_segments(path).collect();
    matches!(
        segments.as_slice(),
        ["v1", "auth", "password", "login" | "forgot"] | ["v1", "invitations"]
    ) || matches!(segments.as_slice(), ["v1", "auth", "invitations", _, ..])
}

pub(crate) type SessionMint<'a> = &'a dyn Fn(&Principal) -> Result<(String, i64), EdgeError>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoginRequest {
    email: String,
    password: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AcceptRequest {
    display_name: Option<String>,
    password: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InviteRequest {
    email: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ForgotRequest {
    email: String,
}

fn parse<T: DeserializeOwned>(bytes: &[u8], operation: &str) -> Result<T, EdgeResponse> {
    if bytes.len() > MAX_ACCOUNT_REQUEST_BYTES {
        return Err(error(&EdgeError::PayloadTooLarge(format!(
            "{operation} request exceeds 16 KiB"
        ))));
    }
    serde_json::from_slice(bytes).map_err(|_| {
        error(&EdgeError::BadRequest(format!(
            "{operation} request must be a JSON object with exactly the documented fields"
        )))
    })
}

fn error(edge: &EdgeError) -> EdgeResponse {
    no_store(EdgeResponse::error(edge))
}

fn account_error(failure: AccountError) -> EdgeResponse {
    let message = failure.to_string();
    match failure {
        AccountError::InvalidEmail
        | AccountError::InvalidDisplayName
        | AccountError::UnexpectedDisplayName => error(&EdgeError::BadRequest(message)),
        AccountError::LoginFailed => no_store(EdgeResponse::json(
            401,
            &json!({ "error": { "code": "unauthorized", "message": message } }),
        )),
        AccountError::InvitationNotFound | AccountError::NoSuchAccount => {
            error(&EdgeError::NotFound(message))
        }
        AccountError::WeakPassword => no_store(EdgeResponse::json(
            422,
            &json!({ "error": { "code": "weak_password", "message": message } }),
        )),
        AccountError::AccountExists => error(&EdgeError::Conflict(message)),
        AccountError::RateLimited { retry_after_secs } => {
            error(&EdgeError::TooManyRequests(message))
                .with_header("retry-after", retry_after_secs.to_string())
        }
        AccountError::Busy => {
            error(&EdgeError::Unavailable(message)).with_header("retry-after", "1")
        }
        AccountError::LinksUnavailable => error(&EdgeError::Unavailable(message)),
        AccountError::Unavailable(_) => error(&EdgeError::Unavailable(
            "accounts are temporarily unavailable".into(),
        )),
    }
}

fn session(principal: &Principal, mint: SessionMint<'_>) -> EdgeResponse {
    match mint(principal) {
        Ok((access_token, expires_at)) => no_store(EdgeResponse::json(
            200,
            &json!({
                "access_token": access_token,
                "token_type": "Bearer",
                "scheme": myelin_identity_service::machine_scheme::SESSION,
                "expires_at": expires_at,
            }),
        )),
        Err(failure) => error(&failure),
    }
}

fn configured(accounts: Option<&AccountService>) -> Result<&AccountService, EdgeResponse> {
    accounts.ok_or_else(|| {
        error(&EdgeError::Unavailable(
            "password sign-in is not configured".into(),
        ))
    })
}

/// `POST /v1/auth/password/login`
pub(crate) fn login(
    accounts: Option<&AccountService>,
    request: &EdgeRequest,
    mint: SessionMint<'_>,
) -> EdgeResponse {
    let outcome = configured(accounts).and_then(|accounts| {
        let body: LoginRequest = parse(&request.body, "password login")?;
        accounts
            .login(&body.email, body.password)
            .map_err(account_error)
    });
    match outcome {
        Ok(principal) => session(&principal, mint),
        Err(response) => response,
    }
}

/// `POST /v1/auth/password/forgot`: one answer for every email, whether or not it has an
/// account, has asked too often, or is malformed.
pub(crate) fn forgot(accounts: Option<&AccountService>, request: &EdgeRequest) -> EdgeResponse {
    let accounts = match configured(accounts) {
        Ok(accounts) if accounts.mail_enabled() => accounts,
        Ok(_) => {
            return error(&EdgeError::Unavailable(
                "password reset by email is not configured".into(),
            ))
        }
        Err(response) => return response,
    };
    let body: ForgotRequest = match parse(&request.body, "password reset") {
        Ok(body) => body,
        Err(response) => return response,
    };
    accounts.forgot_password(&body.email);
    no_store(EdgeResponse::json(202, &json!({ "status": "accepted" })))
}

/// `GET /v1/auth/invitations/{code}`
pub(crate) fn invitation(accounts: Option<&AccountService>, code: &str) -> EdgeResponse {
    let outcome =
        configured(accounts).and_then(|accounts| accounts.invitation(code).map_err(account_error));
    match outcome {
        Ok(view) => no_store(EdgeResponse::json(
            200,
            &json!({
                "kind": view.kind,
                "email": view.email,
                "organization": view.organization,
                "expires_at": view.expires_at_unix,
            }),
        )),
        Err(response) => response,
    }
}

/// `POST /v1/auth/invitations/{code}/accept`
pub(crate) fn accept(
    accounts: Option<&AccountService>,
    request: &EdgeRequest,
    code: &str,
    mint: SessionMint<'_>,
) -> EdgeResponse {
    let outcome = configured(accounts).and_then(|accounts| {
        let body: AcceptRequest = parse(&request.body, "invitation acceptance")?;
        accounts
            .accept(
                code,
                Acceptance {
                    display_name: body.display_name,
                    password: body.password,
                },
            )
            .map_err(account_error)
    });
    match outcome {
        Ok(principal) => session(&principal, mint),
        Err(response) => response,
    }
}

/// `POST /v1/invitations`: a signed-in member invites a teammate.
pub struct InviteHandler {
    accounts: AccountService,
}

impl InviteHandler {
    pub fn new(accounts: AccountService) -> Self {
        Self { accounts }
    }
}

impl Handler for InviteHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        if !matches!(ctx.principal.kind, PrincipalKind::Human) {
            return Err(EdgeError::Forbidden(
                "only a person can invite a teammate".into(),
            ));
        }
        let body: InviteRequest = match parse(&ctx.request.body, "invitation") {
            Ok(body) => body,
            Err(response) => return Ok(response),
        };
        Ok(
            match self.accounts.invite_member(ctx.principal, &body.email) {
                Ok(MemberInvitation::Emailed {
                    email,
                    expires_at_unix,
                }) => no_store(EdgeResponse::json(
                    201,
                    &json!({
                        "delivery": "email",
                        "email": email,
                        "expires_at": expires_at_unix,
                    }),
                )),
                Ok(MemberInvitation::HandedOver(issued)) => no_store(EdgeResponse::json(
                    201,
                    &json!({
                        "delivery": "link",
                        "url": issued.url,
                        "email": issued.email,
                        "expires_at": issued.expires_at_unix,
                    }),
                )),
                Err(failure) => account_error(failure),
            },
        )
    }
}
