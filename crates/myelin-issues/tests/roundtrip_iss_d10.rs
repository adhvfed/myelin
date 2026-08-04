use myelin_content::corpus::CORPUS;
use myelin_content::{InlineNode, OBJ};
use myelin_events::ArtifactRef;
use myelin_issues::roundtrips_md;

fn synth(md: &str) -> Vec<InlineNode> {
    md.chars()
        .filter(|&c| c == OBJ)
        .map(|_| InlineNode::ArtifactRefNode(ArtifactRef("myelin://acme/issue/issue/ENG-1".into())))
        .collect()
}

const ISSUE_BODY_CORPUS: &[&str] = &[
    "",
    "A plain issue description.",
    "Reproduce the **charge bug** when *retrying* a failed payment.",
    "The fix is in `apply_mutation` - see the `OutboxTx::emit` call.",
    "~~Was~~ blocked on the migration; now unblocked.",
    "See the design doc: [data model](https://wiki.test/issues/data-model).",
    "Steps:\n1. open the board\n2. drag the card\n3. observe the rank",
    r"A literal asterisk \* and a bracket \[ stay literal.",
    "snake_case identifiers and f(x) call syntax pass through verbatim.",
    "100% of the corpus must round-trip & stay byte-stable.",
    "Mixed **bold *and italic*** in one run.",
    "A run with `code containing * and ] and ) chars` verbatim.",
];

const ISSUE_COMMENT_CORPUS: &[&str] = &[
    "LGTM, merging.",
    "**Approved** - ship it.",
    "This duplicates the earlier report; closing as a dupe.",
    "Nice catch on the `version` CAS.",
    "Linking the upstream fix: [PR #42](https://git.test/pr/42).",
];

fn corpus_with_nodes() -> Vec<String> {
    vec![
        format!("cc {OBJ} please review"),
        format!("see {OBJ} for the root cause"),
        format!("{OBJ} blocks this; also {OBJ}"),
        format!("**{OBJ}** is the owner"),
        format!("[{OBJ}](https://x.test/p) inline"),
        format!("a {OBJ} mid-sentence {OBJ} and {OBJ} end"),
    ]
}

#[test]
fn iss_d10_body_and_comment_corpus_round_trips_100_percent() {
    let mut total = 0usize;
    let mut passed = 0usize;

    for &md in ISSUE_BODY_CORPUS.iter().chain(ISSUE_COMMENT_CORPUS) {
        total += 1;
        let nodes = synth(md);
        if roundtrips_md(md, &nodes) {
            passed += 1;
        } else {
            panic!("ISS-D10: issue body/comment fixture did NOT round-trip: {md:?}");
        }
    }

    for md in corpus_with_nodes() {
        total += 1;
        let nodes = synth(&md);
        if roundtrips_md(&md, &nodes) {
            passed += 1;
        } else {
            panic!("ISS-D10: node-bearing fixture did NOT round-trip: {md:?}");
        }
    }

    assert_eq!(passed, total, "ISS-D10: round-trip must be 100%");
    assert!(total >= 20, "the corpus must be non-trivial (got {total})");
}

#[test]
fn shared_content_corpus_round_trips_from_issues_consumer() {
    for f in CORPUS {
        let nodes = synth(f.md);
        assert!(
            roundtrips_md(f.md, &nodes),
            "shared corpus fixture {} did not round-trip via the Issues consumer path",
            f.name
        );
    }
}
