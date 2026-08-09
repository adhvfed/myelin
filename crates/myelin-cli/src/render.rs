use crate::dispatch::EdgeCall;
use base64::Engine as _;
use myelin_ci_controlplane::surfacing_store::CI_RUN_CURSOR_PREFIX;
use myelin_git::web::RepoListCursor;
use serde_json::Value;
use std::fmt::Write as _;

mod collaboration;

fn terminal_safe_single_line(value: &str) -> String {
    let mut safe = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\n' => safe.push_str("\\n"),
            '\r' => safe.push_str("\\r"),
            '\t' => safe.push_str("\\t"),
            '\u{2028}' => safe.push_str("\\u{2028}"),
            '\u{2029}' => safe.push_str("\\u{2029}"),
            character if character.is_control() => {
                let codepoint = character as u32;
                if codepoint <= 0xff {
                    let _ = write!(safe, "\\x{codepoint:02x}");
                } else {
                    let _ = write!(safe, "\\u{{{codepoint:x}}}");
                }
            }
            character => safe.push(character),
        }
    }
    safe
}

pub fn render(value: &Value, json_mode: bool) -> String {
    render_with_call(value, json_mode, None)
}

pub fn render_for_call(value: &Value, json_mode: bool, call: &EdgeCall) -> String {
    render_with_call(value, json_mode, Some(call))
}

fn render_with_call(value: &Value, json_mode: bool, call: Option<&EdgeCall>) -> String {
    if json_mode {
        return serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    }
    if is_ci_log_range(value) {
        return render_ci_log_range(value);
    }
    if is_ci_run_detail(value) {
        return render_ci_run_detail(value);
    }
    if let Some(rendered) = collaboration::render_response(value) {
        return rendered;
    }
    if let Some(rendered) = render_issue_import(value) {
        return rendered;
    }
    if let Some(items) = value.get("items").and_then(Value::as_array) {
        let mut out = collaboration::render_collection_header(value).unwrap_or_default();
        if items.is_empty() {
            out.push_str("(no items)\n");
        }
        for item in items {
            out.push_str(&render_item(item));
            out.push('\n');
        }
        if let Some(cursor) = value
            .get("page")
            .and_then(|p| p.get("next_cursor"))
            .and_then(Value::as_str)
        {
            if let Some(command) = call.and_then(|call| {
                collaboration::page_command(call, cursor).or_else(|| ci_page_command(call, cursor))
            }) {
                out.push_str(&format!("… (more - run: {command})\n"));
            } else if RepoListCursor::parse(cursor).is_ok() {
                let cursor = terminal_safe_single_line(cursor);
                out.push_str(&format!(
                    "… (more - run: myelin repo list --cursor {cursor})\n"
                ));
            } else {
                out.push_str("… (more - pass --cursor to page)\n");
            }
        }
        return out;
    }
    if let Some(pid) = value.get("principal_id").and_then(Value::as_str) {
        let kind = value.get("kind").and_then(Value::as_str).unwrap_or("?");
        let tenant = value.get("tenant").and_then(Value::as_str).unwrap_or("?");
        let region = value.get("region").and_then(Value::as_str).unwrap_or("?");
        let pid = terminal_safe_single_line(pid);
        let kind = terminal_safe_single_line(kind);
        let tenant = terminal_safe_single_line(tenant);
        let region = terminal_safe_single_line(region);
        return format!("{pid} ({kind})  tenant={tenant}  region={region}\n");
    }
    if let (Some(issue), Some(authorization)) = (value.get("issue"), value.get("authorization")) {
        if let (Some(id), Some(key)) = (
            issue.get("id").and_then(Value::as_str),
            issue.get("key").and_then(Value::as_str),
        ) {
            let status = authorization
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("pending");
            let id = terminal_safe_single_line(id);
            let key = terminal_safe_single_line(key);
            let status = terminal_safe_single_line(status);
            return format!(
                "{key} staged ({id}); authorization={status}\nnot visible yet; after reconciliation: myelin issue view {id}\n"
            );
        }
    }
    if is_issue(value) {
        return format!("{}\n", render_issue(value));
    }
    if is_notification(value) {
        return format!("{}\n", render_notification(value));
    }
    if let (Some(id), Some(state)) = (
        value.get("id").and_then(Value::as_str),
        value.get("state").and_then(Value::as_str),
    ) {
        if value.as_object().is_some_and(|object| object.len() == 2) {
            return format!(
                "{} [{}]\n",
                terminal_safe_single_line(id),
                terminal_safe_single_line(state)
            );
        }
    }
    let serialized = serde_json::to_string(value).unwrap_or_else(|_| value.to_string());
    format!("{}\n", terminal_safe_single_line(&serialized))
}

