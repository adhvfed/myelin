use myelin_issues::{CanonicalKey, HiLoKeyAllocator, InMemoryPrefixCounter};
use myelin_tenancy::TenantId;

fn tenant() -> TenantId {
    TenantId("acme".into())
}

fn provider_mints_issue_ref(prefix: &str) -> (CanonicalKey, String) {
    let allocator = HiLoKeyAllocator::new(InMemoryPrefixCounter::new());
    let key = allocator.allocate(&tenant(), prefix).expect("allocate");
    let r = key.issue_artifact_ref(&tenant());
    (key, r.0)
}

fn consumer_round_trips(stored: &str) -> String {
    let r = myelin_refs::parse(stored).expect("the consumer admits the stored canonical issue id");
    myelin_refs::format(&r)
}

#[test]
fn provider_mints_projectkey_seqno_consumer_round_trips_byte_identical() {
    let (key, stored) = provider_mints_issue_ref("ENG");
    assert_eq!(key.render(), "ENG-1");
    assert_eq!(stored, "myelin://acme/issue/issue/ENG-1");
    assert_eq!(
        consumer_round_trips(&stored),
        stored,
        "format(parse(s)) == s - the stored canonical key round-trips"
    );
}

#[test]
fn render_time_display_form_is_rejected_as_a_scope_ref_3() {
    let (key, _) = provider_mints_issue_ref("OPS");
    let display = key.render_display_key();
    assert_eq!(display, "#1");
    assert!(
        myelin_refs::parse(&display).is_err(),
        "the #<seqno> display projection is render-time only - the consumer rejects it as a scope"
    );
    assert!(
        myelin_refs::parse(&key.render()).is_err(),
        "the bare <PROJECTKEY>-<seqno> is an <id> segment, not a standalone scope"
    );
}

#[test]
fn distinct_prefixes_mint_distinct_canonical_urns() {
    let allocator = HiLoKeyAllocator::new(InMemoryPrefixCounter::new());
    let eng = allocator.allocate(&tenant(), "ENG").unwrap();
    let ops = allocator.allocate(&tenant(), "OPS").unwrap();
    assert_ne!(eng.render(), ops.render(), "ENG-1 != OPS-1");
    assert_eq!(
        eng.issue_artifact_ref(&tenant()).0,
        "myelin://acme/issue/issue/ENG-1"
    );
    assert_eq!(
        ops.issue_artifact_ref(&tenant()).0,
        "myelin://acme/issue/issue/OPS-1"
    );
}
