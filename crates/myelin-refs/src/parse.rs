use myelin_events::{ArtifactRef, ARTIFACT_TYPE_TOKENS, SUBSYSTEM_TOKENS};
use myelin_tenancy::TenantId;
use serde::{Deserialize, Serialize};

pub const SCHEME: &str = "myelin://";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseError {
    MissingScheme {
        input: String,
    },
    IncompleteScope {
        got_segments: usize,
    },
    EmptySegment {
        segment: &'static str,
    },
    UnknownSubsystem {
        token: String,
    },
    UnknownType {
        token: String,
    },
    EmptySub,
    UnknownSubKind {
        sub: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedArtifactRef {
    pub artifact_ref: ArtifactRef,
    pub tenant: TenantId,
    pub subsystem: String,
    pub type_: String,
    pub id: String,
    pub sub: Option<Sub>,
}

impl core::fmt::Display for ParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ParseError::MissingScheme { input } => write!(
                f,
                "not an ArtifactRef: `{input}` does not start with the canonical scheme `{SCHEME}` \
                 - scope-less/short-hash refs are render-time display projections (REF-3, §4.8), \
                 never a stored scope."
            ),
            ParseError::IncompleteScope { got_segments } => write!(
                f,
                "ambiguous ArtifactRef: scope is not total - a `{SCHEME}` URN needs exactly four \
                 segments `tenant/subsystem/type/id`, got {got_segments}. Scope is never guessed \
                 (REF-3)."
            ),
            ParseError::EmptySegment { segment } => write!(
                f,
                "malformed ArtifactRef: the `{segment}` segment is empty - every scope segment is \
                 required and non-empty (§3.1)."
            ),
            ParseError::UnknownSubsystem { token } => write!(
                f,
                "unknown subsystem token `{token}`: not in the Bus §6.2 canonical set \
                 {SUBSYSTEM_TOKENS:?}. Refs validates against the Bus token table, it never authors \
                 a new one."
            ),
            ParseError::UnknownType { token } => write!(
                f,
                "unknown artifact-type token `{token}`: not in the Bus §6.2 canonical set \
                 {ARTIFACT_TYPE_TOKENS:?}."
            ),
            ParseError::EmptySub => write!(
                f,
                "malformed `#sub`: the sub-anchor is empty (`…/id#`) - a `#sub` carries a \
                 self-describing kind (§3.5)."
            ),
            ParseError::UnknownSubKind { sub } => write!(
                f,
                "unknown/ambiguous `#sub` kind `{sub}`: not in the frozen §3.5 vocabulary \
                 (comment-/thread-/message-/b/h/row-/field-/L<a>-L<b>/check-/step-). The kind \
                 prefix is self-describing; an unknown kind is rejected, never guessed (REF-3)."
            ),
        }
    }
}

