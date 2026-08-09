use crate::config::EdgeConfig;
use crate::dispatch::{EdgeCall, RetryPolicy};
use crate::error::CliError;
use base64::Engine as _;
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{Request, Response, Uri};
use hyper_rustls::HttpsConnectorBuilder;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use serde_json::Value;
use std::time::Duration;

const REQUEST_DEADLINE: Duration = Duration::from_secs(30);
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MAX_AUTH_RESPONSE_BYTES: usize = 64 * 1024;

#[derive(Debug, PartialEq, Eq)]
struct Target {
    uri: Uri,
    authority: String,
}

fn target(config: &EdgeConfig, call: &EdgeCall) -> Result<Target, CliError> {
    let base = config.url.trim_end_matches('/');
    if !(base.starts_with("https://") || base.starts_with("http://")) {
        return Err(CliError::Transport(
            "edge URL must start with https:// (or loopback http:// for development)".into(),
        ));
    }
    if base.contains('#') {
        return Err(CliError::Transport(
            "edge URL must not contain a fragment".into(),
        ));
    }
    let base_uri: Uri = base
        .parse()
        .map_err(|e| CliError::Transport(format!("invalid edge URL: {e}")))?;
    let base_authority = base_uri
        .authority()
        .ok_or_else(|| CliError::Transport("edge URL has no host".into()))?;
    if base_authority.as_str().contains('@') {
        return Err(CliError::Transport(
            "edge URL must not contain user information".into(),
        ));
    }
    if base_uri
        .path_and_query()
        .is_some_and(|value| value.query().is_some())
    {
        return Err(CliError::Transport(
            "edge URL must not contain a query string".into(),
        ));
    }

    let mut absolute = format!("{base}{}", call.path);
    if let Some(q) = &call.query {
        absolute.push('?');
        absolute.push_str(q);
    }
    let uri: Uri = absolute
        .parse()
        .map_err(|e| CliError::Transport(format!("invalid edge URL: {e}")))?;
    let scheme = uri
        .scheme_str()
        .ok_or_else(|| CliError::Transport("edge URL has no scheme".into()))?;
    let authority = uri
        .authority()
        .ok_or_else(|| CliError::Transport("edge URL has no host".into()))?;
    let host = authority.host();
    if scheme == "http" && !is_loopback_host(host) {
        return Err(CliError::Transport(format!(
            "refusing clear-text bearer transport to non-loopback host `{host}`; use https://"
        )));
    }
    let authority = authority.as_str().to_string();
    Ok(Target { uri, authority })
}

