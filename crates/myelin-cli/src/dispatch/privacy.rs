use serde_json::json;

use super::{is_canonical_uuid, CliError, EdgeCall};

const REQUEST_USAGE: &str = "request erase <agent-data|chat-messages|issue-titles> --confirm | \
                             request status <request-id> | request certificate <request-id>";

pub fn privacy_dispatch(args: &[&str]) -> Result<EdgeCall, CliError> {
    match args {
        ["agent-data", "status"] => Ok(EdgeCall::get("/v1/privacy/me/agent-data")),
        ["agent-data", "erase", "--confirm"] => Ok(EdgeCall::post_retry_safe_json(
            "/v1/privacy/me/agent-data/erase",
            json!({}),
        )),
        ["agent-data", "erase"] => Err(CliError::Usage(
            "agent-data erasure is irreversible and permanently blocks new agent processing; \
             repeat the command with --confirm"
                .into(),
        )),
        ["agent-data", "erase", flag] => Err(CliError::Usage(format!(
            "unknown agent-data erase flag `{flag}` (expected --confirm)"
        ))),
        ["request", "erase", scope, "--confirm"] => Ok(EdgeCall::post_json(
            "/v1/privacy/me/requests",
            json!({ "kind": "erasure", "scope": request_scope(scope)? }),
        )),
        ["request", "erase", scope] => {
            request_scope(scope)?;
            Err(CliError::Usage(
                "privacy erasure is irreversible; repeat the command with --confirm".into(),
            ))
        }
        ["request", "status", request_id] => request_read(request_id, false),
        ["request", "certificate", request_id] => request_read(request_id, true),
        ["request", ..] => Err(CliError::Usage(format!(
            "malformed privacy request command (try: {REQUEST_USAGE})"
        ))),
        [] => Err(CliError::Usage(format!(
            "no privacy command given (try: agent-data status | agent-data erase --confirm | \
                 {REQUEST_USAGE})"
        ))),
        ["agent-data", command, ..] => Err(CliError::Usage(format!(
            "unknown agent-data command `{command}` (try: status | erase --confirm)"
        ))),
        [scope, ..] => Err(CliError::Usage(format!(
            "unknown privacy command `{scope}` (try: agent-data status | {REQUEST_USAGE})"
        ))),
    }
}

fn request_scope(scope: &str) -> Result<&'static str, CliError> {
    match scope {
        "agent-data" => Ok("agent_data"),
        "chat-messages" => Ok("chat_messages"),
        "issue-titles" => Ok("issue_titles"),
        _ => Err(CliError::Usage(format!(
            "unknown privacy request scope `{scope}` (expected agent-data, chat-messages, or \
             issue-titles)"
        ))),
    }
}

fn request_read(request_id: &str, certificate: bool) -> Result<EdgeCall, CliError> {
    if !is_canonical_uuid(request_id) {
        return Err(CliError::Usage(
            "privacy request id must be a canonical lowercase UUID".into(),
        ));
    }
    let suffix = if certificate { "/certificate" } else { "" };
    Ok(EdgeCall::get(format!(
        "/v1/privacy/me/requests/{request_id}{suffix}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::{HttpMethod, RetryPolicy};

    #[test]
    fn a_person_can_inspect_their_agent_data_without_mutating_it() {
        let call = privacy_dispatch(&["agent-data", "status"]).unwrap();
        assert_eq!(call.method, HttpMethod::Get);
        assert_eq!(call.path, "/v1/privacy/me/agent-data");
        assert_eq!(call.retry_policy, RetryPolicy::None);
        assert!(call.payload.is_none());
    }

    #[test]
    fn erasure_is_explicit_irreversible_and_intrinsically_retry_safe() {
        let warning = privacy_dispatch(&["agent-data", "erase"])
            .unwrap_err()
            .to_string();
        assert!(warning.contains("irreversible"));
        assert!(warning.contains("permanently blocks"));
        assert!(warning.contains("--confirm"));

        let call = privacy_dispatch(&["agent-data", "erase", "--confirm"]).unwrap();
        assert_eq!(call.method, HttpMethod::Post);
        assert_eq!(call.path, "/v1/privacy/me/agent-data/erase");
        assert_eq!(call.retry_policy, RetryPolicy::None);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(call.payload.as_ref().unwrap()).unwrap(),
            json!({})
        );
    }

    #[test]
    fn durable_requests_cover_every_truthful_holder_scope() {
        for (scope, wire_scope) in [
            ("agent-data", "agent_data"),
            ("chat-messages", "chat_messages"),
            ("issue-titles", "issue_titles"),
        ] {
            let warning = privacy_dispatch(&["request", "erase", scope])
                .unwrap_err()
                .to_string();
            assert!(warning.contains("irreversible"));
            assert!(warning.contains("--confirm"));

            let call = privacy_dispatch(&["request", "erase", scope, "--confirm"]).unwrap();
            assert_eq!(call.method, HttpMethod::Post);
            assert_eq!(call.path, "/v1/privacy/me/requests");
            assert_eq!(call.retry_policy, RetryPolicy::CallerKeyRequired);
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(call.payload.as_ref().unwrap())
                    .unwrap(),
                json!({ "kind": "erasure", "scope": wire_scope })
            );
        }
    }

    #[test]
    fn owned_request_status_and_certificate_are_strictly_addressed() {
        let request_id = "01234567-89ab-cdef-0123-456789abcdef";
        let status = privacy_dispatch(&["request", "status", request_id]).unwrap();
        assert_eq!(status.method, HttpMethod::Get);
        assert_eq!(
            status.path,
            "/v1/privacy/me/requests/01234567-89ab-cdef-0123-456789abcdef"
        );
        assert_eq!(status.retry_policy, RetryPolicy::None);

        let certificate = privacy_dispatch(&["request", "certificate", request_id]).unwrap();
        assert_eq!(
            certificate.path,
            "/v1/privacy/me/requests/01234567-89ab-cdef-0123-456789abcdef/certificate"
        );
        assert!(privacy_dispatch(&["request", "status", "NOT-A-UUID"]).is_err());
        assert!(privacy_dispatch(&["request", "erase", "everything", "--confirm"]).is_err());
    }
}
