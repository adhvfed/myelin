//! Unit + drill + CDC tests for `humanise` (contract 7.3) and the NOTIF-D4 (0 title/PII leak) gate.
//!
//! The coverage (the prompt's TESTS field):
//! - a tombstone binds on deny (the title NEVER appears);
//! - an erased actor → `[erased user]`;
//! - ICU plural/select/locale formatting;
//! - the markdown path never leaks raw + round-trips `render(parse(md)) === md`;
//! - the CHAINED test (EI-01 §4): render WITH access (title) → revoke (new zookie) → re-render →
//!   the title is now a tombstone (the per-viewer property under a mid-flight permission change);
//! - the NOTIF-D4 drill scenario (0 title/PII leak, measured);
//! - the provider + consumer CDC pair for 7.3.

use super::*;
use crate::Reason;
use myelin_identity::{Consistency, ConsistencyMode, PrincipalId, PrincipalKind, Zookie};
use std::sync::Mutex;

// ── Fixtures ──────────────────────────────────────────────────────────────────────────────

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn viewer(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
}
fn bounded_stale() -> Consistency {
    Consistency { at_least: Zookie(String::new()), mode: ConsistencyMode::BoundedStale }
}
fn strong(zk: &str) -> Consistency {
    Consistency { at_least: Zookie(zk.into()), mode: ConsistencyMode::Strong }
}
/// The canonical NOTIF-D4 confidential subject — a private issue whose TITLE must never leak.
fn confidential_issue() -> ArtifactRef {
    ArtifactRef("myelin://acme/issue/issue/ENG-secret".into())
}
/// The secret title a denied viewer must NEVER see (the leak-test payload).
const SECRET_TITLE: &str = "TOP SECRET acquisition plan";

/// **A programmable synthetic Refs resolve chokepoint** (REF-P10 stands in here — the production
/// wire is the named `ResolveService`-over-resilient-client floor). Per (viewer, ref) it returns a
/// projection (allowed) or a tombstone (denied/erased) — the SAME `Projection | Tombstone` shape the
/// real chokepoint returns. It records `resolve` calls so a test proves humanise resolves per-viewer.
#[derive(Default)]
struct SyntheticResolver {
    /// (viewer_id, ref) pairs the resolve allows — everyone else is DENIED (the leak-test).
    allowed: Mutex<Vec<(String, String)>>,
    /// refs that resolve to a `Tombstone{Erased}` (the erased-actor display, independent of viewer).
    erased: Mutex<Vec<String>>,
    /// the title an allowed ref projects (default [`SECRET_TITLE`]).
    title: Mutex<String>,
    /// per-ref icon an allowed projection carries.
    icon: Mutex<String>,
    /// records every resolve call (proves per-viewer resolution happens).
    calls: Mutex<u64>,
}

impl SyntheticResolver {
    fn new() -> SyntheticResolver {
        let s = SyntheticResolver::default();
        *s.title.lock().unwrap() = SECRET_TITLE.into();
        *s.icon.lock().unwrap() = "lock".into();
        s
    }
    fn allow(&self, viewer_id: &str, ref_: &ArtifactRef) {
        self.allowed.lock().unwrap().push((viewer_id.into(), ref_.0.clone()));
    }
    fn revoke(&self, viewer_id: &str, ref_: &ArtifactRef) {
        self.allowed
            .lock()
            .unwrap()
            .retain(|(v, r)| !(v == viewer_id && r == &ref_.0));
    }
    fn mark_erased(&self, ref_: &ArtifactRef) {
        self.erased.lock().unwrap().push(ref_.0.clone());
    }
    fn call_count(&self) -> u64 {
        *self.calls.lock().unwrap()
    }
}

