use myelin_events::{
    AggregateKey, ArtifactRef, DataRole, EventDraft, EventEnvelope, EventId, EventType, OutboxTx,
    Result as BusResult, Visibility,
};

pub const REFS_EDGE_CREATED: &str = "refs.edge.created";

pub const REL_CLASS_LIFECYCLE: &str = "lifecycle";

pub const MAX_CLOSES_MESSAGE_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LifecycleRel {
    Closes,
    Relates,
}

impl LifecycleRel {
    pub fn as_str(self) -> &'static str {
        match self {
            LifecycleRel::Closes => "closes",
            LifecycleRel::Relates => "relates",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LifecycleEdge {
    pub source: ArtifactRef,
    pub target: ArtifactRef,
    pub rel: LifecycleRel,
}

pub fn edge_aggregate_key(source: &ArtifactRef, target: &ArtifactRef) -> AggregateKey {
    myelin_refs::edge_aggregate_key(source, target)
}

pub fn parse_closes_trailers(message: &str) -> Result<Vec<String>, TrailerParseError> {
    const MAX_KEYS: usize = 100;
    const MAX_KEY_BYTES: usize = 256;
    const MAX_TOTAL_KEY_BYTES: usize = 8 * 1024;
    if message.len() > MAX_CLOSES_MESSAGE_BYTES {
        return Err(TrailerParseError::LimitExceeded("message bytes"));
    }
    let mut keys: Vec<String> = Vec::new();
    let mut total_key_bytes = 0usize;
    for raw in message.lines() {
        let line = raw.trim();
        let rest = match strip_closes_keyword(line) {
            Some(rest) => rest,
            None => continue,
        };
        for tok in rest.split([',', ' ', '\t']) {
            let key = tok.trim();
            if key.is_empty() {
                continue;
            }
            if !keys.iter().any(|k| k == key) {
                if key.len() > MAX_KEY_BYTES {
                    return Err(TrailerParseError::LimitExceeded("issue key bytes"));
                }
                if keys.len() >= MAX_KEYS {
                    return Err(TrailerParseError::LimitExceeded("issue key count"));
                }
                total_key_bytes = total_key_bytes
                    .checked_add(key.len())
                    .ok_or(TrailerParseError::LimitExceeded("total issue key bytes"))?;
                if total_key_bytes > MAX_TOTAL_KEY_BYTES {
                    return Err(TrailerParseError::LimitExceeded("total issue key bytes"));
                }
                keys.push(key.to_string());
            }
        }
    }
    Ok(keys)
}

pub fn closes_issue_targets(
    tenant: &str,
    message: &str,
) -> Result<Vec<ArtifactRef>, TrailerParseError> {
    Ok(parse_closes_trailers(message)?
        .into_iter()
        .filter_map(|key| canonical_issue_key(&key))
        .map(|key| ArtifactRef(format!("myelin://{tenant}/issue/issue/{key}")))
        .collect())
}

fn canonical_issue_key(value: &str) -> Option<String> {
    let value = value.to_ascii_uppercase();
    let (prefix, sequence) = value.rsplit_once('-')?;
    if prefix.is_empty()
        || prefix.len() > 32
        || !prefix
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return None;
    }
    let sequence_number = sequence.parse::<u64>().ok()?;
    if sequence_number == 0 || sequence_number.to_string() != sequence {
        return None;
    }
    Some(value)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrailerParseError {
    LimitExceeded(&'static str),
}

impl std::fmt::Display for TrailerParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LimitExceeded(kind) => write!(f, "Closes trailer {kind} limit exceeded"),
        }
    }
}

impl std::error::Error for TrailerParseError {}

fn strip_closes_keyword(line: &str) -> Option<&str> {
    const KW: &str = "closes";
    if line.len() < KW.len() {
        return None;
    }
    let (head, rest) = line.split_at(KW.len());
    if !head.eq_ignore_ascii_case(KW) {
        return None;
    }
    let rest = rest.strip_prefix(':').unwrap_or(rest);
    let trimmed = rest.trim_start();
    if rest.len() == trimmed.len() && !rest.is_empty() {
        return None;
    }
    Some(trimmed)
}

