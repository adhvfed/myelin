use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use myelin_agent_model::{ModelClient, ModelError, ModelRequest, ModelResponse};
use myelin_storage::{AgentModelStepStore, ModelStepBegin, ModelStepCompletion, ModelStepError};
use myelin_tenancy::TenantId;

trait ModelStepJournal: Send + Sync {
    fn begin(
        &self,
        tenant: &TenantId,
        run_id: &str,
        step_key: &str,
        request_hash: &str,
        requested_by: &str,
    ) -> Result<ModelStepBegin, ModelStepError>;

    fn complete(
        &self,
        tenant: &TenantId,
        run_id: &str,
        step_key: &str,
        request_hash: &str,
        requested_by: &str,
        response: &serde_json::Value,
    ) -> Result<ModelStepCompletion, ModelStepError>;
}

impl ModelStepJournal for AgentModelStepStore {
    fn begin(
        &self,
        tenant: &TenantId,
        run_id: &str,
        step_key: &str,
        request_hash: &str,
        requested_by: &str,
    ) -> Result<ModelStepBegin, ModelStepError> {
        AgentModelStepStore::begin(self, tenant, run_id, step_key, request_hash, requested_by)
    }

    fn complete(
        &self,
        tenant: &TenantId,
        run_id: &str,
        step_key: &str,
        request_hash: &str,
        requested_by: &str,
        response: &serde_json::Value,
    ) -> Result<ModelStepCompletion, ModelStepError> {
        AgentModelStepStore::complete(
            self,
            tenant,
            run_id,
            step_key,
            request_hash,
            requested_by,
            response,
        )
    }
}

pub(crate) struct DurableModelClient {
    tenant: TenantId,
    run_id: String,
    requested_by: String,
    journal: Arc<dyn ModelStepJournal>,
    inner: Box<dyn ModelClient + Send + Sync>,
    next_turn: AtomicUsize,
}

impl DurableModelClient {
    pub(crate) fn new(
        tenant: TenantId,
        run_id: String,
        requested_by: String,
        journal: AgentModelStepStore,
        inner: Box<dyn ModelClient + Send + Sync>,
    ) -> Self {
        Self::with_journal(tenant, run_id, requested_by, Arc::new(journal), inner)
    }

    fn with_journal(
        tenant: TenantId,
        run_id: String,
        requested_by: String,
        journal: Arc<dyn ModelStepJournal>,
        inner: Box<dyn ModelClient + Send + Sync>,
    ) -> Self {
        Self {
            tenant,
            run_id,
            requested_by,
            journal,
            inner,
            next_turn: AtomicUsize::new(0),
        }
    }

    fn replay_refused(detail: impl core::fmt::Display) -> ModelError {
        ModelError::UnsafeReplay(format!(
            "durable model turn refused to risk a second provider call: {detail}"
        ))
    }
}

impl ModelClient for DurableModelClient {
    fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        let turn = self.next_turn.fetch_add(1, Ordering::SeqCst);
        let step_key = format!("model-turn/{turn}");
        let request_bytes = serde_json::to_vec(request)
            .map_err(|error| ModelError::Parse(format!("serialize model request: {error}")))?;
        let request_hash = blake3::hash(&request_bytes).to_hex().to_string();