fn is_loopback_host(host: &str) -> bool {
    let host = host
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(host);
    host.eq_ignore_ascii_case("localhost")
        || host == "::1"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

pub async fn execute(config: &EdgeConfig, token: &str, call: &EdgeCall) -> Result<Value, CliError> {
    execute_with_limits(config, token, call, REQUEST_DEADLINE, MAX_RESPONSE_BYTES).await
}

/**
 * Call one of the two deliberately unauthenticated device-login endpoints. Keeping this surface
 * exact prevents a future caller from accidentally treating an ordinary Edge mutation as public.
 */
pub async fn execute_device_auth(config: &EdgeConfig, call: &EdgeCall) -> Result<Value, CliError> {
    if call.method != crate::dispatch::HttpMethod::Post
        || call.query.is_some()
        || call.idempotency_key.is_some()
        || !matches!(
            call.path.as_str(),
            "/v1/auth/device/authorization" | "/v1/auth/device/token"
        )
    {
        return Err(CliError::Transport(
            "device authorization attempted an unexpected Edge request".into(),
        ));
    }
    send_json(
        config,
        None,
        call,
        REQUEST_DEADLINE,
        MAX_AUTH_RESPONSE_BYTES,
    )
    .await
}

pub async fn open_event_stream(
    config: &EdgeConfig,
    token: &str,
    call: &EdgeCall,
    last_event_id: Option<u64>,
) -> Result<Response<hyper::body::Incoming>, CliError> {
    if call.method != crate::dispatch::HttpMethod::Get
        || call.query.is_some()
        || call.payload.is_some()
    {
        return Err(CliError::Transport(
            "event stream call must be a query-free GET".into(),
        ));
    }
    let target = target(config, call)?;
    let connector = HttpsConnectorBuilder::new()
        .with_provider_and_native_roots(rustls::crypto::aws_lc_rs::default_provider())
        .map_err(|e| CliError::Transport(format!("load native TLS trust roots: {e}")))?
        .https_or_http()
        .enable_http1()
        .build();
    let client: Client<_, Full<Bytes>> = Client::builder(TokioExecutor::new()).build(connector);
    let mut builder = Request::builder()
        .method("GET")
        .uri(&target.uri)
        .header("host", &target.authority)
        .header("accept", "text/event-stream")
        .header("cache-control", "no-cache")
        .header("authorization", format!("Bearer {token}"))
        .header("x-myelin-token-scheme", &config.scheme);
    if let Some(cursor) = last_event_id {
        builder = builder.header("last-event-id", cursor.to_string());
    }
    let request = builder
        .body(Full::new(Bytes::new()))
        .map_err(|e| CliError::Transport(format!("build event stream request: {e}")))?;
    tokio::time::timeout(REQUEST_DEADLINE, client.request(request))
        .await
        .map_err(|_| {
            CliError::Transport(format!(
                "edge event stream connect exceeded the {}-second deadline",
                REQUEST_DEADLINE.as_secs()
            ))
        })?
        .map_err(|e| CliError::Transport(format!("edge event stream request failed: {e}")))
}

async fn execute_with_limits(
    config: &EdgeConfig,
    token: &str,
    call: &EdgeCall,
    deadline: Duration,
    max_response_bytes: usize,
) -> Result<Value, CliError> {
    match (call.retry_policy, call.idempotency_key.as_deref()) {
        (RetryPolicy::CallerKeyRequired, None) => {
            return Err(CliError::Usage(
                "mutating commands require --idempotency-key <key>; reuse the same key when \
                 retrying after a lost response"
                    .into(),
            ));
        }
        (RetryPolicy::None, Some(_)) => {
            return Err(CliError::Usage(
                "--idempotency-key applies only to mutating commands".into(),
            ));
        }
        _ => {}
    }
    let body = send_json(config, Some(token), call, deadline, max_response_bytes).await?;
    validate_ci_success(call, &body)?;
    Ok(body)
}

async fn send_json(
    config: &EdgeConfig,
    token: Option<&str>,
    call: &EdgeCall,
    deadline: Duration,
    max_response_bytes: usize,
) -> Result<Value, CliError> {
    let target = target(config, call)?;
    let connector = HttpsConnectorBuilder::new()
        .with_provider_and_native_roots(rustls::crypto::aws_lc_rs::default_provider())
        .map_err(|e| CliError::Transport(format!("load native TLS trust roots: {e}")))?
        .https_or_http()
        .enable_http1()
        .build();
    let client: Client<_, Full<Bytes>> = Client::builder(TokioExecutor::new()).build(connector);

    let mut builder = Request::builder()
        .method(call.method.as_str())
        .uri(&target.uri)
        .header("host", &target.authority)
        .header("accept", "application/json");
    if let Some(token) = token {
        builder = builder
            .header("authorization", format!("Bearer {token}"))
            .header("x-myelin-token-scheme", &config.scheme);
    }
    let payload = call.payload.clone().unwrap_or_default();
    if call.payload.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    if let Some(key) = &call.idempotency_key {
        builder = builder.header("idempotency-key", key);
    }
    let request = builder
        .body(Full::new(Bytes::from(payload)))
        .map_err(|e| CliError::Transport(format!("build request: {e}")))?;

    let (status, bytes) = tokio::time::timeout(deadline, async {
        let response = client
            .request(request)
            .await
            .map_err(|e| CliError::Transport(format!("edge request failed: {e}")))?;
        let status = response.status().as_u16();
        let bytes = Limited::new(response.into_body(), max_response_bytes)
            .collect()
            .await
            .map_err(|_| {
                CliError::Transport(format!(
                    "edge response exceeded the {max_response_bytes}-byte limit or could not be read"
                ))
            })?
            .to_bytes();
        Ok::<_, CliError>((status, bytes))
    })
    .await
    .map_err(|_| {
        CliError::Transport(format!(
            "edge request exceeded the {}-second deadline",
            deadline.as_secs()
        ))
    })??;
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);

    interpret(status, body)
}

fn validate_ci_success(call: &EdgeCall, body: &Value) -> Result<(), CliError> {
    if call.path == "/v1/ci/runs" {
        return validate_ci_run_list(call, body);
    }
    let Some(tail) = call.path.strip_prefix("/v1/ci/runs/") else {
        return Ok(());
    };
    let coordinates = tail.split('/').collect::<Vec<_>>();
    match coordinates.as_slice() {
        [run] if canonical_uuid(run) => validate_ci_run_detail(run, body),
        [run, "jobs", job, "log"] if canonical_uuid(run) && canonical_uuid(job) => {
            validate_ci_log_range(call, run, job, body)
        }
        _ => malformed_ci("request route is not canonical"),
    }
}