fn render_issue_import(value: &Value) -> Option<String> {
    let import = value.get("import")?.as_object()?;
    let job = terminal_safe_single_line(import.get("job_id")?.as_str()?);
    let source = terminal_safe_single_line(import.get("source")?.as_str()?);
    match import.get("mode")?.as_str()? {
        "dry_run" => {
            let report = value.get("reconciliation")?.as_object()?;
            let received = report.get("received")?.as_u64()?;
            let ready = report.get("ready")?.as_u64()?;
            let lossy = report.get("lossy")?.as_u64()?;
            let dropped = report.get("dropped")?.as_u64()?;
            Some(format!(
                "Import preview {job} ({source}): {ready}/{received} ready, {lossy} lossy, {dropped} dropped; no data written.\n"
            ))
        }
        "run" => {
            let summary = value.get("summary")?.as_object()?;
            let created = summary.get("created")?.as_u64()?;
            let resumed = summary.get("resumed")?.as_u64()?;
            let lossy = summary.get("lossy")?.as_u64()?;
            let dropped = summary.get("dropped")?.as_u64()?;
            let mut output = format!(
                "Import {job} ({source}): {created} created, {resumed} resumed, {lossy} lossy, {dropped} dropped.\n"
            );
            for outcome in value.get("issues")?.as_array()? {
                let source_id = terminal_safe_single_line(outcome.get("source_id")?.as_str()?);
                let issue = outcome.get("issue")?;
                let key = terminal_safe_single_line(issue.get("key")?.as_str()?);
                let id = terminal_safe_single_line(issue.get("id")?.as_str()?);
                let disposition = if outcome.get("created")?.as_bool()? {
                    "created"
                } else {
                    "resumed"
                };
                let authorization = terminal_safe_single_line(
                    outcome.get("authorization")?.get("status")?.as_str()?,
                );
                let _ = writeln!(
                    output,
                    "{source_id} -> {key} staged ({id}); {disposition}; authorization={authorization}"
                );
            }
            Some(output)
        }
        _ => None,
    }
}

fn render_item(item: &Value) -> String {
    if is_ci_run_summary(item) {
        return render_ci_run_summary(item);
    }
    if is_issue(item) {
        return render_issue(item);
    }
    if is_notification(item) {
        return render_notification(item);
    }
    if let Some(rendered) = collaboration::render_item(item) {
        return rendered;
    }
    if let Some(slug) = item.get("slug").and_then(Value::as_str) {
        let state = item.get("state").and_then(Value::as_str).unwrap_or("?");
        let slug = terminal_safe_single_line(slug);
        let state = terminal_safe_single_line(state);
        return format!("{slug} [{state}]");
    }
    if let (Some(repo), Some(path)) = (
        item.get("repo").and_then(Value::as_str),
        item.get("path").and_then(Value::as_str),
    ) {
        let line = item.get("line").and_then(Value::as_i64).unwrap_or(0);
        let excerpt = item.get("excerpt").and_then(Value::as_str).unwrap_or("");
        let repo = terminal_safe_single_line(repo);
        let path = terminal_safe_single_line(path);
        let excerpt = terminal_safe_single_line(excerpt);
        return format!("{repo}:{path}:{line}  {excerpt}");
    }
    terminal_safe_single_line(&item.to_string())
}

