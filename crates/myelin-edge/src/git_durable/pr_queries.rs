use super::*;

pub(super) struct EnrichedPr {
    pub(super) rec: PrRecord,
    pub(super) summary: ChecksSummary,
    pub(super) you_requested: bool,
    pub(super) repo_slug: Option<String>,
}

pub(super) struct EnrichedPrSlice {
    pub(super) rows: Vec<EnrichedPr>,
    pub(super) counts: PrListCounts,
    pub(super) total: usize,
    pub(super) offset: usize,
    pub(super) limit: usize,
    pub(super) next_cursor: Option<String>,
    pub(super) prev_cursor: Option<String>,
}

pub(super) struct EnrichedCrossPrSlice {
    pub(super) rows: Vec<EnrichedPr>,
    pub(super) total: usize,
    pub(super) offset: usize,
    pub(super) limit: usize,
    pub(super) next_cursor: Option<String>,
    pub(super) prev_cursor: Option<String>,
}

#[derive(Clone, Copy)]
pub(super) struct CrossPrListLimits {
    pub(super) maximum_records: usize,
    pub(super) maximum_bytes: usize,
}

impl CrossPrListLimits {
    pub(super) const fn production() -> Self {
        Self {
            maximum_records: CROSS_PR_LIST_MAX_RECORDS,
            maximum_bytes: CROSS_PR_LIST_MAX_BYTES,
        }
    }
}

pub(super) const PR_LIST_QUERY_MAX_BYTES: usize = 16 * 1024;

pub(super) enum ParsedPrListCursor {
    Legacy(usize),
    Keyset(PrListCursor),
}

pub(super) fn parse_pr_list_cursor(value: &str) -> Result<ParsedPrListCursor, EdgeError> {
    if let Ok(parsed) = value.parse::<usize>() {
        if value == parsed.to_string() && parsed <= PR_LIST_OFFSET_MAX {
            return Ok(ParsedPrListCursor::Legacy(parsed));
        }
    }
    if value.starts_with(PR_LIST_CURSOR_PREFIX) {
        return PrListCursor::parse(value)
            .map(ParsedPrListCursor::Keyset)
            .map_err(|_| EdgeError::BadRequest("invalid pull request cursor".into()));
    }
    Err(EdgeError::BadRequest("invalid pull request cursor".into()))
}

pub(super) fn decode_pr_list_query_component(raw: &str) -> Result<String, EdgeError> {
    let bytes = raw.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len()
                || !bytes[index + 1].is_ascii_hexdigit()
                || !bytes[index + 2].is_ascii_hexdigit()
            {
                return Err(EdgeError::BadRequest(
                    "pull request list query contains malformed percent encoding".into(),
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
        .map_err(|_| EdgeError::BadRequest("pull request list query is not valid UTF-8".into()))?
        .into_owned();
    if decoded.chars().any(char::is_control) {
        return Err(EdgeError::BadRequest(
            "pull request list query contains a control character".into(),
        ));
    }
    Ok(decoded)
}

pub(super) fn repo_pr_list_query(
    ctx: &HandlerCtx<'_>,
    viewer_pseudonym: String,
    repo_slug: &str,
) -> Result<PrListQuery, EdgeError> {
    if ctx.request.query.len() > PR_LIST_QUERY_MAX_BYTES {
        return Err(EdgeError::BadRequest(
            "pull request list query is too large".into(),
        ));
    }
    let mut state = None;
    let mut sort = None;
    let mut cursor = None;
    let mut limit = None;
    if !ctx.request.query.is_empty() {
        for pair in ctx.request.query.split('&') {
            let (raw_name, raw_value) = pair.split_once('=').ok_or_else(|| {
                EdgeError::BadRequest("pull request list query is malformed".into())
            })?;
            let name = decode_pr_list_query_component(raw_name)?;
            let value = decode_pr_list_query_component(raw_value)?;
            let duplicate = |field: &str| {
                EdgeError::BadRequest(format!(
                    "duplicate pull request list query parameter `{field}`"
                ))
            };
            match name.as_str() {
                "state" => {
                    if state.is_some() {
                        return Err(duplicate("state"));
                    }
                    state = Some(PrListState::parse(&value).ok_or_else(|| {
                        EdgeError::BadRequest("invalid pull request state filter".into())
                    })?);
                }
                "sort" => {
                    if sort.is_some() {
                        return Err(duplicate("sort"));
                    }
                    sort = Some(PrListSort::parse(&value).ok_or_else(|| {
                        EdgeError::BadRequest("invalid pull request sort".into())
                    })?);
                }
                "cursor" => {
                    if cursor.is_some() {
                        return Err(duplicate("cursor"));
                    }
                    cursor = Some(parse_pr_list_cursor(&value)?);
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
                            "pull request list limit must be canonical and within 1..={MAX_PAGE_LIMIT}"
                        ))
                    })?);
                }
                _ => {
                    return Err(EdgeError::BadRequest(format!(
                        "unknown pull request list query parameter `{name}`"
                    )))
                }
            }
        }
    }
    let cursor_fields = match &cursor {
        Some(ParsedPrListCursor::Keyset(cursor)) => {
            let PrListCursorEndpoint::Repository(cursor_state) = cursor.endpoint() else {
                return Err(EdgeError::BadRequest(
                    "pull request list cursor scope mismatch".into(),
                ));
            };
            Some((cursor_state, cursor.sort(), cursor.limit()))
        }
        _ => None,
    };
    let effective_state =
        cursor_fields.map_or(state.unwrap_or(PrListState::Open), |fields| fields.0);
    let effective_sort =
        cursor_fields.map_or(sort.unwrap_or(PrListSort::Updated), |fields| fields.1);
    let effective_limit =
        cursor_fields.map_or(limit.unwrap_or(DEFAULT_PAGE_LIMIT), |fields| fields.2);
    if state.is_some_and(|value| value != effective_state)
        || sort.is_some_and(|value| value != effective_sort)
        || limit.is_some_and(|value| value != effective_limit)
    {
        return Err(EdgeError::BadRequest(
            "pull request list cursor scope mismatch".into(),
        ));
    }
    let page = match cursor {
        None => PrListPage::Initial,
        Some(ParsedPrListCursor::Legacy(offset)) => PrListPage::LegacyOffset(offset),
        Some(ParsedPrListCursor::Keyset(cursor)) => {
            let expected = pr_list_static_scope(
                tenant_of(ctx),
                region_of(ctx),
                &viewer_pseudonym,
                PrListCursorEndpoint::Repository(effective_state),
                Some(repo_slug),
                effective_sort,
                effective_limit,
            );
            if cursor.static_scope() != expected {
                return Err(EdgeError::BadRequest(
                    "pull request list cursor scope mismatch".into(),
                ));
            }
            PrListPage::Keyset(cursor)
        }
    };
    PrListQuery::from_page(
        effective_state,
        effective_sort,
        page,
        effective_limit,
        viewer_pseudonym,
    )
    .map_err(|_| EdgeError::BadRequest("invalid pull request page".into()))
}