impl RefResolvePort for SyntheticResolver {
    fn resolve_display(
        &self,
        _tenant: &TenantId,
        _region: &Region,
        ref_: &ArtifactRef,
        viewer: &Principal,
        _at: &Consistency,
    ) -> RefResolution {
        *self.calls.lock().unwrap() += 1;
        // Erased takes precedence (an erased artifact is unrenderable to everyone).
        if self.erased.lock().unwrap().iter().any(|r| r == &ref_.0) {
            return RefResolution::Tombstone(Tombstone {
                root: ref_.clone(),
                reason: TombstoneReason::Erased,
            });
        }
        let allowed = self
            .allowed
            .lock()
            .unwrap()
            .iter()
            .any(|(v, r)| v == &viewer.principal_id.0 && r == &ref_.0);
        if allowed {
            RefResolution::Projection(RefProjection {
                ref_: ref_.clone(),
                title: self.title.lock().unwrap().clone(),
                icon: self.icon.lock().unwrap().clone(),
            })
        } else {
            // DENIED → a tombstone carrying NO title (the leak-free chokepoint).
            RefResolution::Tombstone(Tombstone {
                root: ref_.clone(),
                reason: TombstoneReason::Denied,
            })
        }
    }
}

fn templates() -> TemplateStore {
    TemplateStore::with_platform_defaults()
}

// ════════════════════════════════════════════════════════════════════════════════════════════
//  NOTIF-D4 — the load-bearing leak invariant: a confidential subject humanises to a tombstone,
//  the title NEVER appears in the output. Threshold 0. (the F1 leak floor)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// **NOTIF-D4: a confidential subject humanises to a TOMBSTONE for a viewer lacking access — the
/// title NEVER appears in `text` (0 title/PII leak).** THE chokepoint property: the denied slot is
/// bound to the PII-free tombstone display, never the secret title.
#[test]
fn notif_d4_denied_viewer_gets_tombstone_zero_title_leak() {
    let resolver = SyntheticResolver::new(); // nobody allowed
    let subject = confidential_issue();
    let h = humanise(
        &resolver,
        &tenant(),
        &region(),
        &templates(),
        "review_requested",
        std::slice::from_ref(&subject),
        &viewer("intruder"),
        DEFAULT_LOCALE,
        &strong("z1"),
        Channel::Cli,
    );
    // 0 LEAK: the secret title is absent from EVERY field of the output.
    assert!(
        !h.text.contains(SECRET_TITLE) && !h.text.contains("SECRET") && !h.text.contains("acquisition"),
        "NOTIF-D4: the title must NEVER appear for a denied viewer, got text=`{}`",
        h.text
    );
    // the slot is the PII-free tombstone display.
    assert!(
        h.text.contains("a restricted issue"),
        "the denied slot renders the tombstone display, got `{}`",
        h.text
    );
    // no click-route link to a denied ref (a denied ref is never routable — never leak a route).
    assert!(h.links.is_empty(), "a denied ref yields no link, got {:?}", h.links);
}

/// **NOTIF-D4 across the inbox-item overload + all three channel projections — 0 leak everywhere.**
/// The leak invariant is channel-INDEPENDENT (the slot is bound before the channel lowering).
#[test]
fn notif_d4_zero_leak_across_every_channel_projection() {
    let resolver = SyntheticResolver::new();
    let subject = confidential_issue();
    let item = routed_item(Reason::ReviewRequested, subject);
    for channel in [Channel::Cli, Channel::Email, Channel::Markdown] {
        let h = humanise_item(
            &resolver,
            &templates(),
            &item,
            &viewer("intruder"),
            DEFAULT_LOCALE,
            &strong("z1"),
            channel,
        );
        assert!(
            !h.text.contains(SECRET_TITLE) && !h.text.to_lowercase().contains("secret"),
            "0 leak on {channel:?}: got `{}`",
            h.text
        );
        assert!(h.links.is_empty(), "no link to a denied ref on {channel:?}");
    }
}

