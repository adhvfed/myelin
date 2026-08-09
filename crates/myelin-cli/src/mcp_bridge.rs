use crate::client::EdgeHttpClient;
use crate::config::EdgeConfig;
use crate::dispatch::{is_canonical_agent_id, EdgeCall, HttpMethod, RetryPolicy};
use crate::error::CliError;
use chrono::{DateTime, Utc};
use rand::{rngs::OsRng, RngCore as _};
use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncBufReadExt as _, AsyncWrite, AsyncWriteExt as _};
use zeroize::Zeroizing;

const AGENT_SCHEME: &str = "agent";
const MAX_MCP_FRAME_BYTES: usize = 1024 * 1024;
const MAX_RUN_TOKEN_BYTES: usize = 32 * 1024;

/**
 * Run a newline-delimited MCP session without asking the external client to own a Myelin token.
 *
 * The caller's browser session is used exactly once to create an attenuated, one-minute run. The
 * returned credential remains in memory, authorizes every MCP frame, and closes itself when stdin
 * ends or the process is interrupted.
 */
pub async fn serve_stdio(
    edge: &EdgeConfig,
    browser_session: &str,
    agent_id: &str,
) -> Result<(), CliError> {
    require_agent_id(agent_id)?;
    let client = EdgeHttpClient::new()?;
    let mut run = AgentRun::start(&client, edge, browser_session, agent_id).await?;
    let result = proxy_frames(&client, edge, &run, tokio::io::stdin(), tokio::io::stdout()).await;
    let cleanup = run.close(&client, edge).await;

    match (result, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(primary), _) => Err(primary),
        (Ok(()), Err(cleanup)) => Err(cleanup),
    }
}

struct AgentRun {
    id: String,
    agent_id: String,
    credential: Zeroizing<String>,
    closed: bool,
}

impl AgentRun {
    async fn start(
        client: &EdgeHttpClient,
        edge: &EdgeConfig,
        browser_session: &str,
        agent_id: &str,
    ) -> Result<Self, CliError> {
        let call = EdgeCall {
            method: HttpMethod::Post,
            path: format!("/v1/agents/{agent_id}/runs"),
            query: None,
            payload: Some(b"{}".to_vec()),
            idempotency_key: Some(new_idempotency_key("mcp-run")),
            retry_policy: RetryPolicy::CallerKeyRequired,
        };
        let response = execute_retry_safe(client, edge, browser_session, &call).await?;
        validate_started_run(response, agent_id)
    }

    async fn send(
        &self,
        client: &EdgeHttpClient,
        edge: &EdgeConfig,
        frame: Vec<u8>,
    ) -> Result<Option<Value>, CliError> {
        let call = EdgeCall {
            method: HttpMethod::Post,
            path: format!("/v1/agent-runs/{}/mcp", self.id),
            query: None,
            payload: Some(frame),
            idempotency_key: None,
            retry_policy: RetryPolicy::None,
        };
        let run_edge = EdgeConfig {
            url: edge.url.clone(),
            scheme: AGENT_SCHEME.into(),
        };
        let response = client.execute(&run_edge, &self.credential, &call).await?;
        Ok((response != Value::Null).then_some(response))
    }

    async fn close(&mut self, client: &EdgeHttpClient, edge: &EdgeConfig) -> Result<(), CliError> {
        if self.closed {
            return Ok(());
        }
        let call = EdgeCall {
            method: HttpMethod::Post,
            path: format!("/v1/agent-runs/{}/close", self.id),
            query: None,
            payload: Some(b"{}".to_vec()),
            idempotency_key: Some(new_idempotency_key("mcp-close")),
            retry_policy: RetryPolicy::CallerKeyRequired,
        };
        let run_edge = EdgeConfig {
            url: edge.url.clone(),
            scheme: AGENT_SCHEME.into(),
        };
        let response = execute_retry_safe(client, &run_edge, &self.credential, &call).await?;
        validate_closed_run(&response, &self.id, &self.agent_id)?;
        self.closed = true;
        Ok(())
    }
}

async fn proxy_frames<R, W>(
    client: &EdgeHttpClient,
    edge: &EdgeConfig,
    run: &AgentRun,
    input: R,
    mut output: W,
) -> Result<(), CliError>
where
    R: tokio::io::AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut input = tokio::io::BufReader::new(input);
    let interruption = process_interruption();
    tokio::pin!(interruption);
    loop {
        let frame = tokio::select! {
            result = read_frame(&mut input) => result?,
            result = &mut interruption => {
                result?;
                return Ok(());
            }
        };
        let Some(frame) = frame else {
            return Ok(());
        };
        if frame.is_empty() {
            continue;
        }
        std::str::from_utf8(&frame)
            .map_err(|_| CliError::Usage("MCP input frames must be UTF-8 encoded JSON".into()))?;
        let Some(response) = run.send(client, edge, frame).await? else {
            continue;
        };
        let encoded = serde_json::to_vec(&response).map_err(|error| {
            CliError::Transport(format!("could not encode MCP response: {error}"))
        })?;
        if encoded.len() > MAX_MCP_FRAME_BYTES {
            return Err(CliError::Transport(format!(
                "MCP response exceeded the {MAX_MCP_FRAME_BYTES}-byte frame limit"
            )));
        }
        output.write_all(&encoded).await.map_err(|error| {
            CliError::Transport(format!("could not write MCP response: {error}"))
        })?;
        output.write_all(b"\n").await.map_err(|error| {
            CliError::Transport(format!("could not write MCP response: {error}"))
        })?;
        output.flush().await.map_err(|error| {
            CliError::Transport(format!("could not flush MCP response: {error}"))
        })?;
    }
}