pub fn extract_lifecycle_edges(
    source: &ArtifactRef,
    closes_targets: &[ArtifactRef],
    relates_targets: &[ArtifactRef],
) -> Vec<LifecycleEdge> {
    let mut edges = Vec::with_capacity(closes_targets.len() + relates_targets.len());
    for target in closes_targets {
        edges.push(LifecycleEdge {
            source: source.clone(),
            target: target.clone(),
            rel: LifecycleRel::Closes,
        });
    }
    for target in relates_targets {
        edges.push(LifecycleEdge {
            source: source.clone(),
            target: target.clone(),
            rel: LifecycleRel::Relates,
        });
    }
    edges
}

fn edge_event_draft(edge: &LifecycleEdge) -> EventDraft {
    EventDraft {
        type_: EventType(REFS_EDGE_CREATED.into()),
        subject: edge.source.clone(),
        aggregate: edge_aggregate_key(&edge.source, &edge.target),
        payload: serde_json::json!({
            "source": edge.source.0,
            "target": edge.target.0,
            "rel": edge.rel.as_str(),
            "rel_class": REL_CLASS_LIFECYCLE,
        }),
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

pub fn emit_lifecycle_edges(
    tx: &mut dyn OutboxTx,
    source: &ArtifactRef,
    closes_targets: &[ArtifactRef],
    relates_targets: &[ArtifactRef],
    lifecycle_event: &EventEnvelope,
) -> BusResult<Vec<EventId>> {
    let edges = extract_lifecycle_edges(source, closes_targets, relates_targets);
    let mut ids = Vec::with_capacity(edges.len());
    for edge in &edges {
        let id = tx.emit(edge_event_draft(edge), Some(lifecycle_event))?;
        ids.push(id);
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr_source() -> ArtifactRef {
        crate::project::git_pr_ref("acme", "repo7", 42).unwrap()
    }

    fn issue(key: &str) -> ArtifactRef {
        ArtifactRef(format!("myelin://acme/issue/issue/{key}"))
    }

    #[test]
    fn closes_trailer_is_line_leading_not_mid_sentence() {
        let msg = "Fix the charge bug\n\nThis closes a long-standing race.\nCloses ENG-1\n";
        let keys = parse_closes_trailers(msg).unwrap();
        assert_eq!(
            keys,
            vec!["ENG-1".to_string()],
            "only the trailer line, not the prose `closes`"
        );
    }

    #[test]
    fn closes_trailer_colon_caseless_multikey_dedup() {
        let msg = "title\n\ncloses: ENG-1, ENG-2\nCLOSES ENG-2\nCloses ENG-3\n";
        let keys = parse_closes_trailers(msg).unwrap();
        assert_eq!(
            keys,
            vec![
                "ENG-1".to_string(),
                "ENG-2".to_string(),
                "ENG-3".to_string()
            ],
            "colon/caseless/multikey parsed; the duplicate ENG-2 is de-duplicated (0 dup)"
        );
    }

    #[test]
    fn closes_without_key_or_undelimited_is_not_a_trailer() {
        assert!(
            parse_closes_trailers("Closes\n").unwrap().is_empty(),
            "a bare `Closes` yields no key"
        );
        assert!(
            parse_closes_trailers("Closes   \n").unwrap().is_empty(),
            "`Closes` + whitespace only, no key"
        );
        assert!(
            parse_closes_trailers("Closesthebug now\n")
                .unwrap()
                .is_empty(),
            "an undelimited `Closesthebug` is NOT the trailer keyword"
        );
    }

    #[test]
    fn closes_targets_keep_only_canonical_issue_keys_and_normalize_human_case() {
        let targets = closes_issue_targets(
            "acme",
            "Closes eng-41, ENG-0042, this-discussion, ENG-0\nCloses PLATFORM-7\n",
        )
        .unwrap();
        assert_eq!(
            targets,
            vec![issue("ENG-41"), issue("PLATFORM-7")],
            "prose and non-canonical sequence spellings never become dangling graph targets"
        );
    }

    #[test]
    fn closes_trailer_parser_bounds_message_keys_and_key_bytes() {
        let exact_keys = (0..100)
            .map(|index| format!("Closes ENG-{index}\n"))
            .collect::<String>();
        assert_eq!(parse_closes_trailers(&exact_keys).unwrap().len(), 100);
        assert!(parse_closes_trailers(&(exact_keys + "Closes ENG-100\n")).is_err());

        let exact_key = "x".repeat(256);
        assert_eq!(
            parse_closes_trailers(&format!("Closes {exact_key}"))
                .unwrap()
                .len(),
            1
        );
        assert!(parse_closes_trailers(&format!("Closes {}", "x".repeat(257))).is_err());
        assert!(parse_closes_trailers(&"x".repeat(64 * 1024 + 1)).is_err());

        let aggregate_over = (0..33)
            .map(|index| format!("Closes {index:03}{}\n", "x".repeat(253)))
            .collect::<String>();
        assert!(parse_closes_trailers(&aggregate_over).is_err());
    }

    #[test]
    fn each_linkage_yields_one_lifecycle_edge_with_correct_rel_and_target() {
        let src = pr_source();
        let closes = vec![issue("ENG-1"), issue("ENG-2")];
        let relates = vec![crate::project::git_pr_ref("acme", "repo7", 7).unwrap()];
        let edges = extract_lifecycle_edges(&src, &closes, &relates);
        assert_eq!(
            edges.len(),
            3,
            "2 trailers + 1 PR-link → exactly 3 lifecycle edges"
        );

        assert_eq!(edges[0].rel, LifecycleRel::Closes);
        assert_eq!(edges[0].rel.as_str(), "closes");
        assert_eq!(edges[0].source, src);
        assert_eq!(edges[0].target, issue("ENG-1"));
        assert_eq!(edges[1].rel, LifecycleRel::Closes);
        assert_eq!(edges[1].target, issue("ENG-2"));

        assert_eq!(edges[2].rel, LifecycleRel::Relates);
        assert_eq!(edges[2].rel.as_str(), "relates");
        assert_eq!(edges[2].source, src);
        assert_eq!(
            edges[2].target,
            crate::project::git_pr_ref("acme", "repo7", 7).unwrap()
        );
    }

    #[test]
    fn merged_pr_without_linkage_yields_zero_edges() {
        let edges = extract_lifecycle_edges(&pr_source(), &[], &[]);
        assert!(
            edges.is_empty(),
            "a plain merge with no trailer/link produces 0 lifecycle edges"
        );
    }

    #[test]
    fn edge_event_draft_is_refs_edge_created_lifecycle_class() {
        let src = pr_source();
        let target = issue("ENG-1");
        let edge = LifecycleEdge {
            source: src.clone(),
            target: target.clone(),
            rel: LifecycleRel::Closes,
        };
        let draft = edge_event_draft(&edge);
        assert_eq!(draft.type_.0, "refs.edge.created");
        assert_eq!(draft.subject, src, "the subject is the referencing PR");
        assert_eq!(draft.payload["source"], src.0);
        assert_eq!(draft.payload["target"], target.0);
        assert_eq!(draft.payload["rel"], "closes");
        assert_eq!(
            draft.payload["rel_class"], "lifecycle",
            "a lifecycle mirror edge is lifecycle-class"
        );
        assert_eq!(draft.aggregate, edge_aggregate_key(&src, &target));
        assert!(
            !draft.contains_personal_data,
            "references-not-payloads: no inline PII"
        );
        assert!(draft.pii_key_ref.is_none());
        assert_eq!(draft.data_role, DataRole::Controller);
    }

    #[test]
    fn frozen_tokens_match_the_refs_mirror_wire_shape() {
        assert_eq!(REFS_EDGE_CREATED, "refs.edge.created");
        assert_eq!(REL_CLASS_LIFECYCLE, "lifecycle");
        assert_eq!(LifecycleRel::Closes.as_str(), "closes");
        assert_eq!(LifecycleRel::Relates.as_str(), "relates");
        assert_ne!(REL_CLASS_LIFECYCLE, crate::body::REL_CLASS_REFERENCE);
    }

    #[test]
    fn lifecycle_edge_shares_the_content_edge_aggregate_convention() {
        let src = pr_source();
        let target = issue("ENG-1");
        assert_eq!(
            edge_aggregate_key(&src, &target),
            crate::body::edge_aggregate_key(&src, &target),
            "the lifecycle + content producers share ONE edge-aggregate convention"
        );
    }
}