/// **An ALLOWED viewer sees the title (the happy path) — and a click-route link + the subject
/// icon.** The complement of NOTIF-D4: the permitted viewer DOES get the title.
#[test]
fn allowed_viewer_sees_title_and_link_and_icon() {
    let resolver = SyntheticResolver::new();
    let subject = confidential_issue();
    resolver.allow("insider", &subject);
    let h = humanise(
        &resolver,
        &tenant(),
        &region(),
        &templates(),
        "review_requested",
        std::slice::from_ref(&subject),
        &viewer("insider"),
        DEFAULT_LOCALE,
        &strong("z1"),
        Channel::Cli,
    );
    assert!(h.text.contains(SECRET_TITLE), "the allowed viewer sees the title, got `{}`", h.text);
    assert_eq!(h.text, "Review requested on TOP SECRET acquisition plan");
    assert_eq!(h.links, vec![subject.0.clone()], "the allowed branch yields the click-route link");
    assert_eq!(h.icon, "lock", "the subject's projection icon (slot 0) drives the item icon");
}

// ════════════════════════════════════════════════════════════════════════════════════════════
//  The CHAINED per-viewer property under a mid-flight permission change (EI-01 §4)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// **CHAINED (EI-01 §4): render WITH access (title shown) → REVOKE (a new zookie) → re-render → the
/// title is now a TOMBSTONE.** The always-current property: the slot is bound at READ time through
/// the per-viewer resolve, so a permission change flips the slot with NO re-write of the item.
#[test]
fn chained_revoke_between_renders_flips_title_to_tombstone() {
    let resolver = SyntheticResolver::new();
    let subject = confidential_issue();
    resolver.allow("alice", &subject);

    // render 1 — WITH access: the title is shown.
    let before = humanise(
        &resolver, &tenant(), &region(), &templates(), "review_requested",
        std::slice::from_ref(&subject), &viewer("alice"), DEFAULT_LOCALE, &strong("z1"), Channel::Cli,
    );
    assert!(before.text.contains(SECRET_TITLE), "before revoke: the title is shown, got `{}`", before.text);

    // REVOKE (a new zookie marks the consistency snapshot).
    resolver.revoke("alice", &subject);

    // render 2 — WITHOUT access: the SAME item re-renders to a tombstone, NO title.
    let after = humanise(
        &resolver, &tenant(), &region(), &templates(), "review_requested",
        std::slice::from_ref(&subject), &viewer("alice"), DEFAULT_LOCALE, &strong("z2"), Channel::Cli,
    );
    assert!(
        !after.text.contains(SECRET_TITLE),
        "after revoke: the title must NOT leak (the per-viewer property under a permission change), got `{}`",
        after.text
    );
    assert!(after.text.contains("a restricted issue"), "after revoke: a tombstone, got `{}`", after.text);
}

// ════════════════════════════════════════════════════════════════════════════════════════════
//  Erasure-safe — an erased actor → [erased user] (EI-04 §1, references-not-payloads)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// **An erased actor humanises to `[erased user]` with NO stored PII to scrub.** The ref resolves to
/// a `Tombstone{Erased}` → the canonical erased display; the inbox row is NOT mutated (it stored a
/// ref, not a rendered string).
#[test]
fn erased_actor_humanises_to_erased_user() {
    let resolver = SyntheticResolver::new();
    let actor = ArtifactRef("myelin://acme/identity/user/u-42".into());
    resolver.allow("bob", &actor); // even an allowed viewer sees the erased display
    resolver.mark_erased(&actor);
    let h = humanise(
        &resolver, &tenant(), &region(), &templates(), "mentioned",
        &[actor], &viewer("bob"), DEFAULT_LOCALE, &strong("z1"), Channel::Cli,
    );
    assert_eq!(h.text, "You were mentioned in [erased user]", "an erased actor → [erased user]");
    assert!(h.links.is_empty(), "an erased ref is not routable");
}

// ════════════════════════════════════════════════════════════════════════════════════════════
//  The ICU-MessageFormat subset — {N} slots, plural, select, locale
// ════════════════════════════════════════════════════════════════════════════════════════════