async fn process_interruption() -> Result<(), CliError> {
    #[cfg(unix)]
    {
        let mut terminate = tokio::signal::unix::signal(
            tokio::signal::unix::SignalKind::terminate(),
        )
        .map_err(|error| {
            CliError::Transport(format!("could not listen for process termination: {error}"))
        })?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result.map_err(interruption_error),
            signal = terminate.recv() => signal.ok_or_else(|| {
                CliError::Transport("process termination listener closed unexpectedly".into())
            }),
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await.map_err(interruption_error)
}

fn interruption_error(error: std::io::Error) -> CliError {
    CliError::Transport(format!(
        "could not listen for process interruption: {error}"
    ))
}

async fn read_frame<R>(input: &mut R) -> Result<Option<Vec<u8>>, CliError>
where
    R: AsyncBufRead + Unpin,
{
    let mut frame = Vec::new();
    loop {
        let available = input
            .fill_buf()
            .await
            .map_err(|error| CliError::Transport(format!("could not read MCP request: {error}")))?;
        if available.is_empty() {
            return Ok((!frame.is_empty()).then_some(frame));
        }

        let newline = available.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(available.len(), |index| index + 1);
        let payload_end = newline.unwrap_or(available.len());
        if frame.len().saturating_add(payload_end) > MAX_MCP_FRAME_BYTES {
            return Err(CliError::Usage(format!(
                "MCP input frame exceeds the {MAX_MCP_FRAME_BYTES}-byte limit"
            )));
        }
        frame.extend_from_slice(&available[..payload_end]);
        input.consume(consumed);

        if newline.is_some() {
            if frame.last() == Some(&b'\r') {
                frame.pop();
            }
            return Ok(Some(frame));
        }
    }
}

#[derive(Deserialize)]
struct StartedRunResponse {
    run: StartedRun,
    credential: RunCredential,
    durable: bool,
}

#[derive(Deserialize)]
struct StartedRun {
    id: String,
    agent_id: String,
    principal_id: String,
    state: String,
    issued_at: String,
    expires_at: String,
}

#[derive(Deserialize)]
struct RunCredential {
    scheme: String,
    token: String,
    expires_at: String,
}

fn validate_started_run(response: Value, expected_agent_id: &str) -> Result<AgentRun, CliError> {
    let started: StartedRunResponse = serde_json::from_value(response).map_err(|error| {
        malformed_run_response(format!("start response has an invalid shape: {error}"))
    })?;
    if !started.durable {
        return Err(malformed_run_response("run is not durable"));
    }
    require_server_uuid("run id", &started.run.id)?;
    if started.run.agent_id != expected_agent_id
        || started.run.principal_id != format!("agent:{expected_agent_id}")
    {
        return Err(malformed_run_response(
            "run identity does not match the requested agent",
        ));
    }
    if started.run.state != "ready" {
        return Err(malformed_run_response("run is not ready"));
    }
    if started.credential.scheme != AGENT_SCHEME {
        return Err(malformed_run_response(
            "run credential does not use the agent scheme",
        ));
    }
    if started.credential.token.is_empty()
        || started.credential.token.len() > MAX_RUN_TOKEN_BYTES
        || !started
            .credential
            .token
            .bytes()
            .all(|byte| byte.is_ascii_graphic())
    {
        return Err(malformed_run_response("run credential is malformed"));
    }
    if started.credential.expires_at != started.run.expires_at {
        return Err(malformed_run_response(
            "run and credential expiries do not match",
        ));
    }
    let issued_at = parse_run_time("issued_at", &started.run.issued_at)?;
    let expires_at = parse_run_time("expires_at", &started.run.expires_at)?;
    if expires_at <= issued_at || expires_at <= Utc::now() {
        return Err(malformed_run_response(
            "run credential is already expired or has an invalid lifetime",
        ));
    }

    Ok(AgentRun {
        id: started.run.id,
        agent_id: started.run.agent_id,
        credential: Zeroizing::new(started.credential.token),
        closed: false,
    })
}

fn validate_closed_run(response: &Value, run_id: &str, agent_id: &str) -> Result<(), CliError> {
    if response.pointer("/run/id").and_then(Value::as_str) != Some(run_id)
        || response.pointer("/run/agent_id").and_then(Value::as_str) != Some(agent_id)
        || response.pointer("/run/state").and_then(Value::as_str) != Some("closed")
        || response.get("closed").and_then(Value::as_bool) != Some(true)
        || response.get("durable").and_then(Value::as_bool) != Some(true)
    {
        return Err(malformed_run_response(
            "close response does not describe the active run",
        ));
    }
    Ok(())
}

fn parse_run_time(field: &str, value: &str) -> Result<DateTime<Utc>, CliError> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| malformed_run_response(format!("run {field} is not an RFC 3339 timestamp")))
}

