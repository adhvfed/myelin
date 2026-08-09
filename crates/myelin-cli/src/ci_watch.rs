use crate::client::{execute, interpret, open_event_stream};
use crate::config::EdgeConfig;
use crate::dispatch::{EdgeCall, HttpMethod, RetryPolicy};
use crate::error::CliError;
use crate::render::terminal_safe_log_bytes;
use base64::Engine as _;
use http_body_util::{BodyExt, Limited};
use serde_json::Value;
use std::io::Write;
use std::time::Duration;

const ARCHIVE_RANGE_LIMIT: i64 = 256 * 1024;
const ARCHIVE_PROBE_START: i64 = 9_007_199_254_740_991;
const MAX_SSE_FRAME_BYTES: usize = 16 * 1024;
const MAX_RECONNECTS: u32 = 8;
const ERROR_BODY_LIMIT: usize = 64 * 1024;
const ERROR_BODY_DEADLINE: Duration = Duration::from_secs(30);

#[derive(Debug, PartialEq, Eq)]
enum LiveEvent {
    Ready {
        id: u64,
        byte_end: i64,
    },
    Appended {
        id: u64,
        byte_start: i64,
        byte_end: i64,
    },
    Complete {
        id: Option<u64>,
        byte_end: i64,
    },
}

#[derive(Debug)]
struct WatchState {
    run_id: String,
    job_id: String,
    event_id: Option<u64>,
    byte_end: i64,
}

enum StreamEnd {
    Complete,
    Disconnected { progressed: bool },
}

pub async fn execute_ci_watch(
    config: &EdgeConfig,
    token: &str,
    call: &EdgeCall,
    json: bool,
    output: &mut dyn Write,
) -> Result<(), CliError> {
    let (run_id, job_id) = watch_coordinates(call)?;
    let mut state = WatchState {
        run_id: run_id.to_string(),
        job_id: job_id.to_string(),
        event_id: None,
        byte_end: 0,
    };

    let initial_end = probe_archive_end(config, token, &state).await?;
    read_archive_through(config, token, &mut state, initial_end, json, output).await?;

    let mut reconnects = 0_u32;
    loop {
        let response = match open_event_stream(config, token, call, state.event_id).await {
            Ok(response) => response,
            Err(error) if reconnects < MAX_RECONNECTS => {
                reconnects += 1;
                reconnect_delay(reconnects).await;
                let _ = error;
                continue;
            }
            Err(error) => return Err(error),
        };
        let status = response.status().as_u16();
        if status == 409 {
            if state.event_id.is_none() {
                return Err(response_error(response).await);
            }
            drop(response);
            let latest_end = probe_archive_end(config, token, &state).await?;
            read_archive_through(config, token, &mut state, latest_end, json, output).await?;
            state.event_id = None;
            reconnects = 0;
            continue;
        }
        if (200..300).contains(&status) && status != 200 {
            return Err(malformed_stream(format!(
                "edge live-log returned unexpected success status {status}"
            )));
        }
        if status != 200 {
            return Err(response_error(response).await);
        }
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .map(str::trim);
        if content_type != Some("text/event-stream") {
            return Err(malformed_stream(
                "edge live-log success is not text/event-stream",
            ));
        }

        match consume_stream(
            response.into_body(),
            config,
            token,
            &mut state,
            json,
            output,
        )
        .await?
        {
            StreamEnd::Complete => return Ok(()),
            StreamEnd::Disconnected { progressed } => {
                if progressed {
                    reconnects = 0;
                }
                if reconnects >= MAX_RECONNECTS {
                    return Err(CliError::Transport(
                        "CI live-log stream disconnected repeatedly without completing".into(),
                    ));
                }
                reconnects += 1;
                reconnect_delay(reconnects).await;
            }
        }
    }
}