#[test]
fn icu_positional_slot_substitution() {
    assert_eq!(render_message("Hello {0} and {1}", &["Alice".into(), "Bob".into()]), "Hello Alice and Bob");
}

#[test]
fn icu_plural_one_vs_other() {
    let body = "{0, plural, one {# comment} other {# comments}}";
    assert_eq!(render_message(body, &["1".into()]), "1 comment");
    assert_eq!(render_message(body, &["3".into()]), "3 comments");
    assert_eq!(render_message(body, &["0".into()]), "0 comments");
}

#[test]
fn icu_select_by_value() {
    let body = "{0, select, merged {It merged} closed {It closed} other {It changed}}";
    assert_eq!(render_message(body, &["merged".into()]), "It merged");
    assert_eq!(render_message(body, &["closed".into()]), "It closed");
    assert_eq!(render_message(body, &["draft".into()]), "It changed");
}

#[test]
fn icu_nested_slot_inside_plural_branch() {
    // `#` is the count; a nested `{1}` still binds.
    let body = "{0, plural, one {# review for {1}} other {# reviews for {1}}}";
    assert_eq!(render_message(body, &["1".into(), "ENG-1".into()]), "1 review for ENG-1");
    assert_eq!(render_message(body, &["2".into(), "ENG-1".into()]), "2 reviews for ENG-1");
}

#[test]
fn icu_out_of_range_slot_is_empty_never_panics() {
    // A template referencing a missing arg degrades to empty — it does not crash the inbox render.
    assert_eq!(render_message("X{5}Y", &["a".into()]), "XY");
}

/// **Per-locale rendering: a tenant `fr` override shadows the platform `en` default (§2.5).** Same
/// key, different locale → the localised body renders.
#[test]
fn locale_override_renders_localised_body() {
    let mut store = TemplateStore::with_platform_defaults();
    store.put(HumaniseTemplate {
        tenant: tenant().0.clone(),
        template_key: "review_requested".into(),
        locale: "fr".into(),
        body: "Revue demandée sur {0}".into(),
        icon: "review".into(),
    });
    let resolver = SyntheticResolver::new();
    let subject = confidential_issue();
    resolver.allow("insider", &subject);
    let h = humanise(
        &resolver, &tenant(), &region(), &store, "review_requested",
        &[subject], &viewer("insider"), "fr", &strong("z1"), Channel::Cli,
    );
    assert_eq!(h.text, "Revue demandée sur TOP SECRET acquisition plan", "the fr override renders");
}

/// **Adjacent placeholders + a placeholder at the very start/end of the body bind exactly (the
/// formatter index arithmetic is precise — off-by-one in the scan corrupts these).**
#[test]
fn icu_adjacent_and_boundary_placeholders() {
    assert_eq!(render_message("{0}{1}", &["A".into(), "B".into()]), "AB");
    assert_eq!(render_message("{0} end", &["start".into()]), "start end");
    assert_eq!(render_message("start {0}", &["end".into()]), "start end");
    // a literal trailing brace after a valid placeholder is preserved verbatim.
    assert_eq!(render_message("{0} {{lit", &["x".into()]), "x {{lit");
}

/// **A non-numeric plural arg selects `other` (the `unwrap_or(-1)` boundary — a deleted `-` would
/// make `1` the fallback and wrongly pick `one`).** Pins the plural-default arithmetic.
#[test]
fn icu_plural_non_numeric_selects_other_not_one() {
    let body = "{0, plural, one {single} other {many}}";
    assert_eq!(render_message(body, &["not-a-number".into()]), "many");
    // and an empty arg likewise picks other (never the `one` branch).
    assert_eq!(render_message(body, &[String::new()]), "many");
}