fn require_agent_id(agent_id: &str) -> Result<(), CliError> {
    if is_canonical_agent_id(agent_id) {
        Ok(())
    } else {
        Err(CliError::Usage(
            "--as must be a canonical lowercase agent UUID".into(),
        ))
    }
}

fn require_server_uuid(label: &str, value: &str) -> Result<(), CliError> {
    if is_canonical_agent_id(value) {
        Ok(())
    } else {
        Err(malformed_run_response(format!(
            "{label} is not a canonical lowercase UUID"
        )))
    }
}

fn malformed_run_response(reason: impl Into<String>) -> CliError {
    CliError::Transport(format!(
        "edge returned a malformed agent-run response: {}",
        reason.into()
    ))
}

fn new_idempotency_key(prefix: &str) -> String {
    let mut random = [0_u8; 16];
    OsRng.fill_bytes(&mut random);
    let suffix = random
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("{prefix}-{suffix}")
}

async fn execute_retry_safe(
    client: &EdgeHttpClient,
    edge: &EdgeConfig,
    token: &str,
    call: &EdgeCall,
) -> Result<Value, CliError> {
    match client.execute(edge, token, call).await {
        Err(error) if error.is_retryable_response_loss() => client.execute(edge, token, call).await,
        result => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const AGENT: &str = "11111111-1111-1111-1111-111111111111";
    const RUN: &str = "22222222-2222-2222-2222-222222222222";

    fn started_response() -> Value {
        let issued = Utc::now() - chrono::Duration::seconds(1);
        let expires = issued + chrono::Duration::minutes(1);
        json!({
            "run": {
                "id": RUN,
                "agent_id": AGENT,
                "principal_id": format!("agent:{AGENT}"),
                "state": "ready",
                "issued_at": issued.to_rfc3339(),
                "expires_at": expires.to_rfc3339(),
                "selected_tools": [],
            },
            "credential": {
                "scheme": "agent",
                "token": "v4.public.transient",
                "expires_at": expires.to_rfc3339(),
            },
            "created": true,
            "durable": true,
        })
    }

    #[test]
    fn a_started_run_is_bound_to_the_requested_durable_agent() {
        let run = validate_started_run(started_response(), AGENT).unwrap();
        assert_eq!(run.id, RUN);
        assert_eq!(run.agent_id, AGENT);

        let mut wrong_agent = started_response();
        wrong_agent["run"]["agent_id"] = json!(RUN);
        assert!(validate_started_run(wrong_agent, AGENT).is_err());

        let mut long_lived_scheme = started_response();
        long_lived_scheme["credential"]["scheme"] = json!("session");
        assert!(validate_started_run(long_lived_scheme, AGENT).is_err());
    }

    #[test]
    fn a_close_must_confirm_the_exact_run() {
        let closed = json!({
            "run": { "id": RUN, "agent_id": AGENT, "state": "closed" },
            "closed": true,
            "durable": true,
        });
        validate_closed_run(&closed, RUN, AGENT).unwrap();
        assert!(validate_closed_run(&closed, AGENT, RUN).is_err());
    }

    #[tokio::test]
    async fn newline_frames_are_bounded_and_windows_friendly() {
        let mut input = tokio::io::BufReader::new(&b"{\"id\":1}\r\n{\"id\":2}"[..]);
        assert_eq!(
            read_frame(&mut input).await.unwrap(),
            Some(b"{\"id\":1}".to_vec())
        );
        assert_eq!(
            read_frame(&mut input).await.unwrap(),
            Some(b"{\"id\":2}".to_vec())
        );
        assert_eq!(read_frame(&mut input).await.unwrap(), None);

        let too_large = vec![b'x'; MAX_MCP_FRAME_BYTES + 1];
        let mut input = tokio::io::BufReader::new(too_large.as_slice());
        assert!(read_frame(&mut input).await.is_err());
    }

    #[test]
    fn generated_retry_keys_are_bounded_and_distinct() {
        let first = new_idempotency_key("mcp-run");
        let second = new_idempotency_key("mcp-run");
        assert_ne!(first, second);
        assert!(first.len() <= 128);
        assert!(first.bytes().all(|byte| byte.is_ascii_graphic()));
    }
}