fn is_ci_run_summary(value: &Value) -> bool {
    value.get("run_id").and_then(Value::as_str).is_some()
        && value.get("repo_ref").and_then(Value::as_str).is_some()
        && value.get("state").and_then(Value::as_str).is_some()
        && value.get("created_at").and_then(Value::as_str).is_some()
}

fn render_ci_run_summary(value: &Value) -> String {
    let state = value
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let marker = state_marker(state);
    let state = terminal_safe_single_line(state);
    let run = terminal_safe_single_line(value.get("run_id").and_then(Value::as_str).unwrap_or("?"));
    let repo =
        terminal_safe_single_line(value.get("repo_ref").and_then(Value::as_str).unwrap_or("?"));
    let commit = terminal_safe_single_line(
        value
            .get("commit_oid")
            .and_then(Value::as_str)
            .unwrap_or("no-commit"),
    );
    let created = terminal_safe_single_line(
        value
            .get("created_at")
            .and_then(Value::as_str)
            .unwrap_or("?"),
    );
    format!("{marker} {state}  {run}  {repo}@{commit}  {created}")
}

fn is_ci_run_detail(value: &Value) -> bool {
    value.get("run").is_some_and(is_ci_run_summary)
        && value.get("jobs").and_then(Value::as_array).is_some()
        && value.get("steps").and_then(Value::as_array).is_some()
}

fn render_ci_run_detail(value: &Value) -> String {
    let run = &value["run"];
    let run_id = run.get("run_id").and_then(Value::as_str).unwrap_or("?");
    let mut output = format!("{}\n", render_ci_run_summary(run));
    let jobs = value["jobs"].as_array().expect("shape checked");
    if jobs.is_empty() {
        output.push_str("  (no jobs)\n");
    }
    for job in jobs {
        let state = job
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let is_running = state == "running";
        let marker = state_marker(state);
        let state = terminal_safe_single_line(state);
        let stage =
            terminal_safe_single_line(job.get("stage").and_then(Value::as_str).unwrap_or("?"));
        let name =
            terminal_safe_single_line(job.get("name").and_then(Value::as_str).unwrap_or("?"));
        let job_id =
            terminal_safe_single_line(job.get("job_id").and_then(Value::as_str).unwrap_or("?"));
        let attempt = job.get("attempt").and_then(Value::as_i64).unwrap_or(0);
        output.push_str(&format!(
            "  {marker} {state}  {stage}/{name}  attempt={attempt}  {job_id}\n"
        ));
        if safe_cli_uuid(run_id)
            && job
                .get("job_id")
                .and_then(Value::as_str)
                .is_some_and(safe_cli_uuid)
        {
            output.push_str(&format!(
                "    archived output: myelin ci logs {run_id} --job {job_id}\n"
            ));
            if is_running {
                output.push_str(&format!(
                    "    live output: myelin ci watch {run_id} --job {job_id}\n"
                ));
            }
        }
    }
    for step in value["steps"].as_array().expect("shape checked") {
        let state = step
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let marker = state_marker(state);
        let state = terminal_safe_single_line(state);
        let step_id =
            terminal_safe_single_line(step.get("step_id").and_then(Value::as_str).unwrap_or("?"));
        let job_id =
            terminal_safe_single_line(step.get("job_id").and_then(Value::as_str).unwrap_or("?"));
        let start = step
            .get("byte_start")
            .and_then(Value::as_i64)
            .map_or_else(|| "?".into(), |value| value.to_string());
        let end = step
            .get("byte_end")
            .and_then(Value::as_i64)
            .map_or_else(|| "?".into(), |value| value.to_string());
        output.push_str(&format!(
            "    {marker} {state}  step {step_id}  job={job_id}  bytes={start}..{end}\n"
        ));
    }
    output
}

