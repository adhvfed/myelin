use crate::client::execute_device_auth;
use crate::config::{Credential, EdgeConfig, SESSION_SCHEME};
use crate::dispatch::{EdgeCall, HttpMethod};
use crate::error::CliError;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use hyper::Uri;
use rand::{rngs::OsRng, RngCore};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const SECRET_BYTES: usize = 32;
const SECRET_LENGTH: usize = 43;
const MAX_LOGIN_SECONDS: u64 = 15 * 60;
const MAX_POLL_INTERVAL_SECONDS: u64 = 30;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceAuthorizationRequest {
    pub code_verifier: String,
    pub code_challenge: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceAuthorization {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthorizedCredential {
    pub credential: Credential,
}

impl AuthorizedCredential {
    pub fn expires_at_unix(&self) -> i64 {
        self.credential
            .expires_at_unix
            .expect("an authorized session always carries its expiry")
    }
}

enum Claim {
    Pending { interval: u64 },
    Authorized(AuthorizedCredential),
}

pub fn new_authorization_request() -> DeviceAuthorizationRequest {
    let mut verifier = [0_u8; SECRET_BYTES];
    OsRng.fill_bytes(&mut verifier);
    let code_verifier = URL_SAFE_NO_PAD.encode(verifier);
    let code_challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(code_verifier.as_bytes()));
    DeviceAuthorizationRequest {
        code_verifier,
        code_challenge,
    }
}

pub async fn begin_authorization(
    edge: &EdgeConfig,
    request: &DeviceAuthorizationRequest,
) -> Result<DeviceAuthorization, CliError> {
    if !canonical_secret(&request.code_verifier) || !canonical_secret(&request.code_challenge) {
        return malformed("local verifier material is invalid");
    }
    let response = execute_device_auth(
        edge,
        &auth_call(
            "/v1/auth/device/authorization",
            json!({ "code_challenge": request.code_challenge }),
        )?,
    )
    .await?;
    parse_authorization(response)
}

pub async fn wait_for_authorization(
    edge: &EdgeConfig,
    request: &DeviceAuthorizationRequest,
    authorization: &DeviceAuthorization,
) -> Result<AuthorizedCredential, CliError> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(authorization.expires_in);
    let mut interval = authorization.interval;
    loop {
        tokio::time::sleep(Duration::from_secs(interval)).await;
        if tokio::time::Instant::now() >= deadline {
            return Err(CliError::NotAuthenticated(
                "the browser approval expired; run `myelin auth login` again".into(),
            ));
        }
        let response = match execute_device_auth(
            edge,
            &auth_call(
                "/v1/auth/device/token",
                json!({
                    "device_code": authorization.device_code,
                    "code_verifier": request.code_verifier,
                }),
            )?,
        )
        .await
        {
            Ok(response) => response,
            Err(error) if error.is_retryable_response_loss() => continue,
            Err(error) => return Err(error),
        };
        match parse_claim(response)? {
            Claim::Authorized(credential) => return Ok(credential),
            Claim::Pending {
                interval: server_interval,
            } => interval = interval.max(server_interval),
        }
    }
}

fn auth_call(path: &str, body: Value) -> Result<EdgeCall, CliError> {
    let payload = serde_json::to_vec(&body)
        .map_err(|error| CliError::Config(format!("cannot encode login request: {error}")))?;
    Ok(EdgeCall {
        method: HttpMethod::Post,
        path: path.into(),
        query: None,
        payload: Some(payload),
        idempotency_key: None,
    })
}

fn parse_authorization(value: Value) -> Result<DeviceAuthorization, CliError> {
    let object = object(&value, "device authorization")?;
    let device_code = bounded_string(object, "device_code", 128)?;
    let user_code = bounded_string(object, "user_code", 16)?;
    let verification_uri = bounded_string(object, "verification_uri", 2_048)?;
    let verification_uri_complete = bounded_string(object, "verification_uri_complete", 2_096)?;
    let expires_in = bounded_u64(object, "expires_in", 1, MAX_LOGIN_SECONDS)?;
    let interval = bounded_u64(object, "interval", 1, MAX_POLL_INTERVAL_SECONDS)?;

    if !canonical_secret(&device_code) {
        return malformed("device code is not a canonical secret");
    }
    if !canonical_user_code(&user_code) {
        return malformed("user code is not canonical");
    }
    validate_verification_uri(&verification_uri)?;
    if verification_uri_complete != format!("{verification_uri}?code={user_code}") {
        return malformed("complete verification URI is not bound to the user code");
    }
    Ok(DeviceAuthorization {
        device_code,
        user_code,
        verification_uri,
        verification_uri_complete,
        expires_in,
        interval,
    })
}

fn parse_claim(value: Value) -> Result<Claim, CliError> {
    let object = object(&value, "device token response")?;
    if object.get("status").and_then(Value::as_str) == Some("authorization_pending") {
        let interval = bounded_u64(object, "interval", 1, MAX_POLL_INTERVAL_SECONDS)?;
        return Ok(Claim::Pending { interval });
    }

    let token = bounded_string(object, "access_token", 32 * 1024)?;
    let token_type = bounded_string(object, "token_type", 16)?;
    let scheme = bounded_string(object, "scheme", 32)?;
    let expires_at = object
        .get("expires_at")
        .and_then(Value::as_i64)
        .ok_or_else(|| malformed_error("device token expiry is missing"))?;
    if token_type != "Bearer"
        || scheme != SESSION_SCHEME
        || token.is_empty()
        || !token.as_bytes().iter().all(u8::is_ascii_graphic)
        || expires_at <= now_unix()
    {
        return malformed("authorized credential has an invalid shape");
    }
    Ok(Claim::Authorized(AuthorizedCredential {
        credential: Credential {
            token,
            scheme,
            edge_url: None,
            expires_at_unix: Some(expires_at),
        },
    }))
}

