use crate::catalogue::{DEFAULT_PAGE_LIMIT, MAX_PAGE_LIMIT};
use crate::error::EdgeError;
use crate::request::EdgeResponse;
use myelin_git::durable::DurableError;
use myelin_git::web::{RepoListCursor, REPO_LIST_CURSOR_MAX_BYTES, REPO_LIST_CURSOR_PREFIX};
use serde_json::Value;

use super::map_durable_err;

pub(super) const REPO_SUMMARY_RESPONSE_MAX_BYTES: usize = 1024 * 1024;
const REPO_SUMMARY_QUERY_MAX_BYTES: usize = 16 * 1024;

pub(super) struct RepoSummaryQuery {
    pub(super) limit: usize,
    pub(super) cursor: Option<String>,
}

pub(super) fn map_repo_summary_durable_err(error: DurableError) -> EdgeError {
    match error {
        DurableError::Git(message) if message.starts_with("wire ref limit exceeded:") => {
            EdgeError::PayloadTooLarge(
                "repository catalogue exceeds the interactive list limit".into(),
            )
        }
        other => map_durable_err(other),
    }
}

pub(super) fn repo_summary_requested(query: &str) -> bool {
    query.split('&').any(|pair| {
        let raw_name = pair.split_once('=').map_or(pair, |(name, _)| name);
        let form_name = raw_name.replace('+', " ");
        percent_encoding::percent_decode_str(&form_name)
            .decode_utf8()
            .is_ok_and(|name| name == "view")
    })
}

fn decode_query_component(raw: &str) -> Result<String, EdgeError> {
    let bytes = raw.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len()
                || !bytes[index + 1].is_ascii_hexdigit()
                || !bytes[index + 2].is_ascii_hexdigit()
            {
                return Err(EdgeError::BadRequest(
                    "repository summary query contains malformed percent encoding".into(),
                ));
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    let form_value = raw.replace('+', " ");
    let decoded = percent_encoding::percent_decode_str(&form_value)
        .decode_utf8()
        .map_err(|_| EdgeError::BadRequest("repository summary query is not valid UTF-8".into()))?
        .into_owned();
    if decoded.chars().any(char::is_control) {
        return Err(EdgeError::BadRequest(
            "repository summary query contains a control character".into(),
        ));
    }
    Ok(decoded)
}

pub(super) fn parse_repo_summary_query(query: &str) -> Result<RepoSummaryQuery, EdgeError> {
    if query.len() > REPO_SUMMARY_QUERY_MAX_BYTES {
        return Err(EdgeError::BadRequest(
            "repository summary query is too large".into(),
        ));
    }
    let mut view = None;
    let mut limit = None;
    let mut cursor = None;
    for pair in query.split('&') {
        let (raw_name, raw_value) = pair.split_once('=').ok_or_else(|| {
            EdgeError::BadRequest("malformed repository summary query parameter".into())
        })?;
        let name = decode_query_component(raw_name)?;
        let value = decode_query_component(raw_value)?;
        let duplicate = |field: &str| {
            EdgeError::BadRequest(format!(
                "duplicate repository summary query parameter `{field}`"
            ))
        };
        match name.as_str() {
            "view" => {
                if view.is_some() {
                    return Err(duplicate("view"));
                }
                if value != "summary" {
                    return Err(EdgeError::BadRequest(
                        "repository list view must be `summary`".into(),
                    ));
                }
                view = Some(());
            }
            "limit" => {
                if limit.is_some() {
                    return Err(duplicate("limit"));
                }
                let parsed = value.parse::<usize>().ok().filter(|parsed| {
                    value == parsed.to_string() && (1..=MAX_PAGE_LIMIT).contains(parsed)
                });
                limit = Some(parsed.ok_or_else(|| {
                    EdgeError::BadRequest(format!(
                        "repository summary limit must be canonical and within 1..={MAX_PAGE_LIMIT}"
                    ))
                })?);
            }
            "cursor" => {
                if cursor.is_some() {
                    return Err(duplicate("cursor"));
                }
                if !value.starts_with(REPO_LIST_CURSOR_PREFIX)
                    || value.len() > REPO_LIST_CURSOR_MAX_BYTES
                {
                    return Err(EdgeError::BadRequest(
                        "repository summary cursor is malformed".into(),
                    ));
                }
                cursor = Some(value);
            }
            "" => {
                return Err(EdgeError::BadRequest(
                    "empty repository summary query parameter name".into(),
                ));
            }
            _ => {
                return Err(EdgeError::BadRequest(format!(
                    "unknown repository summary query parameter `{name}`"
                )));
            }
        }
    }
    if view.is_none() {
        return Err(EdgeError::BadRequest(
            "repository summary query requires `view=summary`".into(),
        ));
    }
    Ok(RepoSummaryQuery {
        limit: limit.unwrap_or(DEFAULT_PAGE_LIMIT),
        cursor,
    })
}

pub(super) fn repo_summary_cursor_scope(tenant: &str, region: &str) -> [u8; 32] {
    let mut hash = blake3::Hasher::new();
    hash.update(b"myelin.edge.durable-repository-catalogue.v1\0");
    for value in [tenant, region] {
        hash.update(&(value.len() as u64).to_be_bytes());
        hash.update(value.as_bytes());
    }
    *hash.finalize().as_bytes()
}

pub(super) fn parse_repo_summary_cursor(
    value: &str,
    tenant: &str,
    region: &str,
) -> Result<RepoListCursor, EdgeError> {
    let cursor = RepoListCursor::parse(value)
        .map_err(|_| EdgeError::BadRequest("repository summary cursor is malformed".into()))?;
    if cursor.scope() != repo_summary_cursor_scope(tenant, region) {
        return Err(EdgeError::BadRequest(
            "repository summary cursor scope mismatch".into(),
        ));
    }
    Ok(cursor)
}

pub(super) fn repo_summary_response(value: &Value) -> Result<EdgeResponse, EdgeError> {
    let body = serde_json::to_vec(value)
        .map_err(|error| EdgeError::Internal(format!("serialize repository summary: {error}")))?;
    if body.len() > REPO_SUMMARY_RESPONSE_MAX_BYTES {
        return Err(EdgeError::PayloadTooLarge(
            "repository summary exceeds the response byte limit".into(),
        ));
    }
    Ok(EdgeResponse::Bytes {
        status: 200,
        content_type: "application/json".into(),
        headers: Vec::new(),
        body,
    })
}