fn validate_ci_run_list(call: &EdgeCall, body: &Value) -> Result<(), CliError> {
    let object = exact_object(body, &["items", "page"], "run list")?;
    let items = object["items"]
        .as_array()
        .ok_or_else(|| malformed_ci_error("run list items must be an array"))?;
    let page = exact_object(&object["page"], &["next_cursor", "limit"], "run list page")?;
    let requested_state = query_value(call, "state")
        .filter(|state| valid_run_state(state, true))
        .ok_or_else(|| malformed_ci_error("request state is not canonical"))?;
    let requested_limit = query_value(call, "limit")
        .and_then(canonical_u32)
        .filter(|limit| (1..=100).contains(limit))
        .ok_or_else(|| malformed_ci_error("request limit is not canonical"))?;
    if page["limit"].as_u64() != Some(u64::from(requested_limit))
        || items.len() > requested_limit as usize
    {
        return malformed_ci("run list page does not match the requested limit");
    }
    match &page["next_cursor"] {
        Value::Null => {}
        Value::String(cursor) if canonical_ci_cursor(cursor) => {}
        _ => return malformed_ci("run list cursor is not canonical"),
    }
    for item in items {
        validate_ci_run_summary(item)?;
        if requested_state != "all" && item["state"].as_str() != Some(requested_state) {
            return malformed_ci("run list item violates the requested state filter");
        }
    }
    Ok(())
}

fn validate_ci_run_detail(requested_run: &str, body: &Value) -> Result<(), CliError> {
    let object = exact_object(body, &["run", "jobs", "steps"], "run detail")?;
    validate_ci_run_summary(&object["run"])?;
    if object["run"]["run_id"].as_str() != Some(requested_run) {
        return malformed_ci("run detail id does not match the request");
    }
    let jobs = object["jobs"]
        .as_array()
        .ok_or_else(|| malformed_ci_error("run detail jobs must be an array"))?;
    if jobs.len() > 10_000 {
        return malformed_ci("run detail has too many jobs");
    }
    let mut job_ids = std::collections::BTreeSet::new();
    let mut dependencies = std::collections::BTreeMap::new();
    for job in jobs {
        let job = exact_object(
            job,
            &[
                "job_id",
                "stage",
                "name",
                "needs",
                "matrix_key",
                "state",
                "attempt",
                "result_summary",
            ],
            "run job",
        )?;
        let job_id = canonical_uuid_field(job, "job_id", "run job")?;
        if !job_ids.insert(job_id) {
            return malformed_ci("run detail contains a duplicate job id");
        }
        bounded_string_field(job, "stage", "run job", 256)?;
        bounded_string_field(job, "name", "run job", 512)?;
        if !job["state"].as_str().is_some_and(valid_job_state) {
            return malformed_ci("run job state is invalid");
        }
        let needs = job["needs"]
            .as_array()
            .ok_or_else(|| malformed_ci_error("run job needs must be an array"))?;
        if needs.len() > 1_000 {
            return malformed_ci("run job has too many dependencies");
        }
        let mut job_dependencies = std::collections::BTreeSet::new();
        for need in needs {
            let Some(need) = need.as_str().filter(|value| canonical_uuid(value)) else {
                return malformed_ci("run job dependency is not a canonical UUID");
            };
            if need == job_id || !job_dependencies.insert(need) {
                return malformed_ci("run job dependency is self-referential or duplicated");
            }
        }
        dependencies.insert(job_id, job_dependencies);
        if !job["attempt"]
            .as_i64()
            .is_some_and(|attempt| (1..=i64::from(i32::MAX)).contains(&attempt))
        {
            return malformed_ci("run job attempt must be positive");
        }
    }
    if dependencies
        .values()
        .flatten()
        .any(|dependency| !job_ids.contains(*dependency))
    {
        return malformed_ci("run job dependency names a job outside the response");
    }
    if ci_graph_has_cycle(&dependencies) {
        return malformed_ci("run job dependency graph contains a cycle");
    }
    let steps = object["steps"]
        .as_array()
        .ok_or_else(|| malformed_ci_error("run detail steps must be an array"))?;
    if steps.len() > 100_000 {
        return malformed_ci("run detail has too many steps");
    }
    let mut step_ids = std::collections::BTreeSet::new();
    for step in steps {
        let step = exact_object(
            step,
            &[
                "job_id",
                "step_id",
                "byte_start",
                "byte_end",
                "status",
                "details_ref",
            ],
            "run step",
        )?;
        let job_id = canonical_uuid_field(step, "job_id", "run step")?;
        if !job_ids.contains(job_id) {
            return malformed_ci("run step names a job outside the response");
        }
        let step_id = bounded_string_field(step, "step_id", "run step", 512)?;
        if !step_ids.insert((job_id, step_id)) {
            return malformed_ci("run detail contains a duplicate step id");
        }
        if !step["status"].as_str().is_some_and(valid_step_status) {
            return malformed_ci("run step status is invalid");
        }
        let start = step["byte_start"]
            .as_i64()
            .filter(|value| *value >= 0)
            .ok_or_else(|| malformed_ci_error("run step byte start must be non-negative"))?;
        match step["byte_end"].as_i64() {
            Some(end) if end >= start => {}
            None if step["byte_end"].is_null() => {}
            _ => return malformed_ci("run step byte end precedes its start"),
        }
        let expected_details_ref = format!("#step-{step_id}");
        if step["details_ref"].as_str() != Some(expected_details_ref.as_str()) {
            return malformed_ci("run step details ref is not canonical");
        }
    }
    Ok(())
}