impl std::error::Error for ParseError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sub {
    Comment(String),
    Thread(String),
    Message(String),
    Block(String),
    Heading(String),
    Row(String),
    Field(String),
    LineRange {
        start: u64,
        end: u64,
    },
    Check(String),
    CommitCheck { commit_oid: String, context: String },
    CommitCiResult { commit_oid: String },
    Step(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SubKind {
    Comment,
    Thread,
    Message,
    Block,
    Heading,
    Row,
    Field,
    LineRange,
    Check,
    Step,
}

impl SubKind {
    pub const fn label(self) -> &'static str {
        match self {
            SubKind::Comment => "comment-",
            SubKind::Thread => "thread-",
            SubKind::Message => "message-",
            SubKind::Block => "b",
            SubKind::Heading => "h",
            SubKind::Row => "row-",
            SubKind::Field => "field-",
            SubKind::LineRange => "L<a>-L<b>",
            SubKind::Check => "check-",
            SubKind::Step => "step-",
        }
    }
}

impl Sub {
    pub const fn kind(&self) -> SubKind {
        match self {
            Sub::Comment(_) => SubKind::Comment,
            Sub::Thread(_) => SubKind::Thread,
            Sub::Message(_) => SubKind::Message,
            Sub::Block(_) => SubKind::Block,
            Sub::Heading(_) => SubKind::Heading,
            Sub::Row(_) => SubKind::Row,
            Sub::Field(_) => SubKind::Field,
            Sub::LineRange { .. } => SubKind::LineRange,
            Sub::Check(_) => SubKind::Check,
            Sub::CommitCheck { .. } | Sub::CommitCiResult { .. } => SubKind::Check,
            Sub::Step(_) => SubKind::Step,
        }
    }
}

fn parse_sub(sub: &str) -> Result<Sub, ParseError> {
    if sub.is_empty() {
        return Err(ParseError::EmptySub);
    }

    if let Some(commit_tail) = sub.strip_prefix("commit-") {
        if let Some((commit_oid, context)) = commit_tail.split_once("/check-") {
            if !commit_oid.is_empty()
                && !context.is_empty()
                && !commit_oid.contains('/')
                && !context.contains('/')
            {
                return Ok(Sub::CommitCheck {
                    commit_oid: commit_oid.to_string(),
                    context: context.to_string(),
                });
            }
            return Err(ParseError::UnknownSubKind { sub: sub.into() });
        }
        if let Some(commit_oid) = commit_tail.strip_suffix("/ci-result") {
            if !commit_oid.is_empty() && !commit_oid.contains('/') {
                return Ok(Sub::CommitCiResult {
                    commit_oid: commit_oid.to_string(),
                });
            }
            return Err(ParseError::UnknownSubKind { sub: sub.into() });
        }
    }

    for (prefix, ctor) in [
        ("comment-", Sub::Comment as fn(String) -> Sub),
        ("thread-", Sub::Thread),
        ("message-", Sub::Message),
        ("row-", Sub::Row),
        ("field-", Sub::Field),
        ("check-", Sub::Check),
    ] {
        if let Some(body) = sub.strip_prefix(prefix) {
            if body.is_empty() {
                return Err(ParseError::UnknownSubKind { sub: sub.into() });
            }
            return Ok(ctor(body.to_string()));
        }
    }

    if let Some(n) = sub.strip_prefix("step-") {
        return n
            .parse::<u64>()
            .map(Sub::Step)
            .map_err(|_| ParseError::UnknownSubKind { sub: sub.into() });
    }

    if let Some(range) = sub.strip_prefix('L') {
        if let Some((start_s, end_s)) = range.split_once("-L") {
            if let (Ok(start), Ok(end)) = (start_s.parse::<u64>(), end_s.parse::<u64>()) {
                if end >= start {
                    return Ok(Sub::LineRange { start, end });
                }
            }
        }
        return Err(ParseError::UnknownSubKind { sub: sub.into() });
    }

    if let Some(body) = sub.strip_prefix('b') {
        if !body.is_empty() {
            return Ok(Sub::Block(body.to_string()));
        }
    }
    if let Some(body) = sub.strip_prefix('h') {
        if !body.is_empty() {
            return Ok(Sub::Heading(body.to_string()));
        }
    }

    Err(ParseError::UnknownSubKind { sub: sub.into() })
}

fn format_sub(sub: &Sub) -> String {
    match sub {
        Sub::Comment(id) => format!("comment-{id}"),
        Sub::Thread(id) => format!("thread-{id}"),
        Sub::Message(id) => format!("message-{id}"),
        Sub::Block(id) => format!("b{id}"),
        Sub::Heading(id) => format!("h{id}"),
        Sub::Row(id) => format!("row-{id}"),
        Sub::Field(id) => format!("field-{id}"),
        Sub::LineRange { start, end } => format!("L{start}-L{end}"),
        Sub::Check(ctx) => format!("check-{ctx}"),
        Sub::CommitCheck {
            commit_oid,
            context,
        } => format!("commit-{commit_oid}/check-{context}"),
        Sub::CommitCiResult { commit_oid } => format!("commit-{commit_oid}/ci-result"),
        Sub::Step(n) => format!("step-{n}"),
    }
}

pub fn parse_scoped(s: &str) -> Result<ParsedArtifactRef, ParseError> {
    let rest = s
        .strip_prefix(SCHEME)
        .ok_or_else(|| ParseError::MissingScheme {
            input: s.to_string(),
        })?;

    let (scope, sub_text): (&str, Option<&str>) = match rest.split_once('#') {
        Some((scope, sub)) => (scope, Some(sub)),
        None => (rest, None),
    };

    let segments: Vec<&str> = scope.split('/').collect();
    if segments.len() != 4 {
        return Err(ParseError::IncompleteScope {
            got_segments: segments.len(),
        });
    }
    let (tenant, subsystem, type_, id) = (segments[0], segments[1], segments[2], segments[3]);

    if tenant.is_empty() {
        return Err(ParseError::EmptySegment { segment: "tenant" });
    }
    if subsystem.is_empty() {
        return Err(ParseError::EmptySegment {
            segment: "subsystem",
        });
    }
    if type_.is_empty() {
        return Err(ParseError::EmptySegment { segment: "type" });
    }
    if id.is_empty() {
        return Err(ParseError::EmptySegment { segment: "id" });
    }

    if !SUBSYSTEM_TOKENS.contains(&subsystem) {
        return Err(ParseError::UnknownSubsystem {
            token: subsystem.to_string(),
        });
    }
    if !ARTIFACT_TYPE_TOKENS.contains(&type_) {
        return Err(ParseError::UnknownType {
            token: type_.to_string(),
        });
    }

    let parsed_sub = match sub_text {
        None => None,
        Some(sub) => Some(parse_sub(sub)?),
    };
    let canonical = match &parsed_sub {
        None => format!("{SCHEME}{tenant}/{subsystem}/{type_}/{id}"),
        Some(parsed) => {
            format!(
                "{SCHEME}{tenant}/{subsystem}/{type_}/{id}#{}",
                format_sub(parsed)
            )
        }
    };
    Ok(ParsedArtifactRef {
        artifact_ref: ArtifactRef(canonical),
        tenant: TenantId(tenant.to_string()),
        subsystem: subsystem.to_string(),
        type_: type_.to_string(),
        id: id.to_string(),
        sub: parsed_sub,
    })
}

pub fn parse(s: &str) -> Result<ArtifactRef, ParseError> {
    parse_scoped(s).map(|parsed| parsed.artifact_ref)
}

pub fn format(r: &ArtifactRef) -> String {
    r.0.clone()
}

pub fn strip_sub(r: &ArtifactRef) -> ArtifactRef {
    match r.0.split_once('#') {
        Some((root, _sub)) => ArtifactRef(root.to_string()),
        None => r.clone(),
    }
}

pub fn sub_kind(r: &ArtifactRef) -> Option<Sub> {
    let (_, sub) = r.0.split_once('#')?;
    parse_sub(sub).ok()
}

pub fn mint(root: &ArtifactRef, sub: Sub) -> Result<ArtifactRef, ParseError> {
    if root.0.contains('#') {
        return Err(ParseError::UnknownSubKind {
            sub: format_sub(&sub),
        });
    }
    parse(&format!("{}#{}", root.0, format_sub(&sub)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_scope_urns_round_trip_byte_identical() {
        for s in [
            "myelin://acme/issue/issue/ENG-1421",
            "myelin://acme/git/pr/4291",
            "myelin://acme/ci/run/01J0RUN",
            "myelin://acme/knowledge/page/7c2",
            "myelin://acme/chat/message/01J0MSG",
            "myelin://acme/identity/member/p-7",
            "myelin://acme/refs/edge/abc123",
            "myelin://acme/issue/initiative/PLAT-9",
        ] {
            let r = parse(s).expect("well-formed URN must parse");
            assert_eq!(format(&r), s, "round-trip must be byte-identical for `{s}`");
        }
    }

    #[test]
    fn every_sub_kind_parses_classifies_and_round_trips() {
        let cases: &[(&str, Sub)] = &[
            (
                "myelin://acme/git/pr/42#comment-c9",
                Sub::Comment("c9".into()),
            ),
            (
                "myelin://acme/chat/message/1#thread-t7",
                Sub::Thread("t7".into()),
            ),
            (
                "myelin://acme/chat/message/1#message-m3",
                Sub::Message("m3".into()),
            ),
            ("myelin://acme/knowledge/page/7#b9", Sub::Block("9".into())),
            (
                "myelin://acme/knowledge/page/7#hIntro",
                Sub::Heading("Intro".into()),
            ),
            (
                "myelin://acme/knowledge/row/7#row-r2",
                Sub::Row("r2".into()),
            ),
            (
                "myelin://acme/issue/issue/ENG-1#field-status",
                Sub::Field("status".into()),
            ),
            (
                "myelin://acme/git/ref/main#L42-L88",
                Sub::LineRange { start: 42, end: 88 },
            ),
            (
                "myelin://acme/ci/check/x#check-build",
                Sub::Check("build".into()),
            ),
            (
                "myelin://acme/git/repo/core#commit-deadbeef/check-build",
                Sub::CommitCheck {
                    commit_oid: "deadbeef".into(),
                    context: "build".into(),
                },
            ),
            (
                "myelin://acme/git/repo/core#commit-deadbeef/ci-result",
                Sub::CommitCiResult {
                    commit_oid: "deadbeef".into(),
                },
            ),
            ("myelin://acme/ci/run/01J#step-3", Sub::Step(3)),
        ];
        for (s, want_kind) in cases {
            let r = parse(s).unwrap_or_else(|e| panic!("`{s}` must parse: {e}"));
            assert_eq!(format(&r), *s, "round-trip for `{s}`");
            assert_eq!(sub_kind(&r).as_ref(), Some(want_kind), "kind for `{s}`");
        }
    }

    #[test]
    fn single_line_range_is_admitted() {
        let r = parse("myelin://acme/git/ref/main#L7-L7").unwrap();
        assert_eq!(sub_kind(&r), Some(Sub::LineRange { start: 7, end: 7 }));
    }

    #[test]
    fn issues_canonical_projectkey_seqno_is_the_stored_id() {
        let r = parse("myelin://acme/issue/issue/ENG-1421").unwrap();
        assert_eq!(format(&r), "myelin://acme/issue/issue/ENG-1421");
    }

    #[test]
    fn issues_short_display_key_and_other_display_projections_are_rejected() {
        for display in ["#1421", "#42", "@alice", "~general", "ENG-1421", "1234567"] {
            assert!(
                parse(display).is_err(),
                "display projection `{display}` must be rejected, never parsed as a scope"
            );
            assert_eq!(
                parse(display),
                Err(ParseError::MissingScheme {
                    input: display.to_string()
                })
            );
        }
    }

    #[test]
    fn incomplete_scope_is_rejected_never_guessed() {
        assert_eq!(
            parse("myelin://acme/issue/issue"),
            Err(ParseError::IncompleteScope { got_segments: 3 })
        );
        assert_eq!(
            parse("myelin://acme/issue"),
            Err(ParseError::IncompleteScope { got_segments: 2 })
        );
        assert_eq!(
            parse("myelin://acme"),
            Err(ParseError::IncompleteScope { got_segments: 1 })
        );
        assert_eq!(
            parse("myelin://acme/issue/issue/ENG/1421"),
            Err(ParseError::IncompleteScope { got_segments: 5 })
        );
    }

    #[test]
    fn empty_segments_are_rejected_with_the_segment_named() {
        assert_eq!(
            parse("myelin:///issue/issue/1"),
            Err(ParseError::EmptySegment { segment: "tenant" })
        );
        assert_eq!(
            parse("myelin://acme//issue/1"),
            Err(ParseError::EmptySegment {
                segment: "subsystem"
            })
        );
        assert_eq!(
            parse("myelin://acme/issue//1"),
            Err(ParseError::EmptySegment { segment: "type" })
        );
        assert_eq!(
            parse("myelin://acme/issue/issue/"),
            Err(ParseError::EmptySegment { segment: "id" })
        );
    }

    #[test]
    fn unknown_subsystem_or_type_token_is_rejected() {
        assert_eq!(
            parse("myelin://acme/billing/invoice/1"),
            Err(ParseError::UnknownSubsystem {
                token: "billing".into()
            })
        );
        assert_eq!(
            parse("myelin://acme/git/widget/1"),
            Err(ParseError::UnknownType {
                token: "widget".into()
            })
        );
    }

    #[test]
    fn unknown_or_malformed_sub_kind_is_rejected() {
        assert_eq!(parse("myelin://acme/git/pr/42#"), Err(ParseError::EmptySub));
        assert!(matches!(
            parse("myelin://acme/git/pr/42#widget-9"),
            Err(ParseError::UnknownSubKind { .. })
        ));
        assert!(matches!(
            parse("myelin://acme/git/pr/42#comment-"),
            Err(ParseError::UnknownSubKind { .. })
        ));
        assert!(matches!(
            parse("myelin://acme/ci/run/1#step-x"),
            Err(ParseError::UnknownSubKind { .. })
        ));
        assert!(matches!(
            parse("myelin://acme/git/ref/main#L88-L42"),
            Err(ParseError::UnknownSubKind { .. })
        ));
        assert!(matches!(
            parse("myelin://acme/git/ref/main#L42"),
            Err(ParseError::UnknownSubKind { .. })
        ));
        assert!(matches!(
            parse("myelin://acme/knowledge/page/7#b"),
            Err(ParseError::UnknownSubKind { .. })
        ));
    }

    #[test]
    fn parse_error_display_is_loud_and_names_the_rule() {
        let cases: &[(ParseError, &str)] = &[
            (
                ParseError::MissingScheme {
                    input: "#42".into(),
                },
                "does not start with the canonical scheme",
            ),
            (
                ParseError::IncompleteScope { got_segments: 2 },
                "scope is not total",
            ),
            (
                ParseError::EmptySegment { segment: "tenant" },
                "the `tenant` segment is empty",
            ),
            (
                ParseError::UnknownSubsystem {
                    token: "billing".into(),
                },
                "unknown subsystem token `billing`",
            ),
            (
                ParseError::UnknownType {
                    token: "widget".into(),
                },
                "unknown artifact-type token `widget`",
            ),
            (ParseError::EmptySub, "the sub-anchor is empty"),
            (
                ParseError::UnknownSubKind {
                    sub: "widget-9".into(),
                },
                "unknown/ambiguous `#sub` kind `widget-9`",
            ),
        ];
        for (err, needle) in cases {
            let rendered = err.to_string();
            assert!(
                rendered.contains(needle),
                "`{err:?}` Display must contain `{needle}`, got `{rendered}`"
            );
            assert!(
                rendered.len() > 16,
                "`{err:?}` Display must be a loud, non-trivial message, got `{rendered}`"
            );
        }
    }

    #[test]
    fn strip_sub_returns_the_root_and_is_idempotent() {
        let with_sub = parse("myelin://acme/git/pr/42#comment-c9").unwrap();
        let root = strip_sub(&with_sub);
        assert_eq!(format(&root), "myelin://acme/git/pr/42");
        assert_eq!(strip_sub(&root), root);
        assert_eq!(sub_kind(&root), None);
    }

    #[test]
    fn scoped_parse_returns_the_authoritative_tenant_and_components() {
        let parsed = parse_scoped("myelin://acme/git/pr/42#comment-c9").expect("canonical ref");
        assert_eq!(parsed.artifact_ref.0, "myelin://acme/git/pr/42#comment-c9");
        assert_eq!(parsed.tenant, TenantId("acme".into()));
        assert_eq!(parsed.subsystem, "git");
        assert_eq!(parsed.type_, "pr");
        assert_eq!(parsed.id, "42");
        assert_eq!(parsed.sub, Some(Sub::Comment("c9".into())));
        assert!(parse_scoped("https://acme/git/pr/42").is_err());
    }

    #[test]
    fn prefix_overlap_is_disambiguated_by_the_longer_kind() {
        let c = parse("myelin://acme/git/pr/42#comment-cabc").unwrap();
        assert_eq!(sub_kind(&c), Some(Sub::Comment("cabc".into())));
        let check = parse("myelin://acme/ci/check/x#check-lint").unwrap();
        assert_eq!(sub_kind(&check), Some(Sub::Check("lint".into())));
        let b = parse("myelin://acme/knowledge/page/7#bcomment").unwrap();
        assert_eq!(sub_kind(&b), Some(Sub::Block("comment".into())));
    }

    #[test]
    fn mint_attaches_a_sub_to_a_root_and_round_trips() {
        let root = parse("myelin://acme/git/pr/repo7:4291").unwrap();
        let r = mint(&root, Sub::Comment("c9".into())).unwrap();
        assert_eq!(format(&r), "myelin://acme/git/pr/repo7:4291#comment-c9");
        assert_eq!(sub_kind(&r), Some(Sub::Comment("c9".into())));
        assert_eq!(strip_sub(&r), root);
    }

    #[test]
    fn mint_rejects_a_malformed_opaque_body() {
        let root = parse("myelin://acme/git/pr/repo7:1").unwrap();
        assert!(matches!(
            mint(&root, Sub::Comment(String::new())),
            Err(ParseError::UnknownSubKind { .. })
        ));
        let blob = parse("myelin://acme/git/blob/r:main:f.rs").unwrap();
        assert!(matches!(
            mint(&blob, Sub::LineRange { start: 88, end: 42 }),
            Err(ParseError::UnknownSubKind { .. })
        ));
    }

    #[test]
    fn mint_refuses_a_sub_of_a_sub() {
        let already = parse("myelin://acme/git/pr/repo7:1#comment-c1").unwrap();
        assert!(matches!(
            mint(&already, Sub::Thread("t2".into())),
            Err(ParseError::UnknownSubKind { .. })
        ));
    }

    #[test]
    fn sub_kind_discriminator_and_label_are_frozen() {
        assert_eq!(Sub::Comment("x".into()).kind(), SubKind::Comment);
        assert_eq!(Sub::Thread("x".into()).kind(), SubKind::Thread);
        assert_eq!(
            Sub::CommitCheck {
                commit_oid: "abc".into(),
                context: "build".into(),
            }
            .kind(),
            SubKind::Check
        );
        assert_eq!(
            Sub::CommitCiResult {
                commit_oid: "abc".into(),
            }
            .kind(),
            SubKind::Check
        );
        assert_eq!(
            Sub::LineRange { start: 1, end: 2 }.kind(),
            SubKind::LineRange
        );
        assert_eq!(Sub::Step(3).kind(), SubKind::Step);
        assert_eq!(SubKind::Comment.label(), "comment-");
        assert_eq!(SubKind::LineRange.label(), "L<a>-L<b>");
    }

    #[test]
    fn fuzz_ambiguity_rejection_zero_guessed_scopes() {
        let mut guessed_scopes = 0usize;
        let mut round_trip_failures = 0usize;

        let schemes = ["myelin://", "https://", "", "myelin:/"];
        let bodies = [
            "",
            "acme",
            "acme/git",
            "acme/git/pr",
            "acme/git/pr/42",
            "acme/git/pr/42/extra",
            "acme/billing/pr/42",
            "acme/git/widget/42",
            "acme//pr/42",
            "/git/pr/42",
        ];
        let subs = [
            None,
            Some(""),
            Some("comment-c9"),
            Some("widget-9"),
            Some("L42-L88"),
            Some("L88-L42"),
            Some("step-3"),
            Some("step-x"),
            Some("b9"),
            Some("b"),
        ];

        for scheme in schemes {
            for body in bodies {
                for sub in subs {
                    let input = match sub {
                        Some(s) => format!("{scheme}{body}#{s}"),
                        None => format!("{scheme}{body}"),
                    };
                    match parse(&input) {
                        Ok(r) => {
                            assert!(
                                input.starts_with(SCHEME),
                                "guessed a scope for a scheme-less input `{input}`"
                            );
                            if format(&r) != input {
                                round_trip_failures += 1;
                            }
                            let re = parse(&format(&r)).expect("canonical re-parses");
                            assert_eq!(re, r, "canonical form is not a fixed point for `{input}`");
                        }
                        Err(_) => {
                        }
                    }
                }
            }
        }

        for display in ["#42", "#1421", "@alice", "~general", "ENG-1421", "abc1234"] {
            if parse(display).is_ok() {
                guessed_scopes += 1;
            }
        }

        assert_eq!(
            guessed_scopes, 0,
            "a display projection was guessed into a scope"
        );
        assert_eq!(round_trip_failures, 0, "a parsed URN failed to round-trip");
    }
}
