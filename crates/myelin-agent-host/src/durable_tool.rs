use std::sync::Arc;

use myelin_agent::{ToolCall, ToolDef, ToolResult};
use myelin_agent_service::{ToolExecError, ToolExecutionContext, ToolExecutor};
use myelin_storage::{
    AgentToolEffectStore, ToolEffectBegin, ToolEffectCompletion, ToolEffectError,
};
use myelin_tenancy::TenantId;

trait ToolEffectJournal: Send + Sync {
    fn begin(
        &self,
        tenant: &TenantId,
        run_id: &str,
        effect_key: &str,
        request_hash: &str,
        requested_by: &str,
    ) -> Result<ToolEffectBegin, ToolEffectError>;

    fn complete(
        &self,
        tenant: &TenantId,
        run_id: &str,
        effect_key: &str,
        request_hash: &str,
        requested_by: &str,
        result: &str,
    ) -> Result<ToolEffectCompletion, ToolEffectError>;
}

impl ToolEffectJournal for AgentToolEffectStore {
    fn begin(
        &self,
        tenant: &TenantId,
        run_id: &str,
        effect_key: &str,
        request_hash: &str,
        requested_by: &str,
    ) -> Result<ToolEffectBegin, ToolEffectError> {
        AgentToolEffectStore::begin(self, tenant, run_id, effect_key, request_hash, requested_by)
    }

    fn complete(
        &self,
        tenant: &TenantId,
        run_id: &str,
        effect_key: &str,
        request_hash: &str,
        requested_by: &str,
        result: &str,
    ) -> Result<ToolEffectCompletion, ToolEffectError> {
        AgentToolEffectStore::complete(
            self,
            tenant,
            run_id,
            effect_key,
            request_hash,
            requested_by,
            result,
        )
    }
}

pub(crate) struct DurableToolExecutor<'a> {
    tenant: TenantId,
    requested_by: String,
    journal: Arc<dyn ToolEffectJournal>,
    inner: &'a dyn ToolExecutor,
}

impl<'a> DurableToolExecutor<'a> {
    pub(crate) fn new(
        tenant: TenantId,
        requested_by: String,
        journal: AgentToolEffectStore,
        inner: &'a dyn ToolExecutor,
    ) -> Self {
        Self::with_journal(tenant, requested_by, Arc::new(journal), inner)
    }

    fn with_journal(
        tenant: TenantId,
        requested_by: String,
        journal: Arc<dyn ToolEffectJournal>,
        inner: &'a dyn ToolExecutor,
    ) -> Self {
        Self {
            tenant,
            requested_by,
            journal,
            inner,
        }
    }
}

impl ToolExecutor for DurableToolExecutor<'_> {
    fn execute(
        &self,
        context: &ToolExecutionContext<'_>,
        definition: &ToolDef,
        call: &ToolCall,
    ) -> Result<ToolResult, ToolExecError> {
        let request_hash = tool_request_hash(definition, call)?;
        match self
            .journal
            .begin(
                &self.tenant,
                context.run_id,
                context.effect_key,
                &request_hash,
                &self.requested_by,
            )
            .map_err(journal_failed)?
        {
            ToolEffectBegin::Completed(result) => return decode_result(&result),
            ToolEffectBegin::Unreplayable => {
                return Err(journal_failed(ToolEffectError::Unreplayable));
            }
            ToolEffectBegin::Indeterminate => {
                return Err(ToolExecError::Failed(
                    "tool effect was admitted but has no durable result; repeating it is refused"
                        .into(),
                ));
            }
            ToolEffectBegin::Execute => {}
        }

        let observed = self.inner.execute(context, definition, call)?;
        let encoded = serde_json::to_string(&observed)
            .map_err(|error| ToolExecError::Failed(format!("serialize tool result: {error}")))?;
        match self
            .journal
            .complete(
                &self.tenant,
                context.run_id,
                context.effect_key,
                &request_hash,
                &self.requested_by,
                &encoded,
            )
            .map_err(journal_failed)?
        {
            ToolEffectCompletion::Applied => Ok(observed),
            ToolEffectCompletion::Replayed(canonical) => decode_result(&canonical),
        }
    }
}

fn decode_result(stored: &str) -> Result<ToolResult, ToolExecError> {
    serde_json::from_str(stored).map_err(|_| {
        ToolExecError::Failed("durable tool replay result has invalid encoding".into())
    })
}

pub(crate) fn tool_request_hash(
    definition: &ToolDef,
    call: &ToolCall,
) -> Result<String, ToolExecError> {
    let identity = serde_json::json!({
        "tool": definition.canonical_name(),
        "version": definition.version,
        "arguments": canonical_json(&call.arguments),
    });
    let bytes = serde_json::to_vec(&identity)
        .map_err(|error| ToolExecError::Failed(format!("serialize tool effect: {error}")))?;
    Ok(blake3::hash(&bytes).to_hex().to_string())
}

