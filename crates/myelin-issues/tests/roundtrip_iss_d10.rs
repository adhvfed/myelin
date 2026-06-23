//! # ISS-D10 — `render(parse(md)) === md` 100% over an issue body + comment corpus (ISS-P10 / P-376)
//!
//! **The drill (drill-catalogue row ISS-D10):** `render(parse(md)) === md` over a corpus for issue
//! BODIES + COMMENTS (the consumed Issues subset; read + edit use the IDENTICAL WASM parser). The
//! green artifact is **100% round-trip** (0 fixtures fail) over this corpus, run on CI.
//!
//! This is the integration-boundary re-assertion of the embedded unit tests: it feeds a corpus of
//! hand-authored issue body / comment markdown-subset strings (the shapes real issue descriptions +
//! comments take) through the ONE WASM render path the editor compiles to `wasm32-unknown-unknown`
//! ([`myelin_issues::roundtrips_md`] → [`myelin_content::wasm::render_parse`]/`render_serialize`) and
//! asserts every one is a byte-exact fixed point. Read + edit use this IDENTICAL parser — there is no
//! Issues-local renderer (EI-01 §7). It also re-asserts the SHARED `myelin-content` corpus
//! (`myelin_content::corpus::CORPUS`) from the Issues consumer side, so a regression in the frozen
//! grammar fails THIS gate too (the consumer rides the same frozen taxonomy, X-2).

use myelin_content::corpus::CORPUS;
use myelin_content::{InlineNode, OBJ};
use myelin_events::ArtifactRef;
use myelin_issues::roundtrips_md;

/// Synthesise the positional structured-node array for a corpus string: one [`InlineNode`] per
/// `U+FFFC` placeholder (an `artifact_ref` to a corpus URN — the node payload does not affect the
/// markdown-subset round-trip, only the positional placeholder count must match).
fn synth(md: &str) -> Vec<InlineNode> {
    md.chars()
        .filter(|&c| c == OBJ)
        .map(|_| InlineNode::ArtifactRefNode(ArtifactRef("myelin://acme/issue/issue/ENG-1".into())))
        .collect()
}

/// A corpus of issue BODY markdown-subset strings — the shapes a real issue description takes
/// (headings, lists, code, bold/italic/strike, links, structured ref nodes). Each must round-trip
/// byte-exact through the ONE WASM render path.
const ISSUE_BODY_CORPUS: &[&str] = &[
    "",
    "A plain issue description.",
    "Reproduce the **charge bug** when *retrying* a failed payment.",
    "The fix is in `apply_mutation` — see the `OutboxTx::emit` call.",
    "~~Was~~ blocked on the migration; now unblocked.",
    "See the design doc: [data model](https://wiki.test/issues/data-model).",
    "Steps:\n1. open the board\n2. drag the card\n3. observe the rank",
    r"A literal asterisk \* and a bracket \[ stay literal.",
    "snake_case identifiers and f(x) call syntax pass through verbatim.",
    "100% of the corpus must round-trip & stay byte-stable.",
    "Mixed **bold *and italic*** in one run.",
    "A run with `code containing * and ] and ) chars` verbatim.",
];

/// A corpus of issue COMMENT markdown-subset strings — comment bodies (often a structured @-mention
/// or an inline artifact ref). Each must round-trip byte-exact through the SAME WASM render path.
const ISSUE_COMMENT_CORPUS: &[&str] = &[
    "LGTM, merging.",
    "**Approved** — ship it.",
    "This duplicates the earlier report; closing as a dupe.",
    "Nice catch on the `version` CAS.",
    "Linking the upstream fix: [PR #42](https://git.test/pr/42).",
];

/// Body + comment corpus strings WITH structured ref nodes (the @-mention / artifact_ref / embed
/// placeholders). The i-th `U+FFFC` binds the i-th synthesised node; the markdown-subset round-trip
/// is over the placeholder string.
fn corpus_with_nodes() -> Vec<String> {
    vec![
        format!("cc {OBJ} please review"),                   // a mention
        format!("see {OBJ} for the root cause"),             // an artifact_ref
        format!("{OBJ} blocks this; also {OBJ}"),            // two refs
        format!("**{OBJ}** is the owner"),                   // a node inside bold
        format!("[{OBJ}](https://x.test/p) inline"),         // a node inside a link
        format!("a {OBJ} mid-sentence {OBJ} and {OBJ} end"), // three refs
    ]
}

/// **THE ISS-D10 GATE — `render(parse(md)) === md` 100% over the issue body + comment corpus.** Every
/// body, comment, and node-bearing fixture round-trips byte-exact through the ONE WASM render path.
/// A single failure FAILS this gate (the green artifact is 100%, 0 regressions).
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

/// **The SHARED `myelin-content` corpus round-trips from the Issues consumer side too** — Issues
/// rides the frozen taxonomy (X-2), so a regression in the shared grammar fails this Issues gate as
/// well (the consumer never re-implements the grammar; it links the one frozen renderer).
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
