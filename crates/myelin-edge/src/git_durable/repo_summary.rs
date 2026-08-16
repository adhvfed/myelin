use crate::catalogue::{DEFAULT_PAGE_LIMIT, MAX_PAGE_LIMIT};
use crate::error::EdgeError;
use crate::request::EdgeResponse;
use myelin_git::durable::DurableError;
use myelin_git::web::{RepoListCursor, REPO_LIST_CURSOR_MAX_BYTES, REPO_LIST_CURSOR_PREFIX};
use serde_json::Value;

use super::map_durable_err;

pub(super) const REPO_LIST_RESPONSE_MAX_BYTES: usize = 1024 * 1024;
const REPO_LIST_QUERY_MAX_BYTES: usize = 16 * 1024;

pub(super) struct RepoListQuery {
    pub(super) limit: usize,
    pub(super) cursor: Option<String>,
}

pub(super) fn map_repo_list_durable_err(error: DurableError) -> EdgeError {
    match error {
        DurableError::Git(message) if message.starts_with("wire ref limit exceeded:") => {
            EdgeError::PayloadTooLarge(
                "repository catalogue exceeds the interactive list limit".into(),
            )
        }
        other => map_durable_err(other),
    }
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
                    "repository list query contains malformed percent encoding".into(),
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
        .map_err(|_| EdgeError::BadRequest("repository list query is not valid UTF-8".into()))?
        .into_owned();
    if decoded.chars().any(char::is_control) {
        return Err(EdgeError::BadRequest(
            "repository list query contains a control character".into(),
        ));
    }
    Ok(decoded)
}

pub(super) fn parse_repo_list_query(query: &str) -> Result<RepoListQuery, EdgeError> {
    if query.len() > REPO_LIST_QUERY_MAX_BYTES {
        return Err(EdgeError::BadRequest(
            "repository list query is too large".into(),
        ));
    }
    let mut limit = None;
    let mut cursor = None;
    if query.is_empty() {
        return Ok(RepoListQuery {
            limit: DEFAULT_PAGE_LIMIT,
            cursor: None,
        });
    }
    for pair in query.split('&') {
        let (raw_name, raw_value) = pair.split_once('=').ok_or_else(|| {
            EdgeError::BadRequest("malformed repository list query parameter".into())
        })?;
        let name = decode_query_component(raw_name)?;
        let value = decode_query_component(raw_value)?;
        let duplicate = |field: &str| {
            EdgeError::BadRequest(format!(
                "duplicate repository list query parameter `{field}`"
            ))
        };
        match name.as_str() {
            "limit" => {
                if limit.is_some() {
                    return Err(duplicate("limit"));
                }
                let parsed = value.parse::<usize>().ok().filter(|parsed| {
                    value == parsed.to_string() && (1..=MAX_PAGE_LIMIT).contains(parsed)
                });
                limit = Some(parsed.ok_or_else(|| {
                    EdgeError::BadRequest(format!(
                        "repository list limit must be canonical and within 1..={MAX_PAGE_LIMIT}"
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
                        "repository list cursor is malformed".into(),
                    ));
                }
                cursor = Some(value);
            }
            "" => {
                return Err(EdgeError::BadRequest(
                    "empty repository list query parameter name".into(),
                ));
            }
            _ => {
                return Err(EdgeError::BadRequest(format!(
                    "unknown repository list query parameter `{name}`"
                )));
            }
        }
    }
    Ok(RepoListQuery {
        limit: limit.unwrap_or(DEFAULT_PAGE_LIMIT),
        cursor,
    })
}

pub(super) fn repo_list_cursor_scope(tenant: &str, region: &str) -> [u8; 32] {
    let mut hash = blake3::Hasher::new();
    hash.update(b"myelin.edge.durable-repository-catalogue.v2\0");
    for value in [tenant, region] {
        hash.update(&(value.len() as u64).to_be_bytes());
        hash.update(value.as_bytes());
    }
    *hash.finalize().as_bytes()
}

pub(super) fn parse_repo_list_cursor(
    value: &str,
    tenant: &str,
    region: &str,
) -> Result<RepoListCursor, EdgeError> {
    let cursor = RepoListCursor::parse(value)
        .map_err(|_| EdgeError::BadRequest("repository list cursor is malformed".into()))?;
    if cursor.scope() != repo_list_cursor_scope(tenant, region) {
        return Err(EdgeError::BadRequest(
            "repository list cursor scope mismatch".into(),
        ));
    }
    Ok(cursor)
}

pub(super) fn repo_list_response(value: &Value) -> Result<EdgeResponse, EdgeError> {
    let body = serde_json::to_vec(value)
        .map_err(|error| EdgeError::Internal(format!("serialize repository list: {error}")))?;
    if body.len() > REPO_LIST_RESPONSE_MAX_BYTES {
        return Err(EdgeError::PayloadTooLarge(
            "repository list exceeds the response byte limit".into(),
        ));
    }
    Ok(EdgeResponse::Bytes {
        status: 200,
        content_type: "application/json".into(),
        headers: Vec::new(),
        body,
    })
}