fn canonical_json(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(object) => {
            let mut keys = object.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            let mut canonical = serde_json::Map::new();
            for key in keys {
                canonical.insert(key.clone(), canonical_json(&object[key]));
            }
            serde_json::Value::Object(canonical)
        }
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.iter().map(canonical_json).collect())
        }
        scalar => scalar.clone(),
    }
}

fn journal_failed(error: ToolEffectError) -> ToolExecError {
    ToolExecError::Failed(format!("durable tool replay refused: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_agent::{EffectKind, ToolCallId, ToolName};
    use myelin_flow::RunTokenHandle;
    use std::collections::{HashMap, VecDeque};
    use std::sync::Mutex;

    type EffectIdentity = (String, String, String);
    type EffectState = (String, Option<String>);

    #[derive(Default)]
    struct MemoryJournal {
        effects: Mutex<HashMap<EffectIdentity, EffectState>>,
    }

    impl ToolEffectJournal for MemoryJournal {
        fn begin(
            &self,
            tenant: &TenantId,
            run_id: &str,
            effect_key: &str,
            request_hash: &str,
            _requested_by: &str,
        ) -> Result<ToolEffectBegin, ToolEffectError> {
            let identity = (tenant.0.clone(), run_id.into(), effect_key.into());
            let mut effects = self.effects.lock().unwrap();
            match effects.get(&identity) {
                None => {
                    effects.insert(identity, (request_hash.into(), None));
                    Ok(ToolEffectBegin::Execute)
                }
                Some((stored_hash, _)) if stored_hash != request_hash => {
                    Err(ToolEffectError::Conflict)
                }
                Some((_, Some(result))) => Ok(ToolEffectBegin::Completed(result.clone())),
                Some((_, None)) => Ok(ToolEffectBegin::Execute),
            }
        }

        fn complete(
            &self,
            tenant: &TenantId,
            run_id: &str,
            effect_key: &str,
            request_hash: &str,
            _requested_by: &str,
            result: &str,
        ) -> Result<ToolEffectCompletion, ToolEffectError> {
            let identity = (tenant.0.clone(), run_id.into(), effect_key.into());
            let mut effects = self.effects.lock().unwrap();
            match effects.get_mut(&identity) {
                None => Err(ToolEffectError::Missing),
                Some((stored_hash, _)) if stored_hash != request_hash => {
                    Err(ToolEffectError::Conflict)
                }
                Some((_, Some(stored))) => Ok(ToolEffectCompletion::Replayed(stored.clone())),
                Some((_, stored @ None)) => {
                    *stored = Some(result.into());
                    Ok(ToolEffectCompletion::Applied)
                }
            }
        }
    }

    struct ScriptedExecutor {
        outcomes: Mutex<VecDeque<Result<ToolResult, ToolExecError>>>,
        call_count: Mutex<usize>,
    }

    impl ScriptedExecutor {
        fn with(outcomes: impl IntoIterator<Item = Result<ToolResult, ToolExecError>>) -> Self {
            Self {
                outcomes: Mutex::new(outcomes.into_iter().collect()),
                call_count: Mutex::new(0),
            }
        }

        fn call_count(&self) -> usize {
            *self.call_count.lock().unwrap()
        }
    }

    impl ToolExecutor for ScriptedExecutor {
        fn execute(
            &self,
            _context: &ToolExecutionContext<'_>,
            _definition: &ToolDef,
            _call: &ToolCall,
        ) -> Result<ToolResult, ToolExecError> {
            *self.call_count.lock().unwrap() += 1;
            self.outcomes
                .lock()
                .unwrap()
                .pop_front()
                .expect("the test supplies every external result")
        }
    }

    fn definition() -> ToolDef {
        ToolDef {
            name: ToolName("create".into()),
            subsystem: "issues".into(),
            version: 1,
            input_schema: r#"{"type":"object"}"#.into(),
            required_caps: vec!["issue.create".into()],
            effect_kind: EffectKind::Mutate,
            side_effecting: true,
            requires_approval: false,
            exposed_over_mcp: true,
        }
    }

    fn call(id: &str) -> ToolCall {
        ToolCall {
            id: ToolCallId(id.into()),
            name: ToolName("issues.create".into()),
            arguments: serde_json::json!({"title": "One bug", "labels": ["ci"]}),
        }
    }

    fn context() -> ToolExecutionContext<'static> {
        let token = Box::leak(Box::new(RunTokenHandle {
            token: "secret-test-token".into(),
            jti: "test-jti".into(),
            ttl_secs: 60,
        }));
        ToolExecutionContext {
            run_id: "run-1",
            run_token: token,
            effect_key: "model-turn/0/tool/0",
        }
    }

    #[test]
    fn request_identity_ignores_provider_ids_but_not_logical_content() {
        let first = call("provider-a");
        let mut renamed = first.clone();
        renamed.id.0 = "provider-b".into();
        assert_eq!(
            tool_request_hash(&definition(), &first).unwrap(),
            tool_request_hash(&definition(), &renamed).unwrap(),
        );

        let mut changed = first;
        changed.arguments["title"] = serde_json::json!("Another bug");
        assert_ne!(
            tool_request_hash(&definition(), &changed).unwrap(),
            tool_request_hash(&definition(), &renamed).unwrap(),
        );
    }

    #[test]
    fn malformed_replay_fails_closed_without_echoing_stored_content() {
        let secret = "malformed-secret-result";
        let error = decode_result(secret).expect_err("corrupt durable output is never a success");

        assert_eq!(
            error,
            ToolExecError::Failed("durable tool replay result has invalid encoding".into())
        );
        assert!(
            !error.to_string().contains(secret),
            "the diagnostic must not disclose the corrupt stored payload"
        );
    }

    #[test]
    fn completed_effect_replays_exact_bytes_after_process_reconstruction() {
        let journal = Arc::new(MemoryJournal::default());
        let first_process =
            ScriptedExecutor::with([Ok(ToolResult::Succeeded("first snapshot".into()))]);
        let first = DurableToolExecutor::with_journal(
            TenantId("acme".into()),
            "founder".into(),
            journal.clone(),
            &first_process,
        );

        assert_eq!(
            first.execute(&context(), &definition(), &call("provider-a")),
            Ok(ToolResult::Succeeded("first snapshot".into())),
        );
        assert_eq!(first_process.call_count(), 1);

        let restarted_process =
            ScriptedExecutor::with([Ok(ToolResult::Succeeded("a changed snapshot".into()))]);
        let replay = DurableToolExecutor::with_journal(
            TenantId("acme".into()),
            "founder".into(),
            journal,
            &restarted_process,
        );
        assert_eq!(
            replay.execute(&context(), &definition(), &call("provider-b")),
            Ok(ToolResult::Succeeded("first snapshot".into())),
            "replay returns the original observation even when the provider renames the call",
        );
        assert_eq!(
            restarted_process.call_count(),
            0,
            "completed work never reaches the external subsystem again",
        );
    }

    #[test]
    fn governed_refusal_remains_a_refusal_after_process_reconstruction() {
        let journal = Arc::new(MemoryJournal::default());
        let first_process = ScriptedExecutor::with([Ok(ToolResult::Refused {
            refused: "project is unavailable".into(),
        })]);
        let first = DurableToolExecutor::with_journal(
            TenantId("acme".into()),
            "founder".into(),
            journal.clone(),
            &first_process,
        );

        assert_eq!(
            first.execute(&context(), &definition(), &call("provider-a")),
            Ok(ToolResult::Refused {
                refused: "project is unavailable".into(),
            }),
        );

        let restarted_process =
            ScriptedExecutor::with([Ok(ToolResult::Succeeded("should not escape".into()))]);
        let replay = DurableToolExecutor::with_journal(
            TenantId("acme".into()),
            "founder".into(),
            journal,
            &restarted_process,
        );
        assert_eq!(
            replay.execute(&context(), &definition(), &call("provider-b")),
            Ok(ToolResult::Refused {
                refused: "project is unavailable".into(),
            }),
        );
        assert_eq!(restarted_process.call_count(), 0);
    }

    #[test]
    fn approval_wait_leaves_the_same_effect_retryable_then_caches_success() {
        let journal = Arc::new(MemoryJournal::default());
        let external = ScriptedExecutor::with([
            Err(ToolExecError::ApprovalRequired {
                gate_id: "gate-1".into(),
            }),
            Ok(ToolResult::Succeeded("merged once".into())),
        ]);
        let durable = DurableToolExecutor::with_journal(
            TenantId("acme".into()),
            "founder".into(),
            journal,
            &external,
        );

        assert_eq!(
            durable.execute(&context(), &definition(), &call("provider-a")),
            Err(ToolExecError::ApprovalRequired {
                gate_id: "gate-1".into(),
            }),
        );
        assert_eq!(
            durable.execute(&context(), &definition(), &call("provider-b")),
            Ok(ToolResult::Succeeded("merged once".into())),
            "the approved retry keeps the original logical effect identity",
        );
        assert_eq!(
            durable.execute(&context(), &definition(), &call("provider-c")),
            Ok(ToolResult::Succeeded("merged once".into())),
        );
        assert_eq!(
            external.call_count(),
            2,
            "one gate attempt and one successful effect escaped; replay stayed local",
        );
    }
}