/// **A plural/select body with commas INSIDE a branch is not split on those commas (the top-level
/// comma scanner respects brace depth).** A deleted depth tracking would split mid-branch.
#[test]
fn icu_commas_inside_branch_do_not_split_the_placeholder() {
    let body = "{0, select, yes {a, b, c} other {x, y}}";
    assert_eq!(render_message(body, &["yes".into()]), "a, b, c");
    assert_eq!(render_message(body, &["no".into()]), "x, y");
}

/// **A select with NO matching key AND NO `other` branch renders empty (the branch lookup returns
/// nothing rather than panicking) — and `parse_branches` whitespace/keys are exact.**
#[test]
fn icu_select_missing_key_and_no_other_is_empty() {
    let body = "{0, select, a {AA}}";
    assert_eq!(render_message(body, &["a".into()]), "AA");
    assert_eq!(render_message(body, &["z".into()]), "", "no match, no other → empty");
}

// ════════════════════════════════════════════════════════════════════════════════════════════
//  The ONE content render path (13.1) — markdown subset, round-trip render(parse(md)) === md
// ════════════════════════════════════════════════════════════════════════════════════════════

/// **The render-determinism check (the prompt's GATE): `render(parse(md)) === md` for every
/// markdown-subset string through the ONE content render path.** The content-crate round-trip
/// generalised to humanise output — markdown is NEVER leaked raw; it round-trips byte-identically.
#[test]
fn render_parse_round_trip_is_identity() {
    let corpus = [
        "plain text",
        "**bold** here",
        "an *italic* word",
        "some `inline code`",
        "a [label](https://example.test/x) link",
        "mixed **b** and *i* and `c` and [l](u)",
        "Review requested on TOP SECRET acquisition plan",
        "", // the empty string round-trips
    ];
    for md in corpus {
        let round = render_markdown(&parse_markdown(md));
        assert_eq!(round, md, "render(parse(md)) === md must hold for `{md}`");
    }
}

/// **Markdown-parser precision: an UNTERMINATED delimiter is literal text (no closer → the `*`/`` ` ``
/// is kept), an EMPTY delimiter is literal, and a span that runs to EOF is handled.** These hit the
/// `read_delim`/`read_link` bound checks (a `< → <=` off-by-one or an `|| → &&` flips them).
#[test]
fn markdown_unterminated_and_empty_delimiters_are_literal() {
    // an unterminated bold runs to EOF with no closer → literal.
    assert_eq!(render_markdown(&parse_markdown("a **b")), "a **b");
    // an unterminated italic / code → literal.
    assert_eq!(render_markdown(&parse_markdown("x *y")), "x *y");
    assert_eq!(render_markdown(&parse_markdown("p `q")), "p `q");
    // an EMPTY emphasis (`**` immediately closed) is NOT bold — kept literal.
    assert_eq!(render_markdown(&parse_markdown("****")), "****");
    // a code span that ends exactly at EOF.
    assert_eq!(render_markdown(&parse_markdown("`c`")), "`c`");
}

/// **A link at the EXACT end of the string parses (the `read_link` end-of-buffer bounds), and a
/// MALFORMED link (`[` with no `](url)`) is literal text — the `||` short-circuits + `<` bounds in
/// `read_link` are pinned by these.**
#[test]
fn markdown_link_at_eof_and_malformed_link_literal() {
    // a well-formed link ending exactly at EOF.
    let doc = parse_markdown("[l](u)");
    assert_eq!(doc.spans, vec![Span::Link { label: "l".into(), url: "u".into() }]);
    assert_eq!(render_markdown(&doc), "[l](u)");
    // malformed: a `[label]` with no `(url)` → literal text (round-trips).
    assert_eq!(render_markdown(&parse_markdown("[just a label]")), "[just a label]");
    // malformed: `[label](url` with no closing paren → literal.
    assert_eq!(render_markdown(&parse_markdown("[l](u")), "[l](u");
    // malformed: a bare `[` at EOF → literal.
    assert_eq!(render_markdown(&parse_markdown("trailing [")), "trailing [");
}