fn is_ci_log_range(value: &Value) -> bool {
    value.get("run_id").and_then(Value::as_str).is_some()
        && value.get("job_id").and_then(Value::as_str).is_some()
        && value.get("encoding").and_then(Value::as_str) == Some("base64")
        && value.get("data").and_then(Value::as_str).is_some()
        && value.get("byte_start").and_then(Value::as_i64).is_some()
        && value.get("byte_end").and_then(Value::as_i64).is_some()
}

fn render_ci_log_range(value: &Value) -> String {
    let start = value["byte_start"].as_i64().unwrap_or(0);
    let end = value["byte_end"].as_i64().unwrap_or(start);
    let total = value["total_end"].as_i64().unwrap_or(end);
    let mut output = format!("archived log bytes {start}..{end} of {total}\n");
    match base64::engine::general_purpose::STANDARD
        .decode(value["data"].as_str().unwrap_or_default())
    {
        Ok(bytes) => {
            output.push_str(&terminal_safe_log_bytes(&bytes));
            if !output.ends_with('\n') {
                output.push('\n');
            }
        }
        Err(_) => output.push_str("(archived log payload is malformed)\n"),
    }
    if let Some(next) = value.get("next_offset").and_then(Value::as_i64) {
        let run = value["run_id"].as_str().unwrap_or_default();
        let job = value["job_id"].as_str().unwrap_or_default();
        if next >= 0 && safe_cli_uuid(run) && safe_cli_uuid(job) {
            if let Some(limit) = end.checked_sub(start).filter(|limit| *limit > 0) {
                output.push_str(&format!(
                    "… (more - run: myelin ci logs {run} --job {job} --start {next} --limit {limit})\n"
                ));
            }
        }
    }
    output
}

pub fn terminal_safe_log_bytes(bytes: &[u8]) -> String {
    let mut output = String::new();
    let mut remaining = bytes;
    while !remaining.is_empty() {
        match std::str::from_utf8(remaining) {
            Ok(text) => {
                push_safe_log_text(&mut output, text);
                break;
            }
            Err(error) => {
                let valid = error.valid_up_to();
                if valid > 0 {
                    let text = std::str::from_utf8(&remaining[..valid])
                        .expect("valid_up_to always identifies valid UTF-8");
                    push_safe_log_text(&mut output, text);
                }
                let invalid = error.error_len().unwrap_or(remaining.len() - valid);
                for byte in &remaining[valid..valid + invalid] {
                    let _ = write!(output, "\\x{byte:02x}");
                }
                remaining = &remaining[valid + invalid..];
            }
        }
    }
    output
}

fn push_safe_log_text(output: &mut String, text: &str) {
    for character in text.chars() {
        if character == '\n' {
            output.push('\n');
        } else if character.is_control() || matches!(character, '\u{2028}' | '\u{2029}') {
            output.push_str(&terminal_safe_single_line(&character.to_string()));
        } else {
            output.push(character);
        }
    }
}

fn state_marker(state: &str) -> &'static str {
    match state {
        "succeeded" | "passed" => "✓",
        "failed" | "timed_out" | "reaped" => "✗",
        "running" => "●",
        "queued" => "○",
        "cancelled" => "–",
        _ => "?",
    }
}

fn ci_page_command(call: &EdgeCall, cursor: &str) -> Option<String> {
    if call.path != "/v1/ci/runs" || !safe_ci_cursor(cursor) {
        return None;
    }
    let query = call.query.as_deref()?;
    let state = query_field(query, "state").filter(|value| {
        matches!(
            *value,
            "all"
                | "queued"
                | "running"
                | "succeeded"
                | "failed"
                | "cancelled"
                | "timed_out"
                | "reaped"
        )
    })?;
    let limit = query_field(query, "limit")?;
    let parsed_limit = limit.parse::<u32>().ok()?;
    if parsed_limit.to_string() != limit || !(1..=100).contains(&parsed_limit) {
        return None;
    }
    Some(format!(
        "myelin ci list --status {state} --limit {limit} --cursor {cursor}"
    ))
}

