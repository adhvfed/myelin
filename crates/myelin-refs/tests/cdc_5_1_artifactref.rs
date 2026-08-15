use myelin_refs::{format, parse, strip_sub, sub_kind, ArtifactRef, ParseError, Sub};

fn provider_mints_canonical_urn(raw: &str) -> String {
    let r = parse(raw).expect("a provider only mints a well-formed, fully-scoped ref");
    format(&r)
}

fn consumer_parses(on_the_wire: &str) -> Result<ArtifactRef, ParseError> {
    parse(on_the_wire)
}

#[test]
fn cdc_5_1_provider_mints_consumer_parses_round_trip() {
    let canonical = [
        "myelin://acme/git/pr/4291",
        "myelin://acme/ci/run/01J0RUN",
        "myelin://acme/issue/issue/ENG-1421",
        "myelin://acme/issue/initiative/PLAT-9",
        "myelin://acme/knowledge/page/7c2",
        "myelin://acme/chat/message/01J0MSG",
        "myelin://acme/identity/member/p-7",
        "myelin://acme/refs/edge/abc123",
        "myelin://acme/git/pr/4291#comment-c9",
        "myelin://acme/chat/message/1#thread-t7",
        "myelin://acme/chat/message/1#message-m3",
        "myelin://acme/knowledge/page/7#b9",
        "myelin://acme/knowledge/page/7#hIntro",
        "myelin://acme/knowledge/row/7#row-r2",
        "myelin://acme/issue/issue/ENG-1#field-status",
        "myelin://acme/git/ref/main#L42-L88",
        "myelin://acme/ci/check/x#check-build",
        "myelin://acme/ci/run/01J#step-3",
    ];
    for raw in canonical {
        let on_the_wire = provider_mints_canonical_urn(raw);
        assert_eq!(
            on_the_wire, raw,
            "provider canonical form drifted for `{raw}`"
        );
        let parsed = consumer_parses(&on_the_wire)
            .unwrap_or_else(|e| panic!("consumer rejected provider URN `{on_the_wire}`: {e}"));
        assert_eq!(
            format(&parsed),
            raw,
            "round-trip not byte-identical for `{raw}`"
        );
    }
}

#[test]
fn cdc_5_1_consumer_rejects_display_projections_loudly() {
    for display in ["#1421", "#42", "@alice", "~general", "ENG-1421"] {
        assert_eq!(
            consumer_parses(display),
            Err(ParseError::MissingScheme {
                input: display.to_string()
            }),
            "consumer must reject the display projection `{display}` (REF-3)"
        );
    }
    assert_eq!(
        consumer_parses("myelin://acme/issue"),
        Err(ParseError::IncompleteScope { got_segments: 2 })
    );
    assert!(matches!(
        consumer_parses("myelin://acme/billing/invoice/1"),
        Err(ParseError::UnknownSubsystem { .. })
    ));
    assert!(matches!(
        consumer_parses("myelin://acme/git/pr/42#widget-9"),
        Err(ParseError::UnknownSubKind { .. })
    ));
    assert_eq!(
        consumer_parses("myelin://acme/issue/issue/ENG 1"),
        Err(ParseError::ForbiddenCharacter)
    );
    assert_eq!(
        consumer_parses(&format!(
            "myelin://acme/issue/issue/{}",
            "x".repeat(myelin_refs::MAX_ARTIFACT_REF_BYTES)
        )),
        Err(ParseError::TooLong)
    );
}

#[test]
fn cdc_5_1_strip_sub_root_agreement() {
    let sub_ref = parse("myelin://acme/git/pr/42#comment-c9").unwrap();
    let root = strip_sub(&sub_ref);
    assert_eq!(format(&root), "myelin://acme/git/pr/42");
    assert_eq!(sub_kind(&sub_ref), Some(Sub::Comment("c9".into())));
    assert_eq!(sub_kind(&root), None);
}
