use myelin_agent::RuntimeStepError;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallRequest {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallResult {
    pub id: String,
    pub content: String,
    #[serde(default)]
    pub is_error: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelTurn {
    User {
        content: String,
    },
    Assistant {
        content: Option<String>,
        tool_calls: Vec<ToolCallRequest>,
    },
    ToolResults(Vec<ToolCallResult>),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ModelRequest {
    pub system: String,
    pub turns: Vec<ModelTurn>,
    pub tools: Vec<ToolSpec>,
    pub max_output_tokens: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelReply {
    ToolCalls(Vec<ToolCallRequest>),
    Final { content: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Usage {
    Reported {
        input: u64,
        cached_input: u64,
        output: u64,
    },
    NotReported,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelResponse {
    pub reply: ModelReply,
    pub usage: Usage,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelError {
    MissingApiKey,
    Transport(String),
    Http { status: u16, body: String },
    Parse(String),
    UnsafeReplay(String),
}

impl ModelError {
    pub fn runtime_step_error(&self) -> RuntimeStepError {
        match self {
            Self::MissingApiKey => RuntimeStepError::Misconfigured,
            Self::Transport(_) => RuntimeStepError::Unavailable,
            Self::Http { status, .. } => RuntimeStepError::Rejected {
                status: Some(*status),
            },
            Self::Parse(_) => RuntimeStepError::InvalidResponse,
            Self::UnsafeReplay(_) => RuntimeStepError::UnsafeReplay,
        }
    }
}

impl core::fmt::Display for ModelError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ModelError::MissingApiKey => {
                write!(f, "model API key is not set (fail-closed; no call made)")
            }
            ModelError::Transport(m) => write!(f, "model transport error: {m}"),
            ModelError::Http { status, body } => {
                write!(f, "model provider returned HTTP {status}: {body}")
            }
            ModelError::Parse(m) => write!(f, "model response parse error: {m}"),
            ModelError::UnsafeReplay(m) => write!(f, "model replay refused: {m}"),
        }
    }
}

impl std::error::Error for ModelError {}

pub trait ModelClient {
    fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError>;
}
