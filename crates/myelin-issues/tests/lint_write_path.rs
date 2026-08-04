use myelin_lints::lints::no_raw_publish;

#[test]
fn no_raw_publish_is_green_over_the_write_path_source() {
    let src = include_str!("../src/write_path.rs");
    let violations = no_raw_publish().run(src);
    assert!(
        violations.is_empty(),
        "no-raw-publish MUST be GREEN over the Issues write path (emit is the only path): {violations:?}"
    );
    assert!(
        violations.iter().all(|v| v.lint.0 == "no-raw-publish"),
        "every violation (none expected) carries the no-raw-publish id"
    );
}

#[test]
fn no_raw_publish_rejects_a_raw_published_issue_write() {
    let red = "\
fn create_issue_BAD(bus: &Bus, ev: IssueEvent) {
    // BUG: a fire-and-forget publish OUTSIDE the outbox transaction - the event can be lost if the
    // state commit fails, or ghost-published if it never committed. This is exactly what ISS-P06
    // forbids.
    bus.publish(ev);
}";
    let violations = no_raw_publish().run(red);
    assert!(
        !violations.is_empty(),
        "no-raw-publish MUST reject a raw bus.publish( in an Issues write path (the lost-event bug)"
    );
    assert!(
        violations.iter().all(|v| v.lint.0 == "no-raw-publish"),
        "every violation carries the no-raw-publish id (no false attribution)"
    );
}

#[test]
fn residency_pin_is_green_over_the_write_path_source() {
    use myelin_lints::lints::residency_pin;
    let src = include_str!("../src/write_path.rs");
    assert!(
        residency_pin().run(src).is_empty(),
        "residency-pin MUST be GREEN over the Issues write-path source (no request-derived region write)"
    );
}
