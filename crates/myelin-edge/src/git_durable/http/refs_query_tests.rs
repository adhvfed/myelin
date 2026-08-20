use super::*;

#[test]
fn refs_query_defaults_and_exact_decoding_are_stable() {
    assert_eq!(parse_refs_query("").unwrap(), RefsPageRequest::default());
    let parsed =
        parse_refs_query("limit=%37&q=Feature%2FOne&current=refs%2Fheads%2Fmain&cursor=gr1_abc")
            .expect("strict decoded query");
    assert_eq!(parsed.limit, 7);
    assert_eq!(parsed.query.as_deref(), Some("Feature/One"));
    assert_eq!(parsed.current_ref.as_deref(), Some("refs/heads/main"));
    assert_eq!(parsed.cursor.as_deref(), Some("gr1_abc"));
    assert_eq!(parse_refs_query("q=").unwrap().query.as_deref(), Some(""));
}

#[test]
fn refs_query_rejects_noncanonical_and_ambiguous_inputs() {
    for query in [
        "limit",
        "=1",
        "unknown=1",
        "q=a&q=b",
        "q=a&%71=b",
        "limit=",
        "cursor=",
        "current=",
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
        "current=main",
        "current=refs%2Fremotes%2Forigin%2Fmain",
        "q=a&&limit=1",
    ] {
        assert!(
            matches!(parse_refs_query(query), Err(EdgeError::BadRequest(_))),
            "query must fail closed: {query}"
        );
    }
}

#[test]
fn refs_query_component_and_total_byte_limits_are_exact() {
    parse_refs_query(&format!("q={}", "x".repeat(REFS_PAGE_MAX_QUERY_BYTES)))
        .expect("exact q bound");
    parse_refs_query(&format!("cursor={}", "x".repeat(REFS_MAX_CURSOR_BYTES)))
        .expect("exact cursor bound");
    let current = format!(
        "refs/heads/{}",
        "x".repeat(WIRE_MAX_REF_NAME_BYTES - "refs/heads/".len())
    );
    parse_refs_query(&format!("current={current}")).expect("exact current bound");

    for query in [
        format!("q={}", "x".repeat(REFS_PAGE_MAX_QUERY_BYTES + 1)),
        format!("cursor={}", "x".repeat(REFS_MAX_CURSOR_BYTES + 1)),
        format!("current={current}x"),
        "x".repeat(REFS_MAX_QUERY_BYTES + 1),
    ] {
        assert!(matches!(
            parse_refs_query(&query),
            Err(EdgeError::BadRequest(_))
        ));
    }
}

#[test]
fn refs_cursor_errors_map_to_scoped_statuses() {
    for error in [
        RefsPageError::MalformedCursor,
        RefsPageError::CursorScopeMismatch,
        RefsPageError::InvalidCurrentRef,
        RefsPageError::InvalidLimit { supplied: 0 },
    ] {
        assert_eq!(map_refs_page_err(error).status(), 400);
    }
    assert_eq!(map_refs_page_err(RefsPageError::CursorStale).status(), 409);
    assert_eq!(
        map_refs_page_err(RefsPageError::Durable(DurableError::NotFound(
            "missing".into()
        )))
        .status(),
        404
    );
}