fn object<'a>(value: &'a Value, context: &str) -> Result<&'a Map<String, Value>, CliError> {
    value
        .as_object()
        .ok_or_else(|| malformed_error(format!("{context} is not an object")))
}

fn bounded_string(
    object: &Map<String, Value>,
    field: &str,
    max_bytes: usize,
) -> Result<String, CliError> {
    object
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= max_bytes)
        .map(str::to_string)
        .ok_or_else(|| malformed_error(format!("{field} is missing or outside its bounds")))
}

fn bounded_u64(
    object: &Map<String, Value>,
    field: &str,
    minimum: u64,
    maximum: u64,
) -> Result<u64, CliError> {
    object
        .get(field)
        .and_then(Value::as_u64)
        .filter(|value| (minimum..=maximum).contains(value))
        .ok_or_else(|| malformed_error(format!("{field} is missing or outside its bounds")))
}

fn canonical_secret(value: &str) -> bool {
    value.len() == SECRET_LENGTH
        && URL_SAFE_NO_PAD
            .decode(value)
            .is_ok_and(|decoded| decoded.len() == SECRET_BYTES)
}

fn canonical_user_code(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 9
        && bytes[4] == b'-'
        && bytes.iter().enumerate().all(|(index, byte)| {
            index == 4 || matches!(byte, b'A'..=b'H' | b'J'..=b'N' | b'P'..=b'Z' | b'2'..=b'9')
        })
}

fn validate_verification_uri(value: &str) -> Result<(), CliError> {
    let uri: Uri = value
        .parse()
        .map_err(|_| malformed_error("verification URI is invalid"))?;
    let scheme = uri.scheme_str();
    let authority = uri.authority();
    if !matches!(scheme, Some("http" | "https"))
        || authority.is_none()
        || authority.is_some_and(|value| value.as_str().contains('@'))
        || uri.query().is_some()
        || uri.path().is_empty()
        || uri.path() == "/"
    {
        return malformed("verification URI is not a credential-free HTTP(S) URL with a path");
    }
    if scheme == Some("http")
        && !authority
            .expect("checked above")
            .host()
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
        && !authority
            .expect("checked above")
            .host()
            .eq_ignore_ascii_case("localhost")
    {
        return malformed("verification URI refuses clear-text transport outside loopback");
    }
    Ok(())
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn malformed<T>(message: impl Into<String>) -> Result<T, CliError> {
    Err(malformed_error(message))
}

fn malformed_error(message: impl Into<String>) -> CliError {
    CliError::Transport(format!(
        "Edge returned a malformed device authorization response: {}",
        message.into()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifier_and_challenge_are_canonical_independent_secrets() {
        let request = new_authorization_request();
        assert!(canonical_secret(&request.code_verifier));
        assert!(canonical_secret(&request.code_challenge));
        assert_ne!(request.code_verifier, request.code_challenge);
        assert_eq!(
            request.code_challenge,
            URL_SAFE_NO_PAD.encode(Sha256::digest(request.code_verifier.as_bytes()))
        );
    }

    #[test]
    fn authorization_response_binds_the_browser_url_to_the_human_code() {
        let device_code = URL_SAFE_NO_PAD.encode([7_u8; SECRET_BYTES]);
        let authorization = parse_authorization(json!({
            "device_code": device_code,
            "user_code": "ABCD-EFGH",
            "verification_uri": "https://myelin.example/cli/auth",
            "verification_uri_complete": "https://myelin.example/cli/auth?code=ABCD-EFGH",
            "expires_in": 600,
            "interval": 2,
        }))
        .unwrap();
        assert_eq!(authorization.user_code, "ABCD-EFGH");

        for poisoned in [
            "https://outside.example/cli/auth?code=ABCD-EFGH",
            "https://myelin.example/cli/auth?code=WXYZ-2345",
            "http://myelin.example/cli/auth?code=ABCD-EFGH",
        ] {
            let response = json!({
                "device_code": URL_SAFE_NO_PAD.encode([7_u8; SECRET_BYTES]),
                "user_code": "ABCD-EFGH",
                "verification_uri": "https://myelin.example/cli/auth",
                "verification_uri_complete": poisoned,
                "expires_in": 600,
                "interval": 2,
            });
            assert!(parse_authorization(response).is_err());
        }
    }

    #[test]
    fn polling_accepts_only_pending_or_a_live_human_session() {
        assert!(matches!(
            parse_claim(json!({"status":"authorization_pending", "interval":2})).unwrap(),
            Claim::Pending { interval: 2 }
        ));
        let accepted = parse_claim(json!({
            "access_token": "SIGNED_SESSION",
            "token_type": "Bearer",
            "scheme": "session",
            "expires_at": now_unix() + 600,
        }))
        .unwrap();
        assert!(matches!(accepted, Claim::Authorized(_)));

        for malformed in [
            json!({"status":"authorization_pending", "interval":0}),
            json!({
                "access_token": "SIGNED_SESSION",
                "token_type": "Bearer",
                "scheme": "agent",
                "expires_at": now_unix() + 600,
            }),
            json!({
                "access_token": "SIGNED_SESSION",
                "token_type": "Bearer",
                "scheme": "session",
                "expires_at": now_unix() - 1,
            }),
        ] {
            assert!(parse_claim(malformed).is_err());
        }
    }
}