fn validate_ci_run_summary(value: &Value) -> Result<(), CliError> {
    let run = exact_object(
        value,
        &[
            "run_id",
            "pipeline_id",
            "repo_ref",
            "commit_oid",
            "trigger_kind",
            "trust_tier",
            "state",
            "cost_settled",
            "created_at",
            "finished_at",
        ],
        "run summary",
    )?;
    canonical_uuid_field(run, "run_id", "run summary")?;
    canonical_uuid_field(run, "pipeline_id", "run summary")?;
    bounded_string_field(run, "repo_ref", "run summary", 1_024)?;
    if !run["trigger_kind"].as_str().is_some_and(valid_trigger_kind)
        || !run["trust_tier"].as_str().is_some_and(valid_trust_tier)
    {
        return malformed_ci("run summary trigger kind or trust tier is invalid");
    }
    if !run["state"]
        .as_str()
        .is_some_and(|state| valid_run_state(state, false))
        || !run["cost_settled"].is_boolean()
    {
        return malformed_ci("run summary state or settlement is invalid");
    }
    if !(run["commit_oid"].is_null()
        || run["commit_oid"]
            .as_str()
            .is_some_and(|value| bounded_string(value, 256)))
    {
        return malformed_ci("run summary commit oid is invalid");
    }
    if !run["created_at"].as_str().is_some_and(canonical_ci_time)
        || !(run["finished_at"].is_null()
            || run["finished_at"].as_str().is_some_and(canonical_ci_time))
    {
        return malformed_ci("run summary timestamp is not canonical");
    }
    Ok(())
}

fn validate_ci_log_range(
    call: &EdgeCall,
    requested_run: &str,
    requested_job: &str,
    body: &Value,
) -> Result<(), CliError> {
    let log = exact_object(
        body,
        &[
            "run_id",
            "job_id",
            "byte_start",
            "byte_end",
            "total_end",
            "next_offset",
            "encoding",
            "data",
        ],
        "log range",
    )?;
    if log["run_id"].as_str() != Some(requested_run)
        || log["job_id"].as_str() != Some(requested_job)
    {
        return malformed_ci("log range ids do not match the request");
    }
    let requested_start = query_value(call, "start")
        .and_then(canonical_i64)
        .filter(|value| *value >= 0)
        .ok_or_else(|| malformed_ci_error("request log start is not canonical"))?;
    let requested_limit = query_value(call, "limit")
        .and_then(canonical_u32)
        .filter(|value| (1..=256 * 1024).contains(value))
        .ok_or_else(|| malformed_ci_error("request log limit is not canonical"))?;
    let start = log["byte_start"]
        .as_i64()
        .filter(|value| *value == requested_start)
        .ok_or_else(|| malformed_ci_error("log range start does not match the request"))?;
    let end = log["byte_end"]
        .as_i64()
        .ok_or_else(|| malformed_ci_error("log range end must be an integer"))?;
    let total = log["total_end"]
        .as_i64()
        .ok_or_else(|| malformed_ci_error("log range total must be an integer"))?;
    if total < 0 {
        return malformed_ci("log range total is negative");
    }
    if start >= total {
        if end != start || !log["next_offset"].is_null() {
            return malformed_ci("beyond-end log range is contradictory");
        }
    } else {
        let requested_end = start
            .checked_add(i64::from(requested_limit))
            .ok_or_else(|| malformed_ci_error("requested log range overflows"))?;
        if end < start || total < end || end != requested_end.min(total) {
            return malformed_ci("log range coordinates are contradictory");
        }
        match &log["next_offset"] {
            Value::Null if end == total => {}
            Value::Number(number) if end < total && number.as_i64() == Some(end) => {}
            _ => return malformed_ci("log continuation offset is contradictory"),
        }
    }
    if log["encoding"].as_str() != Some("base64") {
        return malformed_ci("log range encoding is not base64");
    }
    let encoded = log["data"]
        .as_str()
        .ok_or_else(|| malformed_ci_error("log range data must be a string"))?;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| malformed_ci_error("log range data is not canonical base64"))?;
    if base64::engine::general_purpose::STANDARD.encode(&decoded) != encoded {
        return malformed_ci("log range data is not canonical base64");
    }
    let expected_len = usize::try_from(end - start)
        .map_err(|_| malformed_ci_error("log range length is not representable"))?;
    if decoded.len() != expected_len || decoded.len() > 256 * 1024 {
        return malformed_ci("log range bytes do not match the declared coordinates");
    }
    Ok(())
}

fn exact_object<'a>(
    value: &'a Value,
    fields: &[&str],
    shape: &str,
) -> Result<&'a serde_json::Map<String, Value>, CliError> {
    let object = value
        .as_object()
        .ok_or_else(|| malformed_ci_error(format!("{shape} must be an object")))?;
    if object.len() != fields.len() || !fields.iter().all(|field| object.contains_key(*field)) {
        return malformed_ci(format!("{shape} has missing or unknown fields"));
    }
    Ok(object)
}