/// **A link whose plain text continues after it parses both spans (the index past the `)` is exact —
/// a `+ → *` on the cursor advance would drop or duplicate the tail).**
#[test]
fn markdown_text_after_link_is_preserved() {
    let doc = parse_markdown("see [l](u) now");
    assert_eq!(
        doc.spans,
        vec![
            Span::Text("see ".into()),
            Span::Link { label: "l".into(), url: "u".into() },
            Span::Text(" now".into()),
        ]
    );
    assert_eq!(render_markdown(&doc), "see [l](u) now");
}

/// **The PARSED span structure is exact for bold/italic/code (not just the round-trip).** A
/// round-trip can hide a parser regression that degrades bold→literal (it still round-trips); these
/// assert the actual [`Span`] vector, so a `i + 2`/`i + 1` start-offset mutant that drops the
/// emphasis is caught (the span becomes `Text` instead of `Bold`).
#[test]
fn markdown_parses_exact_span_structure() {
    assert_eq!(
        parse_markdown("**b**").spans,
        vec![Span::Bold("b".into())],
        "`**b**` parses to ONE Bold span (not literal text)"
    );
    assert_eq!(
        parse_markdown("*i*").spans,
        vec![Span::Italic("i".into())],
        "`*i*` parses to ONE Italic span"
    );
    assert_eq!(
        parse_markdown("`c`").spans,
        vec![Span::Code("c".into())],
        "`` `c` `` parses to ONE Code span"
    );
    // a bold NOT at offset 0 (text before it) — pins the start offset is `i + 2`, not `i * 2`.
    assert_eq!(
        parse_markdown("x **b**").spans,
        vec![Span::Text("x ".into()), Span::Bold("b".into())],
        "text then bold parses to [Text, Bold] with the correct inner"
    );
    assert_eq!(
        parse_markdown("ab *i*").spans,
        vec![Span::Text("ab ".into()), Span::Italic("i".into())],
    );
    assert_eq!(
        parse_markdown("xy `c`").spans,
        vec![Span::Text("xy ".into()), Span::Code("c".into())],
    );
}

/// **The email projection HTML-escapes content (never an injection vector, never raw markdown).**
#[test]
fn html_projection_escapes_and_renders_structure() {
    let doc = parse_markdown("a **bold** & [x](http://h?a=1&b=2)");
    let html = render_html(&doc);
    assert!(html.contains("<strong>bold</strong>"), "bold → <strong>, got `{html}`");
    assert!(html.contains("&amp;"), "the ampersand is escaped, got `{html}`");
    assert!(!html.contains(" & "), "a raw ampersand must never leak, got `{html}`");
}

/// **The plain projection strips structure (the CLI/in-app channel).**
#[test]
fn plain_projection_strips_markup() {
    let doc = parse_markdown("a **bold** and a [link](http://h)");
    assert_eq!(render_plain(&doc), "a bold and a link");
}

// ════════════════════════════════════════════════════════════════════════════════════════════
//  Per-viewer resolution actually happens + the template store fallback ladder
// ════════════════════════════════════════════════════════════════════════════════════════════

/// **humanise resolves EACH ref arg per-viewer (the resolve is never skipped).**
#[test]
fn humanise_resolves_each_ref_arg_per_viewer() {
    let resolver = SyntheticResolver::new();
    let a = ArtifactRef("myelin://acme/issue/issue/A".into());
    let b = ArtifactRef("myelin://acme/issue/issue/B".into());
    resolver.allow("v", &a);
    resolver.allow("v", &b);
    let _ = humanise(
        &resolver, &tenant(), &region(), &templates(), "mentioned",
        &[a, b], &viewer("v"), DEFAULT_LOCALE, &strong("z1"), Channel::Cli,
    );
    assert_eq!(resolver.call_count(), 2, "each of the two ref args is resolved per-viewer");
}