        match self
            .journal
            .begin(
                &self.tenant,
                &self.run_id,
                &step_key,
                &request_hash,
                &self.requested_by,
            )
            .map_err(Self::replay_refused)?
        {
            ModelStepBegin::Completed(response) => {
                serde_json::from_value(response).map_err(|error| {
                    Self::replay_refused(format!("stored response is corrupt: {error}"))
                })
            }
            ModelStepBegin::InDoubt => Err(Self::replay_refused(
                "the prior process durably started this turn but did not durably finish it",
            )),
            ModelStepBegin::Unreplayable => Err(Self::replay_refused(
                "the legacy response was privacy-redacted during the ciphertext-only migration",
            )),
            ModelStepBegin::Started => {
                let response = self.inner.complete(request)?;
                let encoded = serde_json::to_value(&response).map_err(|error| {
                    ModelError::Parse(format!("serialize model response: {error}"))
                })?;
                self.journal
                    .complete(
                        &self.tenant,
                        &self.run_id,
                        &step_key,
                        &request_hash,
                        &self.requested_by,
                        &encoded,
                    )
                    .map_err(Self::replay_refused)?;
                Ok(response)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_agent_model::{ModelReply, Usage};
    use std::sync::Mutex;

    #[derive(Default)]
    struct MemoryJournal {
        state: Mutex<Option<(String, Option<serde_json::Value>)>>,
    }

    impl ModelStepJournal for MemoryJournal {
        fn begin(
            &self,
            _tenant: &TenantId,
            _run_id: &str,
            _step_key: &str,
            request_hash: &str,
            _requested_by: &str,
        ) -> Result<ModelStepBegin, ModelStepError> {
            let mut state = self.state.lock().unwrap();
            match state.as_ref() {
                None => {
                    *state = Some((request_hash.to_string(), None));
                    Ok(ModelStepBegin::Started)
                }
                Some((stored_hash, _)) if stored_hash != request_hash => {
                    Err(ModelStepError::Conflict)
                }
                Some((_, Some(response))) => Ok(ModelStepBegin::Completed(response.clone())),
                Some((_, None)) => Ok(ModelStepBegin::InDoubt),
            }
        }

        fn complete(
            &self,
            _tenant: &TenantId,
            _run_id: &str,
            _step_key: &str,
            request_hash: &str,
            _requested_by: &str,
            response: &serde_json::Value,
        ) -> Result<ModelStepCompletion, ModelStepError> {
            let mut state = self.state.lock().unwrap();
            match state.as_mut() {
                Some((stored_hash, stored_response)) if stored_hash == request_hash => {
                    if let Some(stored) = stored_response {
                        if stored == response {
                            Ok(ModelStepCompletion::Replayed)
                        } else {
                            Err(ModelStepError::Conflict)
                        }
                    } else {
                        *stored_response = Some(response.clone());
                        Ok(ModelStepCompletion::Applied)
                    }
                }
                Some(_) => Err(ModelStepError::Conflict),
                None => Err(ModelStepError::Missing),
            }
        }
    }

    struct CountingClient {
        calls: Arc<AtomicUsize>,
    }

    impl ModelClient for CountingClient {
        fn complete(&self, _request: &ModelRequest) -> Result<ModelResponse, ModelError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(answer())
        }
    }

    fn answer() -> ModelResponse {
        ModelResponse {
            reply: ModelReply::Final {
                content: "smallest safe fix".into(),
            },
            usage: Usage::Reported {
                input: 10,
                cached_input: 2,
                output: 3,
            },
        }
    }

    fn client(journal: Arc<MemoryJournal>, calls: Arc<AtomicUsize>) -> DurableModelClient {
        DurableModelClient::with_journal(
            TenantId("acme".into()),
            "run-1".into(),
            "founder".into(),
            journal,
            Box::new(CountingClient { calls }),
        )
    }

    #[test]
    fn completed_turn_replays_without_calling_the_provider_again() {
        let journal = Arc::new(MemoryJournal::default());
        let calls = Arc::new(AtomicUsize::new(0));
        let request = ModelRequest::default();

        assert_eq!(
            client(journal.clone(), calls.clone()).complete(&request),
            Ok(answer())
        );
        assert_eq!(
            client(journal, calls.clone()).complete(&request),
            Ok(answer())
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "the durable response, not a second provider call, serves workflow replay",
        );
    }

    #[test]
    fn ambiguous_turn_fails_closed_without_calling_the_provider() {
        let journal = Arc::new(MemoryJournal::default());
        let request = ModelRequest::default();
        let request_hash = blake3::hash(&serde_json::to_vec(&request).unwrap())
            .to_hex()
            .to_string();
        assert_eq!(
            journal.begin(
                &TenantId("acme".into()),
                "run-1",
                "model-turn/0",
                &request_hash,
                "founder",
            ),
            Ok(ModelStepBegin::Started),
        );
        let calls = Arc::new(AtomicUsize::new(0));

        let error = client(journal, calls.clone())
            .complete(&request)
            .expect_err("an indeterminate prior call is never repeated automatically");
        assert!(matches!(error, ModelError::UnsafeReplay(_)));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "no second provider call escaped"
        );
    }
}
