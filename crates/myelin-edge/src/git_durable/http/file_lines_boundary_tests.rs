use super::*;

#[test]
fn file_lines_query_is_exact_decoded_and_bounded() {
    let parsed = parse_file_lines_query("path=src%2Fmain+file.rs&start=2&end=4")
        .expect("canonical bounded query");
    assert_eq!(parsed.path, "src/main file.rs");
    assert_eq!((parsed.start, parsed.end), (2, 4));

    let exact_end = 17 + FILE_LINES_MAX_RANGE - 1;
    let exact = parse_file_lines_query(&format!("path=x&start=17&end={exact_end}"))
        .expect("the exact line-range cap must remain valid");
    assert_eq!((exact.start, exact.end), (17, exact_end));

    for query in [
        "",
        "path=x&start=1",
        "path=x&start=1&end=1&extra=x",
        "path=x&path=y&start=1&end=1",
        "path=..%2Fsecret&start=1&end=1",
        "path=x&start=0&end=1",
        "path=x&start=01&end=1",
        "path=x&start=1&end=+1",
        "path=x&start=2&end=1",
        &format!("path=x&start=1&end={}", FILE_LINES_MAX_RANGE + 1),
        "path=x%ZZ&start=1&end=1",
    ] {
        assert!(
            matches!(parse_file_lines_query(query), Err(EdgeError::BadRequest(_))),
            "query must fail closed: {query}"
        );
    }
    assert!(matches!(
        parse_file_lines_query(&"x".repeat(FILE_LINES_MAX_QUERY_BYTES + 1)),
        Err(EdgeError::BadRequest(_))
    ));
}

#[test]
fn file_lines_oid_requires_the_full_lowercase_content_address() {
    assert!(canonical_blob_oid(
        "0123456789abcdef0123456789abcdef01234567"
    ));
    assert!(!canonical_blob_oid("01234567"));
    assert!(!canonical_blob_oid(
        "0123456789ABCDEF0123456789ABCDEF01234567"
    ));
    assert!(!canonical_blob_oid(
        "g123456789abcdef0123456789abcdef01234567"
    ));
}