async fn consume_stream(
    mut body: hyper::body::Incoming,
    config: &EdgeConfig,
    token: &str,
    state: &mut WatchState,
    json: bool,
    output: &mut dyn Write,
) -> Result<StreamEnd, CliError> {
    let mut pending = Vec::new();
    let mut progressed = false;
    while let Some(frame) = body.frame().await {
        let frame = match frame {
            Ok(frame) => frame,
            Err(_) => return Ok(StreamEnd::Disconnected { progressed }),
        };
        let Ok(data) = frame.into_data() else {
            continue;
        };
        pending.extend_from_slice(&data);
        loop {
            let Some((frame_end, delimiter_len)) = event_boundary(&pending) else {
                enforce_partial_frame_bound(&pending)?;
                break;
            };
            if frame_end > MAX_SSE_FRAME_BYTES {
                return Err(malformed_stream("SSE frame exceeds 16 KiB"));
            }
            let event = parse_event(&pending[..frame_end], &state.run_id, &state.job_id)?;
            pending.drain(..frame_end + delimiter_len);
            if let Some(event) = event {
                if apply_event(config, token, state, event, json, output).await? {
                    return Ok(StreamEnd::Complete);
                }
                progressed = true;
            }
        }
    }
    if !pending.is_empty() {
        return Err(malformed_stream(
            "SSE response ended with a truncated frame",
        ));
    }
    Ok(StreamEnd::Disconnected { progressed })
}

async fn apply_event(
    config: &EdgeConfig,
    token: &str,
    state: &mut WatchState,
    event: LiveEvent,
    json: bool,
    output: &mut dyn Write,
) -> Result<bool, CliError> {
    match event {
        LiveEvent::Ready { id, byte_end } => {
            if state.event_id.is_some() || byte_end < state.byte_end {
                return Err(malformed_stream(
                    "ready checkpoint regresses or follows an acknowledged cursor",
                ));
            }
            read_archive_through(config, token, state, byte_end, json, output).await?;
            state.event_id = Some(id);
            Ok(false)
        }
        LiveEvent::Appended {
            id,
            byte_start,
            byte_end,
        } => {
            let expected_id = state
                .event_id
                .and_then(|cursor| cursor.checked_add(1))
                .ok_or_else(|| malformed_stream("append arrived before a resume checkpoint"))?;
            if id != expected_id || byte_start != state.byte_end || byte_end <= byte_start {
                return Err(malformed_stream(
                    "append cursor or byte coordinates are discontinuous",
                ));
            }
            read_archive_through(config, token, state, byte_end, json, output).await?;
            state.event_id = Some(id);
            Ok(false)
        }
        LiveEvent::Complete { id, byte_end } => {
            if byte_end < state.byte_end
                || match (state.event_id, id) {
                    (Some(current), Some(event)) => current != event,
                    (Some(0), None) => byte_end != 0,
                    (None, None) => byte_end != 0,
                    _ => true,
                }
            {
                return Err(malformed_stream(
                    "completion cursor or byte coordinate contradicts acknowledged output",
                ));
            }
            read_archive_through(config, token, state, byte_end, json, output).await?;
            Ok(true)
        }
    }
}

async fn probe_archive_end(
    config: &EdgeConfig,
    token: &str,
    state: &WatchState,
) -> Result<i64, CliError> {
    let value = read_archive_range(config, token, state, ARCHIVE_PROBE_START, 1).await?;
    value["total_end"]
        .as_i64()
        .filter(|end| *end >= 0)
        .ok_or_else(|| malformed_stream("archive probe has no non-negative total"))
}

