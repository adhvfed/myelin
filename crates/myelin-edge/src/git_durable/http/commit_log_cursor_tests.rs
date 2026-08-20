use super::*;

#[test]
fn commit_log_cursor_is_strict_and_bounded() {
    let initial = Page::parse("", "commit log").unwrap();
    assert_eq!(
        initial.offset(COMMIT_LOG_MAX_OFFSET, "commit-log").unwrap(),
        0
    );
    let boundary = Page::parse(&format!("cursor={COMMIT_LOG_MAX_OFFSET}"), "commit log").unwrap();
    assert_eq!(
        boundary
            .offset(COMMIT_LOG_MAX_OFFSET, "commit-log")
            .unwrap(),
        COMMIT_LOG_MAX_OFFSET
    );
    let maximum = usize::MAX.to_string();
    for cursor in ["01", "-1", "1.5", "not-a-cursor", maximum.as_str()] {
        let page = Page::parse(&format!("cursor={cursor}"), "commit log").unwrap();
        assert!(matches!(
            page.offset(COMMIT_LOG_MAX_OFFSET, "commit-log"),
            Err(EdgeError::BadRequest(message)) if message == "invalid commit-log cursor"
        ));
    }
}
