use super::*;

#[test]
fn tree_query_defaults_and_exact_decoding_are_stable() {
    assert_eq!(parse_tree_query("").unwrap(), TreePageRequest::default());
    let parsed =
        parse_tree_query("limit=%37&q=Readme+File&cursor=gt1_abc").expect("strict decoded query");
    assert_eq!(parsed.limit, 7);
    assert_eq!(parsed.query.as_deref(), Some("Readme File"));
    assert_eq!(parsed.cursor.as_deref(), Some("gt1_abc"));
    assert_eq!(parse_tree_query("q=").unwrap().query.as_deref(), Some(""));
}

#[test]
fn tree_query_rejects_noncanonical_ambiguous_and_unknown_inputs() {
    for query in [
        "limit",
        "=1",
        "unknown=1",
        "q=a&q=b",
        "q=a&%71=b",
        "limit=",
        "cursor=",
        "limit=01",
        "limit=+1",
        "limit=%2B1",
        "limit=0",
        "limit=101",
        "limit=-1",
        "limit=1.0",
        "q=%00",
        "q=%FF",
        "q=%",
        "q=a&&limit=1",
    ] {
        assert!(
            matches!(parse_tree_query(query), Err(EdgeError::BadRequest(_))),
            "query must fail closed: {query}"
        );
    }
}

#[test]
fn tree_query_component_and_total_byte_limits_are_exact() {
    parse_tree_query(&format!("q={}", "x".repeat(TREE_PAGE_MAX_QUERY_BYTES)))
        .expect("exact q bound");
    parse_tree_query(&format!("cursor={}", "x".repeat(TREE_MAX_CURSOR_BYTES)))
        .expect("exact cursor bound");
    for query in [
        format!("q={}", "x".repeat(TREE_PAGE_MAX_QUERY_BYTES + 1)),
        format!("cursor={}", "x".repeat(TREE_MAX_CURSOR_BYTES + 1)),
        "x".repeat(TREE_MAX_QUERY_BYTES + 1),
    ] {
        assert!(matches!(
            parse_tree_query(&query),
            Err(EdgeError::BadRequest(_))
        ));
    }
}

#[test]
fn tree_cursor_errors_map_to_scoped_statuses() {
    for error in [
        TreePageError::MalformedCursor,
        TreePageError::CursorScopeMismatch,
        TreePageError::InvalidQuery,
        TreePageError::InvalidLimit { supplied: 0 },
    ] {
        assert_eq!(map_tree_page_err(error).status(), 400);
    }
    assert_eq!(map_tree_page_err(TreePageError::CursorStale).status(), 409);
    assert_eq!(
        map_tree_page_err(TreePageError::Durable(DurableError::NotFound(
            "missing".into()
        )))
        .status(),
        404
    );
}

#[test]
fn tree_capacity_errors_are_sanitized_payload_too_large_responses() {
    for private in [
        "tree page limit exceeded: tree object is larger than 8388608 bytes",
        "tree page limit exceeded: scanned entry count",
        "tree page limit exceeded: one entry name",
        "tree page limit exceeded: name bytes",
    ] {
        let mapped = map_tree_page_err(TreePageError::Durable(DurableError::Git(private.into())));
        assert_eq!(mapped.status(), 413);
        assert_eq!(
            mapped.to_string(),
            "413 (payload_too_large): repository tree exceeds the interactive browse limit"
        );
    }
}

#[test]
fn non_capacity_tree_errors_keep_their_existing_classification() {
    assert_eq!(
        map_tree_page_err(TreePageError::Durable(DurableError::Git(
            "tree path segment is invalid".into(),
        )))
        .status(),
        400
    );
    assert_eq!(
        map_tree_page_err(TreePageError::Durable(DurableError::Git(
            "tree object has the wrong kind".into(),
        )))
        .status(),
        500
    );
}