fn canonical_uuid_field<'a>(
    object: &'a serde_json::Map<String, Value>,
    field: &str,
    shape: &str,
) -> Result<&'a str, CliError> {
    object[field]
        .as_str()
        .filter(|value| canonical_uuid(value))
        .ok_or_else(|| malformed_ci_error(format!("{shape} {field} is not a canonical UUID")))
}

fn bounded_string_field<'a>(
    object: &'a serde_json::Map<String, Value>,
    field: &str,
    shape: &str,
    max: usize,
) -> Result<&'a str, CliError> {
    object[field]
        .as_str()
        .filter(|value| bounded_string(value, max))
        .ok_or_else(|| malformed_ci_error(format!("{shape} {field} is not a bounded string")))
}

fn bounded_string(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && !value
            .chars()
            .any(|character| character <= '\u{1f}' || character == '\u{7f}')
}

fn query_value<'a>(call: &'a EdgeCall, name: &str) -> Option<&'a str> {
    call.query.as_deref()?.split('&').find_map(|pair| {
        let (field, value) = pair.split_once('=')?;
        (field == name).then_some(value)
    })
}

fn canonical_u32(value: &str) -> Option<u32> {
    let parsed: u32 = value.parse().ok()?;
    (parsed.to_string() == value).then_some(parsed)
}

fn canonical_i64(value: &str) -> Option<i64> {
    let parsed: i64 = value.parse().ok()?;
    (parsed.to_string() == value).then_some(parsed)
}

fn canonical_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte),
        })
}

fn canonical_ci_cursor(value: &str) -> bool {
    let Some(encoded) = value.strip_prefix("cr1_").filter(|_| value.len() <= 256) else {
        return false;
    };
    let Ok(frame) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(encoded) else {
        return false;
    };
    frame.len() == 60
        && frame[0] == 1
        && base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(frame) == encoded
}

fn valid_run_state(value: &str, allow_all: bool) -> bool {
    matches!(
        value,
        "queued" | "running" | "succeeded" | "failed" | "cancelled" | "timed_out" | "reaped"
    ) || (allow_all && value == "all")
}

fn valid_job_state(value: &str) -> bool {
    matches!(
        value,
        "queued" | "leased" | "running" | "succeeded" | "failed" | "cancelled" | "reaped"
    )
}

fn valid_step_status(value: &str) -> bool {
    matches!(value, "running" | "passed" | "failed" | "skipped")
}

fn valid_trigger_kind(value: &str) -> bool {
    matches!(
        value,
        "push" | "pull_request" | "issue_transition" | "manual" | "agent" | "schedule"
    )
}

fn valid_trust_tier(value: &str) -> bool {
    matches!(value, "trusted" | "untrusted_fork" | "self_hosted")
}

fn canonical_ci_time(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 27
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
        || bytes[19] != b'.'
        || bytes[26] != b'Z'
    {
        return false;
    }
    for index in [
        0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18, 20, 21, 22, 23, 24, 25,
    ] {
        if !bytes[index].is_ascii_digit() {
            return false;
        }
    }
    chrono::DateTime::parse_from_rfc3339(value).is_ok()
}

fn ci_graph_has_cycle<'a>(
    dependencies: &std::collections::BTreeMap<&'a str, std::collections::BTreeSet<&'a str>>,
) -> bool {
    let mut remaining = dependencies
        .iter()
        .map(|(job, needs)| (*job, needs.len()))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut dependents = std::collections::BTreeMap::<&str, Vec<&str>>::new();
    for (job, needs) in dependencies {
        for need in needs {
            dependents.entry(*need).or_default().push(*job);
        }
    }
    let mut ready = remaining
        .iter()
        .filter_map(|(job, count)| (*count == 0).then_some(*job))
        .collect::<std::collections::VecDeque<_>>();
    let mut visited = 0;
    while let Some(job) = ready.pop_front() {
        visited += 1;
        if let Some(waiting) = dependents.get(job) {
            for dependent in waiting {
                let Some(count) = remaining.get_mut(*dependent) else {
                    return true;
                };
                *count -= 1;
                if *count == 0 {
                    ready.push_back(dependent);
                }
            }
        }
    }
    visited != dependencies.len()
}

fn malformed_ci<T>(reason: impl Into<String>) -> Result<T, CliError> {
    Err(malformed_ci_error(reason))
}

fn malformed_ci_error(reason: impl Into<String>) -> CliError {
    CliError::Transport(format!(
        "edge returned a malformed CI success response: {}",
        reason.into()
    ))
}