/// **An UNREGISTERED template key degrades to a stable fallback — still leak-free (slot 0 is the
/// per-viewer-bound subject, so a denied subject is STILL a tombstone in the fallback).**
#[test]
fn unregistered_key_falls_back_without_leaking() {
    let resolver = SyntheticResolver::new(); // denied
    let store = TemplateStore::new(); // no templates registered
    let subject = confidential_issue();
    let h = humanise(
        &resolver, &tenant(), &region(), &store, "some.unknown.key",
        &[subject], &viewer("intruder"), DEFAULT_LOCALE, &strong("z1"), Channel::Cli,
    );
    assert!(!h.text.contains(SECRET_TITLE), "the fallback must not leak a title, got `{}`", h.text);
    assert!(h.text.contains("a restricted issue"), "the fallback binds the per-viewer tombstone, got `{}`", h.text);
}

/// **The §2.5 fallback ladder: a tenant override shadows the platform default at the same key.**
#[test]
fn template_store_tenant_override_shadows_default() {
    let mut store = TemplateStore::with_platform_defaults();
    store.put(HumaniseTemplate {
        tenant: tenant().0.clone(),
        template_key: "review_requested".into(),
        locale: DEFAULT_LOCALE.into(),
        body: "ACME wants your review on {0}".into(),
        icon: "review".into(),
    });
    let found = store.lookup(&tenant().0, "review_requested", DEFAULT_LOCALE).unwrap();
    assert_eq!(found.body, "ACME wants your review on {0}", "the tenant override wins");
    // a different tenant still gets the platform default.
    let other = store.lookup("other-tenant", "review_requested", DEFAULT_LOCALE).unwrap();
    assert_eq!(other.body, "Review requested on {0}", "another tenant gets the platform default");
}

/// **The fallback body for an unregistered key: with NO args it is the bare key (no `{0}` slot to
/// reference); with args it carries the per-viewer-bound `{0}`.** Pins the `arg_count > 0` boundary
/// (a `> → >=` mutant would emit a `{0}` slot even with zero args — and `render_message` would leave
/// the dangling `{0}` empty, a different output).
#[test]
fn fallback_body_arg_count_boundary() {
    let resolver = SyntheticResolver::new();
    let store = TemplateStore::new();
    // ZERO ref args → the fallback is the bare key, no slot.
    let h0 = humanise(
        &resolver, &tenant(), &region(), &store, "no.args.key",
        &[], &viewer("v"), DEFAULT_LOCALE, &strong("z1"), Channel::Cli,
    );
    assert_eq!(h0.text, "no.args.key", "zero args → the bare key (no dangling slot)");
    // ONE allowed arg → the fallback carries the bound title.
    let subject = confidential_issue();
    resolver.allow("v", &subject);
    let h1 = humanise(
        &resolver, &tenant(), &region(), &store, "one.arg.key",
        std::slice::from_ref(&subject), &viewer("v"), DEFAULT_LOCALE, &strong("z1"), Channel::Cli,
    );
    assert_eq!(h1.text, format!("one.arg.key: {SECRET_TITLE}"), "one arg → key + bound slot");
}

/// **`shared_platform_templates()` carries the platform defaults (NOT an empty store).** Pins the
/// `Arc::new(Default::default())` mutant — an empty store would render the fallback, not the default
/// template body.
#[test]
fn shared_platform_templates_has_the_defaults() {
    let store = shared_platform_templates();
    let t = store
        .lookup(PLATFORM_DEFAULT_TENANT, "review_requested", DEFAULT_LOCALE)
        .expect("the shared store carries the platform defaults");
    assert_eq!(t.body, "Review requested on {0}", "the default template body is present");
}