fn query_field<'a>(query: &'a str, field: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name == field).then_some(value)
    })
}

fn safe_ci_cursor(cursor: &str) -> bool {
    cursor.len() <= 256
        && cursor
            .strip_prefix(CI_RUN_CURSOR_PREFIX)
            .is_some_and(|encoded| {
                !encoded.is_empty()
                    && encoded
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            })
}

fn safe_cli_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte),
        })
}

fn is_issue(value: &Value) -> bool {
    value.get("id").and_then(Value::as_str).is_some()
        && value.get("key").and_then(Value::as_str).is_some()
        && value.get("title").and_then(Value::as_str).is_some()
}

fn is_notification(value: &Value) -> bool {
    value.get("id").and_then(Value::as_str).is_some()
        && value.get("reason").and_then(Value::as_str).is_some()
        && value.get("subject").and_then(Value::as_str).is_some()
        && value.get("state").and_then(Value::as_str).is_some()
}

fn render_notification(value: &Value) -> String {
    let marker = match value.get("state").and_then(Value::as_str) {
        Some("unread") => "●",
        Some("done" | "archived") => "✓",
        _ => "○",
    };
    let reason = terminal_safe_single_line(
        value
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("notification"),
    );
    let subject =
        terminal_safe_single_line(value.get("subject").and_then(Value::as_str).unwrap_or("?"));
    let id = terminal_safe_single_line(value.get("id").and_then(Value::as_str).unwrap_or("?"));
    let count = value
        .get("coalesce_count")
        .and_then(Value::as_u64)
        .filter(|count| *count > 1)
        .map(|count| format!(" ×{count}"))
        .unwrap_or_default();
    format!("{marker} {reason}{count}  {subject}  ({id})")
}

