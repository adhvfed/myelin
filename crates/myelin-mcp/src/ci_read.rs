use crate::governance::ReadAuthorization;
use crate::server::{DirectReadError, DirectReadExecutor};
use myelin_ci_controlplane::surfacing_store::CI_LOG_RANGE_DEFAULT;
use myelin_edge::{error::EdgeError, DurableCiReadApi};
use myelin_identity::Principal;
use myelin_identity_service::mint::RunTokenAuthorizer;
use myelin_storage::TenantScope;
use serde_json::Value;
use std::collections::BTreeSet;
use std::sync::Arc;

pub struct CiDirectReadExecutor {
    api: DurableCiReadApi,
    authority: Arc<RunTokenAuthorizer>,
}

impl CiDirectReadExecutor {
    pub fn new(api: DurableCiReadApi, authority: Arc<RunTokenAuthorizer>) -> Self {
        Self { api, authority }
    }
}

impl DirectReadExecutor for CiDirectReadExecutor {
    fn execute(
        &self,
        principal: &Principal,
        authority: &ReadAuthorization,
        tool: &str,
        arguments: &Value,
    ) -> Result<Value, DirectReadError> {
        if authority.tool() != tool {
            return Err(DirectReadError::Denied);
        }
        let scope = TenantScope::from_verified_token(principal, principal.region.clone());
        self.authority
            .authorize(
                &scope,
                &principal.principal_id,
                authority.run_token(),
                authority.required_caps(),
            )
            .map_err(|_| DirectReadError::Denied)?;
        match tool {
            "ci.read_run" => {
                exact_fields(arguments, &["run_id"], &["run_id"])?;
                let run_id = required_string(arguments, "run_id")?;
                self.api.read_run(principal, run_id).map_err(map_edge_error)
            }
            "ci.read_log" => {
                exact_fields(
                    arguments,
                    &["run_id", "job_id"],
                    &["run_id", "job_id", "start", "limit"],
                )?;
                let run_id = required_string(arguments, "run_id")?;
                let job_id = required_string(arguments, "job_id")?;
                let start = optional_i64(arguments, "start")?.unwrap_or(0);
                let limit = optional_u32(arguments, "limit")?.unwrap_or(CI_LOG_RANGE_DEFAULT);
                self.api
                    .read_log(principal, run_id, job_id, start, limit)
                    .map_err(map_edge_error)
            }
            _ => Err(DirectReadError::Unavailable),
        }
    }
}

fn exact_fields(
    arguments: &Value,
    required: &[&str],
    allowed: &[&str],
) -> Result<(), DirectReadError> {
    let object = arguments
        .as_object()
        .ok_or_else(|| invalid("arguments must be an object"))?;
    let allowed = allowed.iter().copied().collect::<BTreeSet<_>>();
    if let Some(field) = object
        .keys()
        .find(|field| !allowed.contains(field.as_str()))
    {
        return Err(invalid(format!("unknown field `{field}`")));
    }
    if let Some(field) = required.iter().find(|field| !object.contains_key(**field)) {
        return Err(invalid(format!("missing field `{field}`")));
    }
    Ok(())
}

fn required_string<'a>(arguments: &'a Value, field: &str) -> Result<&'a str, DirectReadError> {
    arguments
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid(format!("`{field}` must be a string")))
}

fn optional_i64(arguments: &Value, field: &str) -> Result<Option<i64>, DirectReadError> {
    arguments
        .get(field)
        .map(|value| {
            value
                .as_i64()
                .ok_or_else(|| invalid(format!("`{field}` must be an integer")))
        })
        .transpose()
}

fn optional_u32(arguments: &Value, field: &str) -> Result<Option<u32>, DirectReadError> {
    arguments
        .get(field)
        .map(|value| {
            value
                .as_u64()
                .and_then(|value| u32::try_from(value).ok())
                .ok_or_else(|| invalid(format!("`{field}` must be a non-negative 32-bit integer")))
        })
        .transpose()
}

fn invalid(reason: impl Into<String>) -> DirectReadError {
    DirectReadError::InvalidInput(reason.into())
}

fn map_edge_error(error: EdgeError) -> DirectReadError {
    match error {
        EdgeError::BadRequest(reason) | EdgeError::Unprocessable(reason) => {
            DirectReadError::InvalidInput(reason)
        }
        EdgeError::NotFound(_) | EdgeError::Forbidden(_) | EdgeError::Unauthorized(_) => {
            DirectReadError::NotFound
        }
        EdgeError::Conflict(_)
        | EdgeError::PayloadTooLarge(_)
        | EdgeError::RequestTimeout(_)
        | EdgeError::Unavailable(_)
        | EdgeError::Internal(_) => DirectReadError::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn argument_grammar_is_exact_and_bounded_before_storage() {
        assert!(exact_fields(&json!({"run_id":"x"}), &["run_id"], &["run_id"]).is_ok());
        for invalid_args in [
            json!(null),
            json!({}),
            json!({"run_id":"x","tenant":"other"}),
        ] {
            assert!(exact_fields(&invalid_args, &["run_id"], &["run_id"]).is_err());
        }
        assert_eq!(
            optional_i64(&json!({"start": 0}), "start").unwrap(),
            Some(0)
        );
        assert!(optional_i64(&json!({"start": -1.5}), "start").is_err());
        assert_eq!(
            optional_u32(&json!({"limit": 262144}), "limit").unwrap(),
            Some(262_144)
        );
        assert!(optional_u32(&json!({"limit": -1}), "limit").is_err());
        assert!(optional_u32(&json!({"limit": 4294967296_u64}), "limit").is_err());
    }
}
