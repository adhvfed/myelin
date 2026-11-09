//! Pull requests in the terminal: one readable view, and one line per pull request in a list.

use myelin_git::pr_store::PrStateChange;
use serde_json::Value;

use crate::dispatch::EdgeCall;

use super::terminal_safe_single_line;

fn text<'a>(value: &'a Value, field: &str) -> Option<&'a str> {
    value.get(field).and_then(Value::as_str)
}

fn short_ref(reference: &str) -> String {
    terminal_safe_single_line(reference.strip_prefix("refs/heads/").unwrap_or(reference))
}

fn short_oid(oid: &str) -> String {
    terminal_safe_single_line(oid.get(..12).unwrap_or(oid))
}

fn review(value: &Value) -> Option<&'static str> {
    match text(value, "review_state")? {
        "approved" => Some("approved"),
        "changes" => Some("changes requested"),
        "requested" => Some("review requested"),
        _ => None,
    }
}

fn headline(value: &Value) -> Option<String> {
    let number = value.get("number")?.as_u64()?;
    let state = terminal_safe_single_line(text(value, "pr_state")?);
    let title = text(value, "title")
        .map(terminal_safe_single_line)
        .unwrap_or_else(|| "(untitled)".into());
    Some(format!("#{number} {title} [{state}]"))
}

/// The repository named by a `/v1/git/repos/{repo}/prs/{n}` call, for follow-up hints.
fn viewed_repository(call: Option<&EdgeCall>) -> Option<String> {
    let path = call?.path.strip_prefix("/v1/git/repos/")?;
    let (repo, rest) = path.split_once("/prs/")?;
    (!rest.contains('/') && rest.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| terminal_safe_single_line(repo))
}

pub(super) fn render_view(value: &Value, call: Option<&EdgeCall>) -> Option<String> {
    let repo = viewed_repository(call)?;
    let mut out = format!("{}\n", headline(value)?);
    let number = value.get("number")?.as_u64()?;
    let base = short_ref(text(value, "base_ref")?);
    let mut line = format!(
        "  {} → {base} · head {}",
        short_ref(text(value, "head_ref")?),
        short_oid(text(value, "head_oid")?)
    );
    if let Some(count) = value.get("commits_count").and_then(Value::as_u64) {
        let more = value.get("commits_count_capped").and_then(Value::as_bool) == Some(true);
        let plural = if count == 1 && !more { "" } else { "s" };
        line.push_str(&format!(
            " · {count}{} commit{plural}",
            if more { "+" } else { "" }
        ));
    }
    if value.get("is_fork").and_then(Value::as_bool) == Some(true) {
        line.push_str(" · from a fork");
    }
    if let Some(review) = review(value) {
        line.push_str(&format!(" · {review}"));
    }
    out.push_str(&line);
    out.push('\n');
    if value.get("behind_base").and_then(Value::as_bool) == Some(true) {
        out.push_str(&format!(
            "  {base} has moved on; update the branch to merge: myelin --idempotency-key <key> \
             repo pr update {repo} {number}\n"
        ));
    }
    if let Some(queued) = value.get("auto_merge").filter(|queued| queued.is_object()) {
        let place = match queued.get("ahead").and_then(Value::as_u64) {
            Some(0) => "next in line".to_string(),
            Some(ahead) => format!("{ahead} scheduled earlier"),
            None => "queued".to_string(),
        };
        let by = text(queued, "requested_by")
            .map(terminal_safe_single_line)
            .unwrap_or_default();
        out.push_str(&format!(
            "  merging when ready · {place} for {base} · asked by {by}\n"
        ));
    }
    if let Some(reason) = value
        .get("auto_merge_stopped")
        .and_then(|stopped| text(stopped, "reason"))
    {
        out.push_str(&format!(
            "  automatic merge stopped: {}\n",
            terminal_safe_single_line(reason)
        ));
    }
    if let Some(merged) = text(value, "merged_oid") {
        out.push_str(&format!("  merged as {}\n", short_oid(merged)));
    }
    if let Some(closer) = text(value, "closed_by") {
        out.push_str(&format!(
            "  closed by {}\n",
            terminal_safe_single_line(closer)
        ));
    }
    if text(value, "pr_state") == Some("draft") {
        out.push_str(&format!(
            "  a draft; when it is ready for review: myelin --idempotency-key <key> repo pr ready \
             {repo} {number}\n"
        ));
    }
    Some(out)
}

/// The receipt for closing, reopening, or marking a pull request ready for review.
pub(super) fn render_state_change(path: &str, action: &str, pr: &Value) -> Option<String> {
    let change = PrStateChange::ALL
        .into_iter()
        .find(|change| change.action() == action)?;
    if !path.ends_with(&format!("/{}", change.verb())) {
        return None;
    }
    let number = pr.get("number")?.as_u64()?;
    let title = text(pr, "title")
        .map(terminal_safe_single_line)
        .unwrap_or_else(|| "(untitled)".into());
    Some(match change {
        PrStateChange::Close => format!("closed #{number} {title}\n"),
        PrStateChange::Reopen => format!("reopened #{number} {title}\n"),
        PrStateChange::MarkReady => format!("#{number} {title} is ready for review\n"),
    })
}

/// One pull request in a list.
pub(super) fn render_item(item: &Value) -> Option<String> {
    let mut line = headline(item)?;
    line.push_str(&format!(
        "  {} → {}",
        short_ref(text(item, "head_ref")?),
        short_ref(text(item, "base_ref")?)
    ));
    if let Some(repo) = text(item, "repo") {
        line = format!("{} {line}", terminal_safe_single_line(repo));
    }
    if let Some(review) = review(item) {
        line.push_str(&format!(" · {review}"));
    }
    if let Some(checks) = item.get("checks_summary") {
        if let (Some(verdict), Some(passing), Some(total)) = (
            text(checks, "verdict"),
            checks.get("passing").and_then(Value::as_u64),
            checks.get("total").and_then(Value::as_u64),
        ) {
            if total > 0 {
                line.push_str(&format!(
                    " · checks {} ({passing}/{total})",
                    terminal_safe_single_line(verdict)
                ));
            }
        }
    }
    Some(line)
}
