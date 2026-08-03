//! # `MockModelClient` — the deterministic, network-free [`ModelClient`] double.
//!
//! Lets [`crate::runtime::LlmAgentRuntime::step`] be unit-tested WITHOUT a socket: script a
//! `Result<ModelResponse, ModelError>` and assert the mapped [`myelin_agent::StepOutcome`]. Behind
//! the `test-support` feature (and always available in-crate under `cfg(test)`) so downstream
//! service tests can construct an `LlmAgentRuntime` deterministically — the same in-memory-double
//! discipline the other crates use.

use crate::client::{ModelClient, ModelError, ModelRequest, ModelResponse};
use std::sync::Mutex;

/// A scripted [`ModelClient`]: every [`ModelClient::complete`] returns the scripted result (cloned),
/// and the last request is captured for assertions. Never touches the network.
#[derive(Debug)]
pub struct MockModelClient {
    scripted: Result<ModelResponse, ModelError>,
    last_request: Mutex<Option<ModelRequest>>,
}

impl MockModelClient {
    /// Script a successful response.
    pub fn ok(response: ModelResponse) -> MockModelClient {
        MockModelClient {
            scripted: Ok(response),
            last_request: Mutex::new(None),
        }
    }

    /// Script a typed error (the HTTP/parse/transport failure path).
    pub fn err(error: ModelError) -> MockModelClient {
        MockModelClient {
            scripted: Err(error),
            last_request: Mutex::new(None),
        }
    }

    /// The most recent request the runtime built (for asserting the Conversation → request mapping).
    pub fn last_request(&self) -> Option<ModelRequest> {
        self.last_request.lock().expect("mock lock").clone()
    }
}

impl ModelClient for MockModelClient {
    fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        *self.last_request.lock().expect("mock lock") = Some(request.clone());
        self.scripted.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{ModelReply, Usage};

    #[test]
    fn mock_returns_the_scripted_result_and_captures_the_request() {
        let mock = MockModelClient::ok(ModelResponse {
            reply: ModelReply::Final {
                content: "hi".into(),
            },
            usage: Usage::NotReported,
        });
        let request = ModelRequest {
            system: "sys".into(),
            ..Default::default()
        };
        let resp = mock.complete(&request).unwrap();
        assert!(matches!(resp.reply, ModelReply::Final { .. }));
        assert_eq!(mock.last_request().unwrap().system, "sys");
    }

    #[test]
    fn mock_err_is_returned() {
        let mock = MockModelClient::err(ModelError::MissingApiKey);
        assert!(matches!(
            mock.complete(&ModelRequest::default()),
            Err(ModelError::MissingApiKey)
        ));
    }
}