async fn read_archive_through(
    config: &EdgeConfig,
    token: &str,
    state: &mut WatchState,
    target_end: i64,
    json: bool,
    output: &mut dyn Write,
) -> Result<(), CliError> {
    if target_end < state.byte_end {
        return Err(malformed_stream("archive target regresses"));
    }
    while state.byte_end < target_end {
        let limit = (target_end - state.byte_end).min(ARCHIVE_RANGE_LIMIT);
        let value = read_archive_range(config, token, state, state.byte_end, limit).await?;
        let start = value["byte_start"]
            .as_i64()
            .ok_or_else(|| malformed_stream("archive range start is missing"))?;
        let end = value["byte_end"]
            .as_i64()
            .ok_or_else(|| malformed_stream("archive range end is missing"))?;
        if start != state.byte_end || end <= start || end > target_end {
            return Err(malformed_stream(
                "archive range does not advance exactly toward the live pointer",
            ));
        }
        if json {
            serde_json::to_writer(&mut *output, &value)
                .map_err(|error| CliError::Transport(format!("write CI JSON output: {error}")))?;
            output
                .write_all(b"\n")
                .map_err(|error| CliError::Transport(format!("write CI JSON output: {error}")))?;
        } else {
            let encoded = value["data"]
                .as_str()
                .ok_or_else(|| malformed_stream("archive range data is missing"))?;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .map_err(|_| malformed_stream("archive range data is not base64"))?;
            output
                .write_all(terminal_safe_log_bytes(&bytes).as_bytes())
                .map_err(|error| CliError::Transport(format!("write CI log output: {error}")))?;
        }
        output
            .flush()
            .map_err(|error| CliError::Transport(format!("flush CI log output: {error}")))?;
        state.byte_end = end;
    }
    Ok(())
}

async fn read_archive_range(
    config: &EdgeConfig,
    token: &str,
    state: &WatchState,
    start: i64,
    limit: i64,
) -> Result<Value, CliError> {
    let call = EdgeCall {
        method: HttpMethod::Get,
        path: format!("/v1/ci/runs/{}/jobs/{}/log", state.run_id, state.job_id),
        query: Some(format!("start={start}&limit={limit}")),
        payload: None,
        idempotency_key: None,
        retry_policy: RetryPolicy::None,
    };
    execute(config, token, &call).await
}

async fn response_error(response: hyper::Response<hyper::body::Incoming>) -> CliError {
    let status = response.status().as_u16();
    let body = tokio::time::timeout(
        ERROR_BODY_DEADLINE,
        Limited::new(response.into_body(), ERROR_BODY_LIMIT).collect(),
    )
    .await
    .ok()
    .and_then(Result::ok)
    .map(|collected| collected.to_bytes())
    .unwrap_or_default();
    let value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    match interpret(status, value) {
        Err(error) => error,
        Ok(_) => malformed_stream(format!(
            "edge live-log returned unexpected success status {status}"
        )),
    }
}

fn watch_coordinates(call: &EdgeCall) -> Result<(&str, &str), CliError> {
    if call.method != HttpMethod::Get || call.query.is_some() || call.payload.is_some() {
        return Err(malformed_stream("watch call is not a query-free GET"));
    }
    let parts = call.path.split('/').collect::<Vec<_>>();
    match parts.as_slice() {
        ["", "v1", "ci", "runs", run, "jobs", job, "log", "live"]
            if canonical_uuid(run) && canonical_uuid(job) =>
        {
            Ok((run, job))
        }
        _ => Err(malformed_stream("watch route is not canonical")),
    }
}

fn event_boundary(bytes: &[u8]) -> Option<(usize, usize)> {
    let lf = bytes.windows(2).position(|window| window == b"\n\n");
    let crlf = bytes.windows(4).position(|window| window == b"\r\n\r\n");
    match (lf, crlf) {
        (Some(left), Some(right)) if left <= right => Some((left, 2)),
        (Some(_), Some(right)) => Some((right, 4)),
        (Some(left), None) => Some((left, 2)),
        (None, Some(right)) => Some((right, 4)),
        (None, None) => None,
    }
}

fn enforce_partial_frame_bound(bytes: &[u8]) -> Result<(), CliError> {
    if bytes.len() > MAX_SSE_FRAME_BYTES {
        return Err(malformed_stream("SSE frame exceeds 16 KiB"));
    }
    Ok(())
}

