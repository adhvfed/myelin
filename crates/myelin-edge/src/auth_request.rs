use serde::de::DeserializeOwned;
use serde::Deserialize;

use crate::error::EdgeError;

const MAX_AUTH_REQUEST_BYTES: usize = 4 * 1024;

pub(crate) fn parse_auth_request<T: DeserializeOwned>(
    bytes: &[u8],
    operation: &str,
) -> Result<T, EdgeError> {
    require_auth_request_budget(bytes)?;
    if bytes.is_empty() {
        return Err(EdgeError::BadRequest(format!(
            "{operation} request body is empty"
        )));
    }
    serde_json::from_slice(bytes)
        .map_err(|error| EdgeError::BadRequest(format!("invalid {operation} request: {error}")))
}

pub(crate) fn require_auth_request_budget(bytes: &[u8]) -> Result<(), EdgeError> {
    if bytes.len() > MAX_AUTH_REQUEST_BYTES {
        return Err(EdgeError::PayloadTooLarge(
            "authentication request exceeds 4 KiB".into(),
        ));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeviceStartRequest {
    pub(crate) code_challenge: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeviceApprovalRequest {
    pub(crate) user_code: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeviceClaimRequest {
    pub(crate) device_code: String,
    pub(crate) code_verifier: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LoginRequest {
    pub(crate) scheme: String,
    pub(crate) material: String,
    pub(crate) nonce: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authentication_requests_have_one_exact_shape() {
        let start: DeviceStartRequest =
            parse_auth_request(br#"{"code_challenge":"challenge"}"#, "device login start").unwrap();
        assert_eq!(start.code_challenge, "challenge");

        assert!(parse_auth_request::<DeviceStartRequest>(
            br#"{"code_challenge":"challenge","method":"S256"}"#,
            "device login start",
        )
        .is_err());
        assert!(parse_auth_request::<DeviceClaimRequest>(
            br#"{"device_code":"device","code_verifier":"proof","poll":true}"#,
            "device login claim",
        )
        .is_err());
        assert!(parse_auth_request::<LoginRequest>(
            br#"{"scheme":"oidc","material":"assertion","nonce":"nonce","redirect":"/"}"#,
            "login",
        )
        .is_err());
    }

    #[test]
    fn every_authentication_request_shares_the_small_body_budget() {
        assert!(matches!(
            parse_auth_request::<DeviceApprovalRequest>(
                &vec![b'x'; MAX_AUTH_REQUEST_BYTES + 1],
                "device approval"
            ),
            Err(EdgeError::PayloadTooLarge(_))
        ));
    }
}
