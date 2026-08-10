use serde_json::json;

use super::{CliError, EdgeCall};

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
        [] => Err(CliError::Usage(
            "no privacy command given (try: agent-data status | agent-data erase --confirm)".into(),
        )),
        ["agent-data", command, ..] => Err(CliError::Usage(format!(
            "unknown agent-data command `{command}` (try: status | erase --confirm)"
        ))),
        [scope, ..] => Err(CliError::Usage(format!(
            "unknown privacy scope `{scope}` (try: agent-data status)"
        ))),
    }
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
}
