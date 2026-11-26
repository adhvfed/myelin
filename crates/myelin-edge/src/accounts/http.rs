//! HTTP shapes for password sign-in and invitation links. Every response here is `no-store`: they
//! carry session tokens, invitation details, or the fact that an account exists.

use myelin_identity::{Principal, PrincipalKind};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

use super::{
    Acceptance, AccountError, AccountService, MemberInvitation, OrgRole, RosterView, SignedIn,
};
use crate::catalogue::{Handler, HandlerCtx};
use crate::error::EdgeError;
use crate::gateway::{no_store, route_segments, GatewayBuilder};
use crate::request::{EdgeRequest, EdgeResponse};
use crate::Method;

/// Room for a 1024-character password even when every character is escaped in JSON. The server
/// applies it while reading the body, so a larger one is never buffered.
pub(crate) const MAX_ACCOUNT_REQUEST_BYTES: usize = 16 * 1024;

/// The endpoints whose bodies are capped at `MAX_ACCOUNT_REQUEST_BYTES`.
pub(crate) fn is_account_path(path: &str) -> bool {
    let segments: Vec<&str> = route_segments(path).collect();
    matches!(
        segments.as_slice(),
        ["v1", "auth", "password", "login" | "forgot" | "change"]
            | ["v1", "invitations"]
            | ["v1", "members", _, "role"]
    ) || matches!(segments.as_slice(), ["v1", "auth", "invitations", _, ..])
}