pub fn interpret(status: u16, body: Value) -> Result<Value, CliError> {
    if (200..300).contains(&status) {
        return Ok(body);
    }
    let message = body
        .get("error")
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("(no message)")
        .to_string();
    let code = body
        .get("error")
        .and_then(|e| e.get("code"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if status == 401 {
        return Err(CliError::Unauthorized(message));
    }
    Err(CliError::Edge {
        status,
        code,
        message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::{EdgeCall, HttpMethod};
    use serde_json::json;

    fn cfg(url: &str) -> EdgeConfig {
        EdgeConfig {
            url: url.into(),
            scheme: "agent".into(),
        }
    }
    fn get(path: &str, query: Option<&str>) -> EdgeCall {
        EdgeCall {
            method: HttpMethod::Get,
            path: path.into(),
            query: query.map(str::to_string),
            payload: None,
            idempotency_key: None,
            retry_policy: RetryPolicy::None,
        }
    }

    fn golden_expected(id: &str) -> Value {
        let contract: Value = serde_json::from_str(include_str!(
            "../../../contracts/ci-read-dev-edge.golden.json"
        ))
        .expect("CI contract artifact is JSON");
        let mut expected = contract["vectors"]
            .as_array()
            .expect("vectors")
            .iter()
            .find(|vector| vector["id"] == id)
            .unwrap_or_else(|| panic!("missing CI vector {id}"))["expected"]
            .clone();
        expected
            .as_object_mut()
            .expect("expected response is an object")
            .remove("status");
        expected
    }

    fn canonical_cursor() -> String {
        let mut frame = [0_u8; 60];
        frame[0] = 1;
        format!(
            "cr1_{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(frame)
        )
    }

    #[test]
    fn target_admits_https_and_only_loopback_http() {
        let local = target(&cfg("http://127.0.0.1:8080"), &get("/v1/git/repos", None)).unwrap();
        assert_eq!(local.authority, "127.0.0.1:8080");
        assert_eq!(local.uri.to_string(), "http://127.0.0.1:8080/v1/git/repos");

        let secure = target(
            &cfg("https://edge.example.com/platform"),
            &get("/v1/git/search/code", Some("q=x")),
        )
        .unwrap();
        assert_eq!(secure.authority, "edge.example.com");
        assert_eq!(
            secure.uri.to_string(),
            "https://edge.example.com/platform/v1/git/search/code?q=x"
        );

        for local in [
            "http://localhost:8080",
            "http://127.42.0.1:8080",
            "http://[::1]:8080",
        ] {
            assert!(target(&cfg(local), &get("/v1/whoami", None)).is_ok());
        }
        assert!(target(&cfg("http://edge.example.com"), &get("/v1/whoami", None)).is_err());
        assert!(target(&cfg("ftp://edge.example.com"), &get("/v1/whoami", None)).is_err());
        assert!(target(
            &cfg("https://user@edge.example.com"),
            &get("/v1/whoami", None)
        )
        .is_err());
        assert!(target(
            &cfg("https://edge.example.com?route=other"),
            &get("/v1/whoami", None)
        )
        .is_err());
        assert!(target(
            &cfg("https://edge.example.com/#fragment"),
            &get("/v1/whoami", None)
        )
        .is_err());
    }

    #[tokio::test]
    async fn response_body_is_bounded_before_json_parsing() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 2048];
            let _ = stream.read(&mut request).await;
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 17\r\n\r\n{\"items\":[1,2]}")
                .await
                .unwrap();
        });

        let error = execute_with_limits(
            &cfg(&format!("http://{address}")),
            "sensitive-token",
            &get("/v1/whoami", None),
            Duration::from_secs(1),
            8,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("8-byte limit"));
        assert!(!error.to_string().contains("sensitive-token"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn one_deadline_bounds_connect_headers_and_body() {
        use tokio::io::AsyncReadExt;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 2048];
            let _ = stream.read(&mut request).await;
            std::future::pending::<()>().await;
        });

        let error = execute_with_limits(
            &cfg(&format!("http://{address}")),
            "sensitive-token",
            &get("/v1/whoami", None),
            Duration::from_millis(20),
            1024,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("deadline"));
        assert!(!error.to_string().contains("sensitive-token"));
        server.abort();
    }

    #[tokio::test]
    async fn device_login_calls_only_the_two_public_routes_without_a_bearer() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4096];
            let read = stream.read(&mut request).await.unwrap();
            stream
                .write_all(
                    b"HTTP/1.1 202 Accepted\r\nContent-Length: 47\r\n\r\n{\"status\":\"authorization_pending\",\"interval\":2}",
                )
                .await
                .unwrap();
            String::from_utf8_lossy(&request[..read]).to_string()
        });
        let call = EdgeCall {
            method: HttpMethod::Post,
            path: "/v1/auth/device/token".into(),
            query: None,
            payload: Some(b"{}".to_vec()),
            idempotency_key: None,
            retry_policy: RetryPolicy::None,
        };

        let response = execute_device_auth(&cfg(&format!("http://{address}")), &call)
            .await
            .unwrap();
        assert_eq!(response["status"], "authorization_pending");
        let request = server.await.unwrap().to_ascii_lowercase();
        assert!(!request.contains("\r\nauthorization:"));
        assert!(!request.contains("\r\nx-myelin-token-scheme:"));
        assert!(!request.contains("\r\nidempotency-key:"));

        let mut ordinary_mutation = call;
        ordinary_mutation.path = "/v1/git/repos".into();
        assert!(
            execute_device_auth(&cfg("http://127.0.0.1:1"), &ordinary_mutation)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn mutations_require_and_transmit_the_exact_retry_stable_key() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let missing = EdgeCall {
            method: HttpMethod::Post,
            path: "/v1/git/repos/core/prs".into(),
            query: None,
            payload: Some(b"{}".to_vec()),
            idempotency_key: None,
            retry_policy: RetryPolicy::CallerKeyRequired,
        };
        let error = execute_with_limits(
            &cfg("http://127.0.0.1:1"),
            "sensitive-token",
            &missing,
            Duration::from_secs(1),
            1024,
        )
        .await
        .unwrap_err();
        assert_eq!(error.code(), 2);
        assert!(error.to_string().contains("--idempotency-key"));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4096];
            let read = stream.read(&mut request).await.unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")
                .await
                .unwrap();
            String::from_utf8_lossy(&request[..read]).to_string()
        });
        let keyed = missing
            .with_idempotency_key("retry-pr-open-123")
            .expect("valid explicit mutation key");
        execute_with_limits(
            &cfg(&format!("http://{address}")),
            "sensitive-token",
            &keyed,
            Duration::from_secs(1),
            1024,
        )
        .await
        .expect("keyed mutation succeeds");
        let request = server.await.unwrap().to_ascii_lowercase();
        assert!(
            request.contains("\r\nidempotency-key: retry-pr-open-123\r\n"),
            "the exact caller key reaches the wire"
        );
    }

    #[test]
    fn interpret_maps_status_to_typed_errors() {
        let ok = interpret(200, json!({"items":[]})).unwrap();
        assert_eq!(ok["items"], json!([]));
        let e = interpret(
            401,
            json!({"error":{"message":"authentication required","code":"unauthorized"}}),
        )
        .unwrap_err();
        assert_eq!(e.code(), 3);
        let e = interpret(
            404,
            json!({"error":{"message":"no such pull request","code":"not_found"}}),
        )
        .unwrap_err();
        match e {
            CliError::Edge {
                status,
                code,
                message,
            } => {
                assert_eq!(status, 404);
                assert_eq!(code, "not_found");
                assert_eq!(message, "no such pull request");
            }
            other => panic!("expected Edge, got {other:?}"),
        }
        assert!(interpret(500, Value::Null).is_err());
    }

    #[test]
    fn ci_success_decoder_accepts_the_shared_contract_vectors() {
        let run = "91000000-0000-4000-8000-000000000001";
        let job = "92000000-0000-4000-8000-000000000001";
        let mut list = golden_expected("runs-first-page-keyset");
        list["page"]["next_cursor"] = json!(canonical_cursor());
        validate_ci_success(&get("/v1/ci/runs", Some("state=all&limit=1")), &list).unwrap();
        validate_ci_success(
            &get(&format!("/v1/ci/runs/{run}"), None),
            &golden_expected("failed-run-detail"),
        )
        .unwrap();
        validate_ci_success(
            &get(
                &format!("/v1/ci/runs/{run}/jobs/{job}/log"),
                Some("start=9&limit=7"),
            ),
            &golden_expected("archived-log-byte-range"),
        )
        .unwrap();
    }

    #[test]
    fn ci_list_success_is_bound_to_requested_filter_limit_and_exact_shape() {
        let call = get("/v1/ci/runs", Some("state=all&limit=1"));
        let mut response = golden_expected("runs-first-page-keyset");
        response["page"]["next_cursor"] = json!(canonical_cursor());

        let mut wrong_limit = response.clone();
        wrong_limit["page"]["limit"] = json!(2);
        assert!(validate_ci_success(&call, &wrong_limit).is_err());

        let filtered_call = get("/v1/ci/runs", Some("state=running&limit=1"));
        assert!(validate_ci_success(&filtered_call, &response).is_err());

        let mut noncanonical_cursor = response.clone();
        noncanonical_cursor["page"]["next_cursor"] = json!("cr1_bad=");
        assert!(validate_ci_success(&call, &noncanonical_cursor).is_err());

        let mut extra_field = response;
        extra_field["page"]["offset"] = json!(1);
        assert!(validate_ci_success(&call, &extra_field).is_err());
    }

    #[test]
    fn ci_detail_success_is_bound_to_requested_run_and_internal_job_ids() {
        let run = "91000000-0000-4000-8000-000000000001";
        let call = get(&format!("/v1/ci/runs/{run}"), None);
        let response = golden_expected("failed-run-detail");

        let mut wrong_run = response.clone();
        wrong_run["run"]["run_id"] = json!("91000000-0000-4000-8000-000000000002");
        assert!(validate_ci_success(&call, &wrong_run).is_err());

        let mut alien_step = response.clone();
        alien_step["steps"][0]["job_id"] = json!("92000000-0000-4000-8000-000000000002");
        assert!(validate_ci_success(&call, &alien_step).is_err());

        let mut alien_dependency = response.clone();
        alien_dependency["jobs"][0]["needs"] = json!(["92000000-0000-4000-8000-000000000002"]);
        assert!(validate_ci_success(&call, &alien_dependency).is_err());

        let mut self_dependency = response.clone();
        self_dependency["jobs"][0]["needs"] = json!(["92000000-0000-4000-8000-000000000001"]);
        assert!(validate_ci_success(&call, &self_dependency).is_err());

        let mut duplicate_dependency = response.clone();
        duplicate_dependency["jobs"][0]["needs"] = json!([
            "92000000-0000-4000-8000-000000000002",
            "92000000-0000-4000-8000-000000000002"
        ]);
        assert!(validate_ci_success(&call, &duplicate_dependency).is_err());

        let mut cycle = response.clone();
        let mut second_job = cycle["jobs"][0].clone();
        second_job["job_id"] = json!("92000000-0000-4000-8000-000000000002");
        second_job["needs"] = json!(["92000000-0000-4000-8000-000000000001"]);
        cycle["jobs"][0]["needs"] = json!(["92000000-0000-4000-8000-000000000002"]);
        cycle["jobs"].as_array_mut().unwrap().push(second_job);
        assert!(validate_ci_success(&call, &cycle).is_err());

        let mut extreme_attempt = response.clone();
        extreme_attempt["jobs"][0]["attempt"] = json!(i64::MAX);
        assert!(validate_ci_success(&call, &extreme_attempt).is_err());

        let mut duplicate_step = response.clone();
        let repeated_step = duplicate_step["steps"][0].clone();
        duplicate_step["steps"]
            .as_array_mut()
            .unwrap()
            .push(repeated_step);
        assert!(validate_ci_success(&call, &duplicate_step).is_err());

        let mut bad_range = response.clone();
        bad_range["steps"][0]["byte_end"] = json!(-1);
        assert!(validate_ci_success(&call, &bad_range).is_err());

        let mut bad_step_status = response;
        bad_step_status["steps"][0]["status"] = json!("unknown");
        assert!(validate_ci_success(&call, &bad_step_status).is_err());
    }

    #[test]
    fn ci_run_success_rejects_unknown_enums_and_noncanonical_times() {
        let call = get("/v1/ci/runs", Some("state=all&limit=1"));
        let mut response = golden_expected("runs-first-page-keyset");
        response["page"]["next_cursor"] = json!(canonical_cursor());

        let mut trigger = response.clone();
        trigger["items"][0]["trigger_kind"] = json!("webhook");
        assert!(validate_ci_success(&call, &trigger).is_err());

        let mut trust = response.clone();
        trust["items"][0]["trust_tier"] = json!("unknown");
        assert!(validate_ci_success(&call, &trust).is_err());

        let mut timestamp = response;
        timestamp["items"][0]["created_at"] = json!("2026-02-30T12:00:00.000000Z");
        assert!(validate_ci_success(&call, &timestamp).is_err());
    }

    #[test]
    fn ci_log_success_rejects_mismatched_ids_ranges_and_bytes() {
        let run = "91000000-0000-4000-8000-000000000001";
        let job = "92000000-0000-4000-8000-000000000001";
        let call = get(
            &format!("/v1/ci/runs/{run}/jobs/{job}/log"),
            Some("start=9&limit=7"),
        );
        let response = golden_expected("archived-log-byte-range");

        let mut wrong_job = response.clone();
        wrong_job["job_id"] = json!("92000000-0000-4000-8000-000000000002");
        assert!(validate_ci_success(&call, &wrong_job).is_err());

        let mut contradictory_range = response.clone();
        contradictory_range["byte_end"] = json!(i64::MAX);
        assert!(validate_ci_success(&call, &contradictory_range).is_err());

        let mut wrong_continuation = response.clone();
        wrong_continuation["next_offset"] = json!(15);
        assert!(validate_ci_success(&call, &wrong_continuation).is_err());

        let mut noncanonical_base64 = response.clone();
        noncanonical_base64["data"] = json!("qQpmYWlsZQ");
        assert!(validate_ci_success(&call, &noncanonical_base64).is_err());

        let mut wrong_decoded_length = response;
        wrong_decoded_length["data"] = json!("ZmFpbA==");
        assert!(validate_ci_success(&call, &wrong_decoded_length).is_err());
    }

    #[test]
    fn ci_log_success_accepts_the_production_beyond_end_empty_range() {
        let run = "91000000-0000-4000-8000-000000000001";
        let job = "92000000-0000-4000-8000-000000000001";
        let call = get(
            &format!("/v1/ci/runs/{run}/jobs/{job}/log"),
            Some("start=100&limit=7"),
        );
        validate_ci_success(
            &call,
            &json!({
                "run_id": run,
                "job_id": job,
                "byte_start": 100,
                "byte_end": 100,
                "total_end": 18,
                "next_offset": null,
                "encoding": "base64",
                "data": ""
            }),
        )
        .unwrap();
    }
}
