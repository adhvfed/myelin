use myelin_chat::roundtrips_md;
use myelin_content::corpus::CORPUS;
use myelin_content::{InlineNode, OBJ};
use myelin_events::ArtifactRef;

fn synth(md: &str) -> Vec<InlineNode> {
    md.chars()
        .filter(|&c| c == OBJ)
        .map(|_| {
            InlineNode::ArtifactRefNode(ArtifactRef("myelin://acme/chat/message/01J0M".into()))
        })
        .collect()
}

const MESSAGE_CORPUS: &[&str] = &[
    "",
    "hey, can you take a look?",
    "the **deploy** is *blocked* on the migration",
    "run `cargo test --workspace` before pushing",
    "~~ignore~~ that was the old plan",
    "see the [runbook](https://wiki.test/runbook) for the rollback steps",
    "Mixed **bold *and italic*** in a single message.",
    r"a literal asterisk \* and a bracket \[ stay literal",
    "snake_case ids and f(x) call syntax pass through verbatim",
    "100% green & shipping today",
    "`code containing * and ] and ) chars` is verbatim",
    "LGTM, merging",
    "**approved** - ship it",
    "nice catch on the `version` CAS",
];

fn corpus_with_nodes() -> Vec<String> {
    vec![
        format!("cc {OBJ} please review"),
        format!("see {OBJ} for the root cause"),
        format!("{OBJ} blocks this; also {OBJ}"),
        format!("**{OBJ}** owns this"),
        format!("[{OBJ}](https://x.test/p) inline"),
        format!("a {OBJ} mid-message {OBJ} and {OBJ} end"),
        format!("ping {OBJ} about {OBJ} - `not a {OBJ}?`"),
    ]
}

#[test]
fn chat_message_corpus_round_trips_100_percent() {
    let mut total = 0usize;
    let mut passed = 0usize;
    let mut mismatches: Vec<String> = Vec::new();

    for &md in MESSAGE_CORPUS {
        total += 1;
        let nodes = synth(md);
        if roundtrips_md(md, &nodes) {
            passed += 1;
        } else {
            mismatches.push(format!("{md:?}"));
        }
    }

    for md in corpus_with_nodes() {
        total += 1;
        let nodes = synth(&md);
        if roundtrips_md(&md, &nodes) {
            passed += 1;
        } else {
            mismatches.push(format!("{md:?}"));
        }
    }

    assert_eq!(
        mismatches.len(),
        0,
        "the round-trip-mismatch signal must be 0 over the corpus; got: {mismatches:?}"
    );
    assert_eq!(
        passed, total,
        "round-trip must be 100% over the chat corpus"
    );
    assert!(total >= 20, "the corpus must be non-trivial (got {total})");
}

#[test]
fn shared_content_corpus_round_trips_from_chat_consumer() {
    for f in CORPUS {
        let nodes = synth(f.md);
        assert!(
            roundtrips_md(f.md, &nodes),
            "shared corpus fixture {} did not round-trip via the Chat consumer path",
            f.name
        );
    }
}