pub(super) fn cross_pr_list_query(
    ctx: &HandlerCtx<'_>,
    viewer_pseudonym: String,
) -> Result<PrCrossListQuery, EdgeError> {
    if ctx.request.query.len() > PR_LIST_QUERY_MAX_BYTES {
        return Err(EdgeError::BadRequest(
            "pull request list query is too large".into(),
        ));
    }
    let mut bucket = None;
    let mut sort = None;
    let mut cursor = None;
    let mut limit = None;
    if !ctx.request.query.is_empty() {
        for pair in ctx.request.query.split('&') {
            let (raw_name, raw_value) = pair.split_once('=').ok_or_else(|| {
                EdgeError::BadRequest("pull request list query is malformed".into())
            })?;
            let name = decode_pr_list_query_component(raw_name)?;
            let value = decode_pr_list_query_component(raw_value)?;
            let duplicate = |field: &str| {
                EdgeError::BadRequest(format!(
                    "duplicate pull request list query parameter `{field}`"
                ))
            };
            match name.as_str() {
                "bucket" => {
                    if bucket.is_some() {
                        return Err(duplicate("bucket"));
                    }
                    bucket = Some(PrListBucket::parse(&value).ok_or_else(|| {
                        EdgeError::BadRequest("invalid pull request bucket".into())
                    })?);
                }
                "sort" => {
                    if sort.is_some() {
                        return Err(duplicate("sort"));
                    }
                    sort = Some(PrListSort::parse(&value).ok_or_else(|| {
                        EdgeError::BadRequest("invalid pull request sort".into())
                    })?);
                }
                "cursor" => {
                    if cursor.is_some() {
                        return Err(duplicate("cursor"));
                    }
                    cursor = Some(parse_pr_list_cursor(&value)?);
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
                            "pull request list limit must be canonical and within 1..={MAX_PAGE_LIMIT}"
                        ))
                    })?);
                }
                _ => {
                    return Err(EdgeError::BadRequest(format!(
                        "unknown pull request list query parameter `{name}`"
                    )))
                }
            }
        }
    }
    let cursor_fields = match &cursor {
        Some(ParsedPrListCursor::Keyset(cursor)) => {
            let PrListCursorEndpoint::CrossRepository(cursor_bucket) = cursor.endpoint() else {
                return Err(EdgeError::BadRequest(
                    "pull request list cursor scope mismatch".into(),
                ));
            };
            Some((cursor_bucket, cursor.sort(), cursor.limit()))
        }
        _ => None,
    };
    let effective_bucket = cursor_fields
        .map_or(bucket.unwrap_or(PrListBucket::NeedsReview), |fields| {
            fields.0
        });
    let effective_sort =
        cursor_fields.map_or(sort.unwrap_or(PrListSort::Updated), |fields| fields.1);
    let effective_limit =
        cursor_fields.map_or(limit.unwrap_or(DEFAULT_PAGE_LIMIT), |fields| fields.2);
    if bucket.is_some_and(|value| value != effective_bucket)
        || sort.is_some_and(|value| value != effective_sort)
        || limit.is_some_and(|value| value != effective_limit)
    {
        return Err(EdgeError::BadRequest(
            "pull request list cursor scope mismatch".into(),
        ));
    }
    let page = match cursor {
        None => PrListPage::Initial,
        Some(ParsedPrListCursor::Legacy(offset)) => PrListPage::LegacyOffset(offset),
        Some(ParsedPrListCursor::Keyset(cursor)) => {
            let expected = pr_list_static_scope(
                tenant_of(ctx),
                region_of(ctx),
                &viewer_pseudonym,
                PrListCursorEndpoint::CrossRepository(effective_bucket),
                None,
                effective_sort,
                effective_limit,
            );
            if cursor.static_scope() != expected {
                return Err(EdgeError::BadRequest(
                    "pull request list cursor scope mismatch".into(),
                ));
            }
            PrListPage::Keyset(cursor)
        }
    };
    PrCrossListQuery::from_page(
        effective_bucket,
        effective_sort,
        page,
        effective_limit,
        viewer_pseudonym,
    )
    .map_err(|_| EdgeError::BadRequest("invalid pull request page".into()))
}