fn parse_event(bytes: &[u8], run_id: &str, job_id: &str) -> Result<Option<LiveEvent>, CliError> {
    let frame =
        std::str::from_utf8(bytes).map_err(|_| malformed_stream("SSE frame is not UTF-8"))?;
    let mut event = None;
    let mut id = None;
    let mut data = None;
    for raw_line in frame.split('\n') {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if line.starts_with(':') {
            continue;
        } else if let Some(value) = line.strip_prefix("event: ") {
            if event.replace(value).is_some() {
                return Err(malformed_stream("SSE frame repeats event"));
            }
        } else if let Some(value) = line.strip_prefix("id: ") {
            if id.replace(value).is_some() {
                return Err(malformed_stream("SSE frame repeats id"));
            }
        } else if let Some(value) = line.strip_prefix("data: ") {
            if data.replace(value).is_some() {
                return Err(malformed_stream("SSE frame repeats data"));
            }
        } else {
            return Err(malformed_stream("SSE frame contains an unknown field"));
        }
    }
    if event.is_none() && id.is_none() && data.is_none() {
        return Ok(None);
    }
    let event = event.ok_or_else(|| malformed_stream("SSE frame has no event"))?;
    let id = id.map(canonical_u64).transpose()?;
    let data: Value =
        serde_json::from_str(data.ok_or_else(|| malformed_stream("SSE frame has no data"))?)
            .map_err(|_| malformed_stream("SSE data is not JSON"))?;
    match event {
        "ci.log.ready" => {
            let object = exact_data(&data, &["run_id", "job_id", "byte_end"])?;
            exact_scope(object, run_id, job_id)?;
            Ok(Some(LiveEvent::Ready {
                id: id.ok_or_else(|| malformed_stream("ready event has no cursor"))?,
                byte_end: non_negative_i64(&object["byte_end"], "ready byte_end")?,
            }))
        }
        "ci.log.appended" => {
            let object = exact_data(&data, &["run_id", "job_id", "byte_start", "byte_end"])?;
            exact_scope(object, run_id, job_id)?;
            Ok(Some(LiveEvent::Appended {
                id: id.ok_or_else(|| malformed_stream("append event has no cursor"))?,
                byte_start: non_negative_i64(&object["byte_start"], "append byte_start")?,
                byte_end: non_negative_i64(&object["byte_end"], "append byte_end")?,
            }))
        }
        "ci.log.complete" => {
            let object = exact_data(&data, &["run_id", "job_id", "byte_end"])?;
            exact_scope(object, run_id, job_id)?;
            Ok(Some(LiveEvent::Complete {
                id,
                byte_end: non_negative_i64(&object["byte_end"], "complete byte_end")?,
            }))
        }
        _ => Err(malformed_stream(
            "SSE event type is not a CI live-log event",
        )),
    }
}

fn exact_data<'a>(
    value: &'a Value,
    fields: &[&str],
) -> Result<&'a serde_json::Map<String, Value>, CliError> {
    let object = value
        .as_object()
        .ok_or_else(|| malformed_stream("SSE data is not an object"))?;
    if object.len() != fields.len() || !fields.iter().all(|field| object.contains_key(*field)) {
        return Err(malformed_stream("SSE data has missing or unknown fields"));
    }
    Ok(object)
}

fn exact_scope(
    object: &serde_json::Map<String, Value>,
    run_id: &str,
    job_id: &str,
) -> Result<(), CliError> {
    if object["run_id"].as_str() != Some(run_id) || object["job_id"].as_str() != Some(job_id) {
        return Err(malformed_stream("SSE event crosses the requested scope"));
    }
    Ok(())
}

fn non_negative_i64(value: &Value, field: &str) -> Result<i64, CliError> {
    value
        .as_i64()
        .filter(|value| *value >= 0)
        .ok_or_else(|| malformed_stream(format!("{field} is not a non-negative integer")))
}

fn canonical_u64(value: &str) -> Result<u64, CliError> {
    let parsed = value
        .parse::<u64>()
        .map_err(|_| malformed_stream("SSE cursor is not a canonical u64"))?;
    if parsed.to_string() != value {
        return Err(malformed_stream("SSE cursor is not a canonical u64"));
    }
    Ok(parsed)
}

fn canonical_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte),
        })
}

fn malformed_stream(reason: impl Into<String>) -> CliError {
    CliError::Transport(format!(
        "edge returned a malformed CI live-log stream: {}",
        reason.into()
    ))
}

