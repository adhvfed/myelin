use super::{CliError, EdgeCall, FormQuery, HttpMethod, RetryPolicy};

const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 100;

pub fn refs_dispatch(args: &[&str]) -> Result<EdgeCall, CliError> {
    let (verb, rest) = args.split_first().ok_or_else(|| {
        CliError::Usage("no ref command given (try: backlinks <ArtifactRef>)".into())
    })?;
    match *verb {
        "backlinks" => backlinks(rest),
        other => Err(CliError::Usage(format!(
            "unknown ref command `{other}` (try: backlinks <ArtifactRef>)"
        ))),
    }
}

fn backlinks(args: &[&str]) -> Result<EdgeCall, CliError> {
    let Some((reference, flags)) = args.split_first() else {
        return Err(CliError::Usage(
            "`ref backlinks` needs one canonical <ArtifactRef>".into(),
        ));
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
            .ok_or_else(|| CliError::Usage(format!("`ref backlinks {flag}` needs a value")))?;
        match flag {
            "--limit" if limit.is_none() => {
                let parsed = value.parse::<usize>().map_err(|_| {
                    CliError::Usage("ref backlinks limit must be an integer".into())
                })?;
                if !(1..=MAX_LIMIT).contains(&parsed) {
                    return Err(CliError::Usage(format!(
                        "ref backlinks limit must be within 1..={MAX_LIMIT}"
                    )));
                }
                limit = Some(parsed);
            }
            "--cursor" if cursor.is_none() => {
                if !canonical_cursor(value) {
                    return Err(CliError::Usage(
                        "ref backlinks cursor is not a canonical edge cursor".into(),
                    ));
                }
                cursor = Some(*value);
            }
            "--limit" | "--cursor" => {
                return Err(CliError::Usage(format!(
                    "duplicate ref backlinks flag `{flag}`"
                )))
            }
            other => {
                return Err(CliError::Usage(format!(
                    "unknown ref backlinks flag `{other}`"
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
        path: "/v1/refs/backlinks".into(),
        query: Some(query.finish()),
        payload: None,
        idempotency_key: None,
        retry_policy: RetryPolicy::None,
    })
}

fn canonical_cursor(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
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
    fn backlinks_refuses_ambiguous_or_unbounded_inputs() {
        for args in [
            vec!["backlinks"],
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
