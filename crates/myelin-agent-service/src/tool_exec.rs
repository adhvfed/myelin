use myelin_agent::{ToolCall, ToolDef, ToolResult};
use myelin_flow::RunTokenHandle;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolExecError {
    Failed(String),
}

impl core::fmt::Display for ToolExecError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ToolExecError::Failed(m) => write!(f, "tool execution failed: {m}"),
        }
    }
}

impl std::error::Error for ToolExecError {}

pub trait ToolExecutor: Send + Sync {
    fn execute(
        &self,
        context: &ToolExecutionContext<'_>,
        def: &ToolDef,
        call: &ToolCall,
    ) -> Result<ToolResult, ToolExecError>;
}

pub struct ToolExecutionContext<'a> {
    pub run_id: &'a str,
    pub run_token: &'a RunTokenHandle,
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Clone, Debug, Default)]
pub struct MockToolSurface {
    defs: Vec<ToolDef>,
}

#[cfg(any(test, feature = "test-support"))]
impl MockToolSurface {
    pub fn new() -> MockToolSurface {
        MockToolSurface::default()
    }

    pub fn with(defs: impl IntoIterator<Item = ToolDef>) -> MockToolSurface {
        MockToolSurface {
            defs: defs.into_iter().collect(),
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl myelin_agent::ToolSurface for MockToolSurface {
    fn register_tool(&mut self, def: ToolDef) {
        self.defs.push(def);
    }
    fn resolve(&self, name: &myelin_agent::ToolName) -> Option<&ToolDef> {
        self.defs.iter().find(|d| &d.name == name)
    }
}

#[cfg(any(test, feature = "test-support"))]
pub struct MockToolExecutor {
    scripted: std::sync::Mutex<std::collections::VecDeque<Result<ToolResult, ToolExecError>>>,
    seen: std::sync::Mutex<Vec<ToolCall>>,
}

#[cfg(any(test, feature = "test-support"))]
impl MockToolExecutor {
    pub fn new() -> MockToolExecutor {
        MockToolExecutor {
            scripted: std::sync::Mutex::new(std::collections::VecDeque::new()),
            seen: std::sync::Mutex::new(Vec::new()),
        }
    }

    pub fn with_results(
        results: impl IntoIterator<Item = Result<ToolResult, ToolExecError>>,
    ) -> MockToolExecutor {
        MockToolExecutor {
            scripted: std::sync::Mutex::new(results.into_iter().collect()),
            seen: std::sync::Mutex::new(Vec::new()),
        }
    }

    pub fn calls(&self) -> Vec<ToolCall> {
        self.seen.lock().unwrap().clone()
    }

    pub fn call_count(&self) -> usize {
        self.seen.lock().unwrap().len()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Default for MockToolExecutor {
    fn default() -> MockToolExecutor {
        MockToolExecutor::new()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl ToolExecutor for MockToolExecutor {
    fn execute(
        &self,
        _context: &ToolExecutionContext<'_>,
        _def: &ToolDef,
        call: &ToolCall,
    ) -> Result<ToolResult, ToolExecError> {
        self.seen.lock().unwrap().push(call.clone());
        match self.scripted.lock().unwrap().pop_front() {
            Some(result) => result,
            None => Ok(ToolResult(format!("mock-exec:{}:ok", call.name.0))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_agent::{EffectKind, ToolCallId, ToolName, ToolSurface};

    fn def(name: &str) -> ToolDef {
        ToolDef {
            name: ToolName(name.into()),
            subsystem: "test".into(),
            version: 1,
            input_schema: "{}".into(),
            required_caps: vec![],
            effect_kind: EffectKind::Read,
            side_effecting: false,
            requires_approval: false,
            exposed_over_mcp: false,
        }
    }

    fn call(name: &str) -> ToolCall {
        ToolCall {
            id: ToolCallId(format!("call:{name}")),
            name: ToolName(name.into()),
            arguments: serde_json::Value::Null,
        }
    }

    fn context() -> ToolExecutionContext<'static> {
        let token = Box::leak(Box::new(RunTokenHandle {
            token: "secret-test-token".into(),
            jti: "test-jti".into(),
            ttl_secs: 60,
        }));
        ToolExecutionContext {
            run_id: "test-run",
            run_token: token,
        }
    }

    #[test]
    fn mock_tool_surface_resolves_registered_tools() {
        let mut cat = MockToolSurface::new();
        assert!(cat.resolve(&ToolName("search".into())).is_none());
        cat.register_tool(def("search"));
        assert!(cat.resolve(&ToolName("search".into())).is_some());
        assert!(cat.resolve(&ToolName("nope".into())).is_none());

        let seeded = MockToolSurface::with([def("a"), def("b")]);
        assert!(seeded.resolve(&ToolName("a".into())).is_some());
        assert!(seeded.resolve(&ToolName("b".into())).is_some());
    }

    #[test]
    fn mock_tool_executor_records_calls_and_replays_results() {
        let exec = MockToolExecutor::with_results([
            Ok(ToolResult("first".into())),
            Err(ToolExecError::Failed("boom".into())),
        ]);
        let d = def("t");

        let context = context();
        assert_eq!(
            exec.execute(&context, &d, &call("t")),
            Ok(ToolResult("first".into()))
        );
        assert_eq!(
            exec.execute(&context, &d, &call("t")),
            Err(ToolExecError::Failed("boom".into()))
        );
        assert_eq!(
            exec.execute(&context, &d, &call("search")),
            Ok(ToolResult("mock-exec:search:ok".into()))
        );

        assert_eq!(exec.call_count(), 3);
        let seen = exec.calls();
        assert_eq!(seen[0].name, ToolName("t".into()));
        assert_eq!(seen[2].name, ToolName("search".into()));
    }

    #[test]
    fn tool_exec_error_display_is_loud() {
        let e = ToolExecError::Failed("subsystem down".into()).to_string();
        assert!(e.contains("tool execution failed"), "loud reason: {e}");
        assert!(e.contains("subsystem down"));
    }
}