async fn reconnect_delay(attempt: u32) {
    let shift = attempt.saturating_sub(1).min(4);
    tokio::time::sleep(Duration::from_millis(125_u64 << shift)).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const RUN: &str = "91000000-0000-4000-8000-000000000002";
    const JOB: &str = "92000000-0000-4000-8000-000000000002";

    fn wire(event: &Value) -> Vec<u8> {
        let mut output = format!("event: {}\n", event["event"].as_str().unwrap());
        if let Some(id) = event["id"].as_str() {
            output.push_str(&format!("id: {id}\n"));
        }
        output.push_str(&format!("data: {}", event["data"]));
        output.into_bytes()
    }

    #[test]
    fn parses_every_shared_live_golden_event() {
        let contract: Value = serde_json::from_str(include_str!(
            "../../../contracts/ci-read-dev-edge.golden.json"
        ))
        .unwrap();
        let vectors = contract["vectors"].as_array().unwrap();
        let mut parsed = 0;
        for vector in vectors.iter().filter(|vector| vector["endpoint"] == "live") {
            let Some(events) = vector["expected"]["events"].as_array() else {
                continue;
            };
            for event in events {
                let run = event["data"]["run_id"].as_str().unwrap();
                let job = event["data"]["job_id"].as_str().unwrap();
                parse_event(&wire(event), run, job).unwrap();
                parsed += 1;
            }
        }
        assert_eq!(parsed, 4, "all shared live event shapes stay executable");
        assert_eq!(
            parse_event(b": connected", RUN, JOB).unwrap(),
            None,
            "legal SSE keepalive/connected comments carry no CI event"
        );
    }

    #[test]
    fn refuses_scope_drift_unknown_fields_noncanonical_cursors_and_oversize() {
        let base = json!({
            "event": "ci.log.appended",
            "id": "2",
            "data": {
                "run_id": RUN,
                "job_id": JOB,
                "byte_start": 5,
                "byte_end": 11
            }
        });
        parse_event(&wire(&base), RUN, JOB).unwrap();

        let mut drift = base.clone();
        drift["data"]["job_id"] = Value::String("92000000-0000-4000-8000-000000000003".into());
        assert!(parse_event(&wire(&drift), RUN, JOB).is_err());

        let mut unknown = base.clone();
        unknown["data"]["bytes"] = Value::String("payload-must-not-ride-SSE".into());
        assert!(parse_event(&wire(&unknown), RUN, JOB).is_err());

        let mut cursor = base;
        cursor["id"] = Value::String("02".into());
        assert!(parse_event(&wire(&cursor), RUN, JOB).is_err());

        let oversized = vec![b'x'; MAX_SSE_FRAME_BYTES + 1];
        assert!(event_boundary(&oversized).is_none());
        assert!(enforce_partial_frame_bound(&oversized).is_err());
    }

    #[tokio::test]
    async fn empty_terminal_stream_accepts_the_ready_zero_then_idless_complete_contract() {
        let config = EdgeConfig {
            url: "http://127.0.0.1:1".into(),
            scheme: "agent".into(),
        };
        let mut state = WatchState {
            run_id: RUN.into(),
            job_id: JOB.into(),
            event_id: None,
            byte_end: 0,
        };
        let mut output = Vec::new();
        let ready = parse_event(
            format!(
                "event: ci.log.ready\nid: 0\ndata: {{\"run_id\":\"{RUN}\",\"job_id\":\"{JOB}\",\"byte_end\":0}}"
            )
            .as_bytes(),
            RUN,
            JOB,
        )
        .unwrap()
        .unwrap();
        assert!(
            !apply_event(&config, "unused", &mut state, ready, false, &mut output,)
                .await
                .unwrap()
        );
        assert_eq!(state.event_id, Some(0));

        let complete = parse_event(
            format!(
                "event: ci.log.complete\ndata: {{\"run_id\":\"{RUN}\",\"job_id\":\"{JOB}\",\"byte_end\":0}}"
            )
            .as_bytes(),
            RUN,
            JOB,
        )
        .unwrap()
        .unwrap();
        assert!(
            apply_event(&config, "unused", &mut state, complete, false, &mut output,)
                .await
                .unwrap()
        );
        assert!(output.is_empty());
    }
}