/// **The tombstone display is kind-shaped from the OPAQUE root URN, never from content.** A page
/// subject → `a restricted page`; an unknown URN → `a restricted item`.
#[test]
fn tombstone_display_is_kind_shaped_pii_free() {
    let page = Tombstone {
        root: ArtifactRef("myelin://acme/knowledge/page/7c2".into()),
        reason: TombstoneReason::Denied,
    };
    assert_eq!(tombstone_display(&page), "a restricted page");
    let weird = Tombstone {
        root: ArtifactRef("not-a-urn".into()),
        reason: TombstoneReason::RootGone,
    };
    assert_eq!(tombstone_display(&weird), "a restricted item");
    let erased = Tombstone {
        root: ArtifactRef("myelin://acme/identity/user/x".into()),
        reason: TombstoneReason::Erased,
    };
    assert_eq!(tombstone_display(&erased), "[erased user]");
}

// ════════════════════════════════════════════════════════════════════════════════════════════
//  The 7.3 provider + consumer CDC pair (the contract-coverage scanner reads this)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// **CDC — the 7.3 PROVIDER contract: `humanise` returns the frozen `HumanisedString{text, links[],
/// icon}` shape, and a denied subject is a tombstone (the leak invariant is part of the contract).**
#[test]
fn cdc_7_3_provider_contract_shape_and_leak_invariant() {
    let resolver = SyntheticResolver::new();
    let subject = confidential_issue();
    resolver.allow("insider", &subject);
    let h = humanise(
        &resolver, &tenant(), &region(), &templates(), "assigned",
        std::slice::from_ref(&subject), &viewer("insider"), DEFAULT_LOCALE, &strong("z1"), Channel::Cli,
    );
    // the frozen three-field shape.
    assert_eq!(h.text, "You were assigned TOP SECRET acquisition plan");
    assert_eq!(h.links, vec![subject.0.clone()]);
    assert_eq!(h.icon, "lock");
    // the contract's leak clause: a denied viewer of the SAME subject gets a tombstone, never the title.
    let denied = humanise(
        &resolver, &tenant(), &region(), &templates(), "assigned",
        &[subject], &viewer("outsider"), DEFAULT_LOCALE, &strong("z1"), Channel::Cli,
    );
    assert!(!denied.text.contains(SECRET_TITLE), "the 7.3 contract: a denied viewer never sees the title");
}

/// **CDC — the 7.3 CONSUMER contract: a delivery/inbox consumer calls humanise via the item
/// overload and reads ONLY `{text, links, icon}`; the resolve mode is the frozen `Display`.** The
/// consumer never reaches around humanise for a raw title (the ONE templating surface).
#[test]
fn cdc_7_3_consumer_mode_is_display_and_uses_one_surface() {
    assert_eq!(HUMANISE_RESOLVE_MODE, "Display", "humanise always resolves refs in Display mode (5.2)");
    let resolver = SyntheticResolver::new();
    let subject = confidential_issue();
    resolver.allow("insider", &subject);
    let item = routed_item(Reason::Mentioned, subject);
    let h = humanise_item(&resolver, &templates(), &item, &viewer("insider"), DEFAULT_LOCALE, &bounded_stale(), Channel::Cli);
    // the consumer reads the rendered shape — never a stored string.
    assert!(h.text.contains(SECRET_TITLE), "the allowed consumer renders the title through humanise");
    assert_eq!(reason_template_key(Reason::Mentioned), "mentioned", "the item overload keys on the reason");
}

// ── A RoutedInboxItem fixture (the inbox-item overload's input). ─────────────────────────────

fn routed_item(reason: Reason, subject: ArtifactRef) -> RoutedInboxItem {
    RoutedInboxItem {
        tenant: tenant(),
        region: region(),
        item_id: "it-1".into(),
        recipient: "insider".into(),
        subject,
        reason,
        class: crate::Class::Direct,
        origin_event: ArtifactRef("myelin://acme/issue/event/e-1".into()),
        dedup_key: "dk-1".into(),
        coalesce_count: 0,
        state: "unread".into(),
        snooze_until: None,
    }
}