fn render_issue(value: &Value) -> String {
    let key = terminal_safe_single_line(value.get("key").and_then(Value::as_str).unwrap_or("?"));
    let title = terminal_safe_single_line(value.get("title").and_then(Value::as_str).unwrap_or(""));
    let state =
        terminal_safe_single_line(value.get("state").and_then(Value::as_str).unwrap_or("?"));
    let id = terminal_safe_single_line(value.get("id").and_then(Value::as_str).unwrap_or("?"));
    format!("{key}  [{state}]  {title}  ({id})")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn canonical_ci_cursor() -> String {
        let mut frame = [0_u8; 60];
        frame[0] = 1;
        format!(
            "cr1_{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(frame)
        )
    }

    #[test]
    fn json_mode_is_pretty_raw() {
        let v = json!({
            "items": [
                {
                    "state": "populated",
                    "slug": "acme/alpha",
                    "clone_url": "/acme/eu-west/alpha.git"
                },
                {"state": "empty", "slug": "acme/empty"}
            ],
            "page": {"next_cursor": null, "limit": 50}
        });
        let out = render(&v, true);
        assert!(out.contains("\"slug\": \"acme/alpha\""));
        assert_eq!(serde_json::from_str::<Value>(&out).unwrap(), v);
        assert!(!out.contains("readme_excerpt") && !out.contains("entries"));
    }

    #[test]
    fn human_repo_list_is_one_line_per_repo() {
        let v = json!({
            "items":[
                {"slug":"acme/alpha","state":"populated"},
                {"slug":"acme/beta","state":"populated"}
            ],
            "page":{"next_cursor":null,"limit":50}
        });
        let out = render(&v, false);
        assert!(out.contains("acme/alpha [populated]"));
        assert!(out.contains("acme/beta [populated]"));
        assert!(!out.contains("more"), "no cursor → no more hint");
    }

    #[test]
    fn human_list_shows_more_hint_when_cursor_present() {
        let v = json!({"items":[{"slug":"a","state":"populated"}],"page":{"next_cursor":"2","limit":2}});
        assert!(render(&v, false).contains("more"));
    }

    #[test]
    fn repository_next_cursor_hint_round_trips_through_git_parser_and_dispatch() {
        let cursor = RepoListCursor::new([8; 32], "alpha").unwrap().encode();
        let rendered = render(
            &json!({
                "items": [{"slug": "acme/alpha", "state": "populated"}],
                "page": {"next_cursor": cursor, "limit": 1}
            }),
            false,
        );
        let command = rendered
            .lines()
            .find_map(|line| {
                line.strip_prefix("… (more - run: ")
                    .and_then(|line| line.strip_suffix(')'))
            })
            .expect("actionable next-page command");
        let words = command.split_whitespace().collect::<Vec<_>>();
        assert_eq!(&words[..3], &["myelin", "repo", "list"]);
        let call = crate::dispatch::repo_dispatch(&words[2..]).expect("hint parses and dispatches");
        assert_eq!(call.path, "/v1/git/repos");
        assert_eq!(
            call.query.as_deref(),
            Some(format!("view=summary&cursor={cursor}").as_str())
        );
    }

    #[test]
    fn human_whoami_is_a_single_line() {
        let v =
            json!({"principal_id":"svc:agent","kind":"service","tenant":"acme","region":"eu-west"});
        let out = render(&v, false);
        assert!(out.contains("svc:agent (service)"));
        assert!(out.contains("tenant=acme"));
    }

    #[test]
    fn human_search_hit_renders_repo_path_line() {
        let v = json!({"items":[{"repo":"myelin","path":"src/x.rs","line":3,"excerpt":"fn x"}],"page":{"next_cursor":null}});
        assert!(render(&v, false).contains("myelin:src/x.rs:3  fn x"));
    }

    #[test]
    fn unknown_shape_falls_back_to_json_no_panic() {
        let out = render(&json!({"weird":[1,2,3]}), false);
        assert!(out.contains("weird"));
        assert!(render(&json!({"items":[]}), false).contains("(no items)"));
    }

    #[test]
    fn pending_issue_receipt_never_claims_immediate_visibility() {
        let id = "33333333-3333-3333-3333-333333333333";
        let out = render(
            &json!({
                "issue": {"id": id, "key": "ENG-1", "project_id": "p"},
                "authorization": {"status": "pending", "request_event_id": "evt"}
            }),
            false,
        );
        assert!(out.contains("authorization=pending"));
        assert!(out.contains("not visible yet"));
        assert!(out.contains(&format!("myelin issue view {id}")));
        assert!(!out.contains("created successfully"));
    }

    #[test]
    fn issue_import_preview_and_resume_are_honest_human_summaries() {
        let job = "33333333-3333-3333-3333-333333333333";
        let preview = render(
            &json!({
                "import": {"job_id": job, "source": "jira", "mode": "dry_run"},
                "reconciliation": {"received": 2, "ready": 2, "lossy": 0, "dropped": 0},
                "losses": [],
            }),
            false,
        );
        assert_eq!(
            preview,
            format!(
                "Import preview {job} (jira): 2/2 ready, 0 lossy, 0 dropped; no data written.\n"
            )
        );

        let resumed = render(
            &json!({
                "import": {"job_id": job, "source": "jira", "mode": "run"},
                "summary": {"received": 1, "created": 0, "resumed": 1, "lossy": 0, "dropped": 0},
                "issues": [{
                    "source_id": "JIRA-41",
                    "created": false,
                    "issue": {"id": "44444444-4444-4444-4444-444444444444", "key": "ENG-41"},
                    "authorization": {"status": "requested"},
                }],
                "losses": [],
            }),
            false,
        );
        assert!(resumed.contains("0 created, 1 resumed"));
        assert!(resumed.contains("JIRA-41 -> ENG-41 staged"));
        assert!(resumed.contains("authorization=requested"));
    }

    #[test]
    fn issue_list_and_view_have_a_human_row() {
        let issue = json!({
            "id":"33333333-3333-3333-3333-333333333333",
            "key":"ENG-1",
            "title":"Founder issue",
            "state":"open"
        });
        let row = render(&issue, false);
        assert!(row.contains("ENG-1  [open]  Founder issue"));
        let list = render(&json!({"items":[issue],"page":{"next_cursor":null}}), false);
        assert!(list.contains("ENG-1  [open]  Founder issue"));
    }

    #[test]
    fn notification_list_show_and_read_receipt_are_human_scannable() {
        let item = json!({
            "id": "item-1",
            "reason": "review_requested",
            "class": "direct",
            "subject": "myelin://acme/git/pr/core:42",
            "state": "unread",
            "coalesce_count": 2
        });
        let row = render(&item, false);
        assert_eq!(
            row,
            "● review_requested ×2  myelin://acme/git/pr/core:42  (item-1)\n"
        );
        let list = render(
            &json!({"items": [item], "page": {"next_cursor": null, "limit": 50}}),
            false,
        );
        assert!(list.contains("● review_requested ×2"));
        assert_eq!(
            render(&json!({"id": "item-1", "state": "read"}), false),
            "item-1 [read]\n"
        );
    }

    #[test]
    fn human_issue_fields_escape_terminal_controls_but_preserve_printable_unicode() {
        let issue = json!({
            "id":"id\u{7f}tail",
            "key":"ENG\t1",
            "title":"Grüße 🚀\nsecond row\u{1b}[31mred\u{85}next",
            "state":"open\rclosed"
        });

        let out = render(&issue, false);
        assert_eq!(
            out.lines().count(),
            1,
            "untrusted fields cannot inject rows"
        );
        assert!(!out.contains('\u{1b}'));
        assert!(!out.contains('\r'));
        assert!(!out.contains('\t'));
        assert!(!out.contains('\u{7f}'));
        assert!(!out.contains('\u{85}'));
        assert!(out.contains("Grüße 🚀"), "printable Unicode survives");
        assert!(out.contains("\\nsecond row\\x1b[31mred\\x85next"));
        assert!(out.contains("ENG\\t1  [open\\rclosed]"));
        assert!(out.contains("id\\x7ftail"));

        let json = render(&issue, true);
        assert!(json.contains("Grüße 🚀"));
        assert!(json.contains("\\nsecond row"), "JSON keeps JSON escaping");
    }

    #[test]
    fn all_other_human_server_fields_are_terminal_safe() {
        let receipt = json!({
            "issue": {"id": "id\n2", "key": "ENG\u{1b}[2J"},
            "authorization": {"status": "pending\tunsafe"}
        });
        let receipt_out = render(&receipt, false);
        assert!(!receipt_out.contains('\u{1b}'));
        assert!(receipt_out.contains("id\\n2"));
        assert!(receipt_out.contains("ENG\\x1b[2J"));
        assert!(receipt_out.contains("pending\\tunsafe"));

        let page = json!({
            "items": [{"repo": "repo\nrow", "path": "p\u{1b}[A", "line": 1,
                       "excerpt": "Grüße\tthere"}],
            "page": {"next_cursor": null}
        });
        let page_out = render(&page, false);
        assert_eq!(page_out.lines().count(), 1);
        assert!(!page_out.contains('\u{1b}'));
        assert!(page_out.contains("repo\\nrow:p\\x1b[A:1  Grüße\\tthere"));

        let fallback_out = render(&json!({"unknown": "safe 🚀\u{7f}\u{85}"}), false);
        assert!(fallback_out.contains("safe 🚀\\x7f\\x85"));
        assert!(!fallback_out.contains('\u{7f}'));
        assert!(!fallback_out.contains('\u{85}'));
    }

    #[test]
    fn ci_list_has_human_state_words_and_an_actionable_opaque_cursor() {
        let cursor = canonical_ci_cursor();
        let value = json!({
            "items": [{
                "run_id": "91000000-0000-4000-8000-000000000001",
                "pipeline_id": "93000000-0000-4000-8000-000000000001",
                "repo_ref": "myelin://acme/git/repo/alpha",
                "commit_oid": "0123456789abcdef",
                "trigger_kind": "push",
                "trust_tier": "trusted",
                "state": "failed",
                "cost_settled": true,
                "created_at": "2026-07-24T12:00:00.000000Z",
                "finished_at": "2026-07-24T12:05:00.000000Z"
            }],
            "page": {"next_cursor": &cursor, "limit": 1}
        });
        let call =
            crate::dispatch::ci_dispatch(&["list", "--status", "failed", "--limit", "1"]).unwrap();
        let output = render_for_call(&value, false, &call);
        assert!(output.contains("✗ failed"));
        let command = output
            .lines()
            .find_map(|line| {
                line.strip_prefix("… (more - run: ")
                    .and_then(|line| line.strip_suffix(')'))
            })
            .expect("actionable next-page command");
        let words = command.split_whitespace().collect::<Vec<_>>();
        assert_eq!(&words[..4], &["myelin", "ci", "list", "--status"]);
        let next = crate::dispatch::ci_dispatch(&words[2..]).unwrap();
        assert_eq!(
            next.query.as_deref(),
            Some(format!("state=failed&limit=1&cursor={cursor}").as_str())
        );
    }

    #[test]
    fn ci_golden_detail_and_archive_render_without_live_claims_or_terminal_injection() {
        let contract: Value = serde_json::from_str(include_str!(
            "../../../contracts/ci-read-dev-edge.golden.json"
        ))
        .unwrap();
        let vector = |id: &str| {
            contract["vectors"]
                .as_array()
                .unwrap()
                .iter()
                .find(|vector| vector["id"] == id)
                .unwrap()["expected"]
                .clone()
        };

        let detail = render(&vector("failed-run-detail"), false);
        assert!(detail.contains("✗ failed"));
        assert!(detail.contains("test/contract"));
        assert!(detail.contains(
            "myelin ci logs 91000000-0000-4000-8000-000000000001 --job \
             92000000-0000-4000-8000-000000000001"
        ));
        assert!(!detail.contains("live"));

        let archive = render(&vector("archived-log-byte-range"), false);
        assert!(archive.contains("archived log bytes 9..16 of 18"));
        assert!(archive.contains("\\xa9\nfail"));
        assert!(!archive.contains('\u{a9}'));
        assert!(archive.contains("--start 16 --limit 7"));
        assert!(!archive.contains("watch"));
    }

    #[test]
    fn running_ci_job_surfaces_the_exact_scoped_watch_command() {
        let run = "91000000-0000-4000-8000-000000000002";
        let job = "92000000-0000-4000-8000-000000000002";
        let detail = json!({
            "run": {
                "run_id": run,
                "repo_ref": "myelin://acme/git/repo/alpha",
                "state": "running",
                "created_at": "now"
            },
            "jobs": [{
                "job_id": job,
                "stage": "test",
                "name": "contract",
                "state": "running",
                "attempt": 1
            }],
            "steps": []
        });
        let output = render(&detail, false);
        assert!(output.contains(&format!("live output: myelin ci watch {run} --job {job}")));
    }

    #[test]
    fn ci_action_hints_require_canonical_shell_safe_identifiers() {
        let detail = json!({
            "run": {
                "run_id": "run; unsafe",
                "repo_ref": "myelin://acme/git/repo/alpha",
                "state": "failed",
                "created_at": "now"
            },
            "jobs": [{
                "job_id": "job $(unsafe)",
                "stage": "test",
                "name": "contract",
                "state": "failed",
                "attempt": 1
            }],
            "steps": []
        });
        let output = render(&detail, false);
        assert!(!output.contains("archived output:"));

        let log = json!({
            "run_id": "run; unsafe",
            "job_id": "job $(unsafe)",
            "byte_start": 0,
            "byte_end": 1,
            "total_end": 2,
            "next_offset": 1,
            "encoding": "base64",
            "data": "eA=="
        });
        assert!(!render(&log, false).contains("more - run:"));
    }
}
