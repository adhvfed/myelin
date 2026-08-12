use super::{CliError, EdgeCall, FormQuery, HttpMethod, RetryPolicy};

const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 100;

pub fn refs_dispatch(args: &[&str]) -> Result<EdgeCall, CliError> {
    let (verb, rest) = args.split_first().ok_or_else(|| {
        CliError::Usage("no ref command given (try: links | backlinks <ArtifactRef>)".into())
    })?;
    match *verb {
        "links" => related(rest, "links"),
        "backlinks" => related(rest, "backlinks"),
        other => Err(CliError::Usage(format!(
            "unknown ref command `{other}` (try: links | backlinks <ArtifactRef>)"
        ))),
    }
}

fn related(args: &[&str], direction: &'static str) -> Result<EdgeCall, CliError> {
    let Some((reference, flags)) = args.split_first() else {
        return Err(CliError::Usage(format!(
            "`ref {direction}` needs one canonical <ArtifactRef>"
        )));
    };
    myelin_refs::parse_scoped(reference)
        .map_err(|error| CliError::Usage(format!("invalid ArtifactRef: {error}")))?;

    let mut limit = None;
    let mut cursor = None;
    let mut index = 0;
    while index < flags.len() {
        let flag = flags[index];
        let value = flags
            .get(index + 1)
            .ok_or_else(|| CliError::Usage(format!("`ref {direction} {flag}` needs a value")))?;
        match flag {
            "--limit" if limit.is_none() => {
                let parsed = value.parse::<usize>().map_err(|_| {
                    CliError::Usage(format!("ref {direction} limit must be an integer"))
                })?;
                if !(1..=MAX_LIMIT).contains(&parsed) {
                    return Err(CliError::Usage(format!(
                        "ref {direction} limit must be within 1..={MAX_LIMIT}"
                    )));
                }
                limit = Some(parsed);
            }
            "--cursor" if cursor.is_none() => {
                if !canonical_cursor(value) {
                    return Err(CliError::Usage(format!(
                        "ref {direction} cursor is not a canonical edge cursor"
                    )));
                }
                cursor = Some(*value);
            }
            "--limit" | "--cursor" => {
                return Err(CliError::Usage(format!(
                    "duplicate ref {direction} flag `{flag}`"
                )))
            }
            other => {
                return Err(CliError::Usage(format!(
                    "unknown ref {direction} flag `{other}`"
                )))
            }
        }
        index += 2;
    }

    let mut query = FormQuery::default();
    query.push("ref", reference);
    query.push("limit", &limit.unwrap_or(DEFAULT_LIMIT).to_string());
    if let Some(cursor) = cursor {
        query.push("cursor", cursor);
    }
    Ok(EdgeCall {
        method: HttpMethod::Get,
        path: format!("/v1/refs/{direction}"),
        query: Some(query.finish()),
        payload: None,
        idempotency_key: None,
        retry_policy: RetryPolicy::None,
    })
}

fn canonical_cursor(value: &str) -> bool {
    let hexadecimal = |bytes: &[u8]| {
        bytes
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    };
    if value.len() == 32 && hexadecimal(value.as_bytes()) {
        return true;
    }
    value
        .strip_prefix("blake3:")
        .is_some_and(|digest| digest.len() == 64 && hexadecimal(digest.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backlinks_carries_the_canonical_ref_without_shortening_it() {
        let call = refs_dispatch(&[
            "backlinks",
            "myelin://acme/issue/issue/ENG-41",
            "--limit",
            "7",
            "--cursor",
            "0123456789abcdef0123456789abcdef",
        ])
        .unwrap();
        assert_eq!(call.path, "/v1/refs/backlinks");
        assert_eq!(
            call.query.as_deref(),
            Some(
                "ref=myelin%3A%2F%2Facme%2Fissue%2Fissue%2FENG-41&limit=7&cursor=0123456789abcdef0123456789abcdef"
            )
        );
    }

    #[test]
    fn links_walks_visible_targets_and_accepts_the_stronger_cursor_generation() {
        let cursor = format!("blake3:{}", "a".repeat(64));
        let call = refs_dispatch(&[
            "links",
            "myelin://acme/knowledge/page/01J00000000000000000000000",
            "--cursor",
            &cursor,
        ])
        .unwrap();
        assert_eq!(call.path, "/v1/refs/links");
        let expected = format!(
            "ref=myelin%3A%2F%2Facme%2Fknowledge%2Fpage%2F01J00000000000000000000000&limit=50&cursor=blake3%3A{}",
            "a".repeat(64)
        );
        assert_eq!(call.query.as_deref(), Some(expected.as_str()));
    }

    #[test]
    fn backlinks_refuses_ambiguous_or_unbounded_inputs() {
        for args in [
            vec!["backlinks"],
            vec!["links"],
            vec!["backlinks", "ENG-41"],
            vec![
                "backlinks",
                "myelin://acme/issue/issue/ENG-41",
                "--limit",
                "0",
            ],
            vec![
                "backlinks",
                "myelin://acme/issue/issue/ENG-41",
                "--wat",
                "x",
            ],
        ] {
            assert!(refs_dispatch(&args).is_err(), "accepted {args:?}");
        }
    }
}