pub(crate) type SessionMint<'a> = &'a dyn Fn(&SignedIn) -> Result<(String, i64), EdgeError>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoginRequest {
    email: String,
    password: String,
    #[serde(default)]
    device_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AcceptRequest {
    display_name: Option<String>,
    password: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChangePasswordRequest {
    current_password: String,
    new_password: String,
    #[serde(default)]
    device_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InviteRequest {
    email: String,
    #[serde(default)]
    role: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RoleRequest {
    role: String,
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
        // Not 401: the session that asked is fine, only the password it gave is not.
        AccountError::WrongPassword => no_store(EdgeResponse::json(
            403,
            &json!({ "error": { "code": "wrong_password", "message": message } }),
        )),
        AccountError::NoPassword => error(&EdgeError::Conflict(message)),
        AccountError::PasswordChangedElsewhere => no_store(EdgeResponse::json(
            409,
            &json!({ "error": { "code": "password_changed_elsewhere", "message": message } }),
        )),
        AccountError::SessionEnded => error(&EdgeError::Unauthorized(message)),
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
        AccountError::RecipientRateLimited { retry_after_secs } => no_store(EdgeResponse::json(
            429,
            &json!({ "error": {
                "code": "recipient_rate_limited",
                "message": message,
                "retry_after_secs": retry_after_secs,
            } }),
        ))
        .with_header("retry-after", retry_after_secs.to_string()),
        AccountError::Busy => {
            error(&EdgeError::Unavailable(message)).with_header("retry-after", "1")
        }
        AccountError::LinksUnavailable => error(&EdgeError::Unavailable(message)),
        AccountError::RoleRequired => no_store(EdgeResponse::json(
            403,
            &json!({ "error": { "code": "role_required", "message": message } }),
        )),
        AccountError::LastOwner => no_store(EdgeResponse::json(
            409,
            &json!({ "error": { "code": "last_owner", "message": message } }),
        )),
        AccountError::NoSuchMember => error(&EdgeError::NotFound(message)),
        AccountError::InvitationExists => no_store(EdgeResponse::json(
            409,
            &json!({ "error": { "code": "invitation_exists", "message": message } }),
        )),
        AccountError::PersonErased => no_store(EdgeResponse::json(
            409,
            &json!({ "error": { "code": "person_erased", "message": message } }),
        )),
        AccountError::Unavailable(_) => error(&EdgeError::Unavailable(
            "accounts are temporarily unavailable".into(),
        )),
    }
}

fn session(signed_in: &SignedIn, mint: SessionMint<'_>) -> EdgeResponse {
    match mint(signed_in) {
        Ok((access_token, expires_at)) => no_store(EdgeResponse::json(
            200,
            &json!({
                "access_token": access_token,
                "token_type": "Bearer",
                "scheme": myelin_identity_service::machine_scheme::SESSION,
                "expires_at": expires_at,
                "device_token": signed_in.device_token,
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
            .login(&body.email, body.password, body.device_token.as_deref())
            .map_err(account_error)
    });
    match outcome {
        Ok(signed_in) => session(&signed_in, mint),
        Err(response) => response,
    }
}

/// `POST /v1/auth/password/change`: `person` is the verified holder of the session that asked.
/// The answer is a fresh session, since the change ends every session they held.
pub(crate) fn change_password(
    accounts: Option<&AccountService>,
    request: &EdgeRequest,
    person: &Principal,
    mint: SessionMint<'_>,
) -> EdgeResponse {
    let outcome = configured(accounts).and_then(|accounts| {
        let body: ChangePasswordRequest = parse(&request.body, "password change")?;
        accounts
            .change_password(
                person,
                body.current_password,
                body.new_password,
                body.device_token.as_deref(),
            )
            .map_err(account_error)
    });
    match outcome {
        Ok(signed_in) => session(&signed_in, mint),
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
            ));
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
        Ok(signed_in) => session(&signed_in, mint),
        Err(response) => response,
    }
}

/// The people endpoints, each answered for a signed-in person. Roles are judged by the account
/// service against the database on every request, not by the session's authority.
pub fn register_members(builder: GatewayBuilder, accounts: AccountService) -> GatewayBuilder {
    let route = |op| {
        Arc::new(MembersHandler {
            accounts: accounts.clone(),
            op,
        })
    };
    builder
        .route(
            Method::Post,
            "/v1/invitations",
            "edge.invitations.create",
            route(MembersOp::Invite),
        )
        .route(
            Method::Delete,
            "/v1/invitations/{id}",
            "edge.invitations.revoke",
            route(MembersOp::Revoke),
        )
        .route(
            Method::Get,
            "/v1/members",
            "edge.members.list",
            route(MembersOp::List),
        )
        .route(
            Method::Post,
            "/v1/members/{id}/role",
            "edge.members.role.set",
            route(MembersOp::SetRole),
        )
        .route(
            Method::Delete,
            "/v1/members/{id}",
            "edge.members.remove",
            route(MembersOp::Remove),
        )
}

/// `POST /v1/invitations` on its own.
pub fn invitation_handler(accounts: AccountService) -> impl Handler {
    MembersHandler {
        accounts,
        op: MembersOp::Invite,
    }
}

#[derive(Clone, Copy)]
enum MembersOp {
    Invite,
    Revoke,
    List,
    SetRole,
    Remove,
}

struct MembersHandler {
    accounts: AccountService,
    op: MembersOp,
}

impl Handler for MembersHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        if !matches!(ctx.principal.kind, PrincipalKind::Human) {
            return Err(EdgeError::Forbidden(
                "only a person can manage an organization's people".into(),
            ));
        }
        let id = || ctx.params.get("id").map(String::as_str).unwrap_or_default();
        let person = ctx.principal;
        Ok(match self.op {
            MembersOp::Invite => self.invite(ctx),
            MembersOp::List => respond(self.accounts.roster(person), |roster| {
                (200, roster_json(&roster))
            }),
            MembersOp::SetRole => {
                let body: RoleRequest = match parse(&ctx.request.body, "role change") {
                    Ok(body) => body,
                    Err(response) => return Ok(response),
                };
                let Some(role) = OrgRole::parse(&body.role) else {
                    return Ok(unknown_role());
                };
                respond(self.accounts.set_member_role(person, id(), role), |()| {
                    (200, json!({ "id": id(), "role": role.as_str() }))
                })
            }
            MembersOp::Remove => respond(self.accounts.remove_member(person, id()), |()| {
                (200, json!({ "id": id(), "status": "removed" }))
            }),
            MembersOp::Revoke => respond(self.accounts.revoke_invitation(person, id()), |()| {
                (200, json!({ "id": id(), "status": "revoked" }))
            }),
        })
    }
}

impl MembersHandler {
    /// `POST /v1/invitations`: an owner or admin invites a teammate, as a member unless the body
    /// names another role.
    fn invite(&self, ctx: &HandlerCtx<'_>) -> EdgeResponse {
        let body: InviteRequest = match parse(&ctx.request.body, "invitation") {
            Ok(body) => body,
            Err(response) => return response,
        };
        let role = match body.role.as_deref().map(OrgRole::parse) {
            None => OrgRole::Member,
            Some(Some(role)) => role,
            Some(None) => return unknown_role(),
        };
        respond(
            self.accounts
                .invite_member(ctx.principal, &body.email, role),
            |invitation| {
                let mut body = match invitation {
                    MemberInvitation::Emailed {
                        email,
                        expires_at_unix,
                    } => json!({
                        "delivery": "email",
                        "email": email,
                        "expires_at": expires_at_unix,
                    }),
                    MemberInvitation::HandedOver(issued) => json!({
                        "delivery": "link",
                        "url": issued.url,
                        "email": issued.email,
                        "expires_at": issued.expires_at_unix,
                    }),
                };
                body["role"] = json!(role.as_str());
                (201, body)
            },
        )
    }
}

fn respond<T>(
    outcome: Result<T, AccountError>,
    ok: impl FnOnce(T) -> (u16, serde_json::Value),
) -> EdgeResponse {
    match outcome {
        Ok(value) => {
            let (status, body) = ok(value);
            no_store(EdgeResponse::json(status, &body))
        }
        Err(failure) => account_error(failure),
    }
}

fn unknown_role() -> EdgeResponse {
    error(&EdgeError::BadRequest(
        "role must be one of owner, admin or member".into(),
    ))
}

fn roster_json(roster: &RosterView) -> serde_json::Value {
    let people: Vec<_> = roster
        .people
        .iter()
        .map(|person| {
            json!({
                "id": person.principal_id,
                "display_name": person.display_name,
                "email": person.email,
                "role": person.role.as_str(),
                "joined_at": person.joined_at_unix,
            })
        })
        .collect();
    let mut body = json!({
        "role": roster.viewer.as_str(),
        "owners": roster.owners,
        "people": people,
    });
    if let Some(invitations) = &roster.invitations {
        body["invitations"] = invitations
            .iter()
            .map(|invitation| {
                json!({
                    "id": invitation.id,
                    "email": invitation.email,
                    "role": invitation.role.as_str(),
                    "expires_at": invitation.expires_at_unix,
                })
            })
            .collect();
    }
    body
}
