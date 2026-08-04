use crate::surfacing_store::{
    CiLogRangeRequest, CiRunPageRequest, CiRunStateFilter, CI_LOG_RANGE_DEFAULT,
    CI_RUN_CURSOR_PREFIX, CI_RUN_PAGE_DEFAULT,
};
use base64::Engine as _;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CliCommand {
    List(CiRunPageRequest),
    View { run_id: String },
    Logs {
        run_id: String,
        job_id: String,
        range: CiLogRangeRequest,
    },
    Watch { run_id: String, job_id: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CliParseError(String);

impl std::fmt::Display for CliParseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for CliParseError {}

pub fn parse_cli(args: &[&str]) -> Result<CliCommand, CliParseError> {
    let (verb, rest) = args.split_first().ok_or_else(|| {
        usage(
            "no CI command given (try: list | view <run> | logs <run> --job <job> | \
             watch <run> --job <job>)",
        )
    })?;
    match *verb {
        "list" => parse_list(rest),
        "view" | "show" => parse_view(verb, rest),
        "logs" => parse_logs(rest),
        "watch" => parse_watch(rest),
        other => Err(usage(format!(
            "unknown CI command `{other}` (try: list | view <run> | logs <run> --job <job> | \
             watch <run> --job <job>)"
        ))),
    }
}

fn parse_list(args: &[&str]) -> Result<CliCommand, CliParseError> {
    let mut state = None;
    let mut limit = None;
    let mut cursor = None;
    let mut index = 0;
    while index < args.len() {
        let flag = args[index];
        let value = args
            .get(index + 1)
            .ok_or_else(|| usage(format!("`ci list {flag}` needs a value")))?;
        match flag {
            "--status" if state.is_none() => {
                state = Some(CiRunStateFilter::parse(value).ok_or_else(|| {
                    usage(
                        "--status must be all, queued, running, succeeded, failed, cancelled, \
                         timed_out, or reaped",
                    )
                })?);
            }
            "--limit" if limit.is_none() => {
                limit = Some(parse_canonical_u32("--limit", value)?);
            }
            "--cursor" if cursor.is_none() => {
                validate_cursor(value)?;
                cursor = Some((*value).to_string());
            }
            "--status" | "--limit" | "--cursor" => {
                return Err(usage(format!("duplicate CI list flag `{flag}`")));
            }
            other => return Err(usage(format!("unknown CI list flag `{other}`"))),
        }
        index += 2;
    }
    let request = CiRunPageRequest::new(
        state.unwrap_or(CiRunStateFilter::All),
        limit.unwrap_or(CI_RUN_PAGE_DEFAULT),
        cursor,
    )
    .map_err(|error| usage(error.to_string()))?;
    Ok(CliCommand::List(request))
}

fn parse_view(verb: &str, args: &[&str]) -> Result<CliCommand, CliParseError> {
    if args.len() != 1 {
        return Err(usage(format!("`ci {verb}` needs exactly one <run>")));
    }
    validate_uuid("run", args[0])?;
    Ok(CliCommand::View {
        run_id: args[0].to_string(),
    })
}

fn parse_logs(args: &[&str]) -> Result<CliCommand, CliParseError> {
    let (run_id, flags) = args
        .split_first()
        .ok_or_else(|| usage("`ci logs` needs a <run> and `--job <job>`"))?;
    validate_uuid("run", run_id)?;
    let mut job_id = None;
    let mut start = None;
    let mut limit = None;
    let mut index = 0;
    while index < flags.len() {
        let flag = flags[index];
        let value = flags
            .get(index + 1)
            .ok_or_else(|| usage(format!("`ci logs {flag}` needs a value")))?;
        match flag {
            "--job" if job_id.is_none() => {
                validate_uuid("job", value)?;
                job_id = Some((*value).to_string());
            }
            "--start" if start.is_none() => {
                start = Some(parse_canonical_i64("--start", value)?);
            }
            "--limit" if limit.is_none() => {
                limit = Some(parse_canonical_u32("--limit", value)?);
            }
            "--job" | "--start" | "--limit" => {
                return Err(usage(format!("duplicate CI logs flag `{flag}`")));
            }
            other => return Err(usage(format!("unknown CI logs flag `{other}`"))),
        }
        index += 2;
    }
    let job_id = job_id.ok_or_else(|| usage("`ci logs` requires `--job <job>`"))?;
    let range = CiLogRangeRequest::new(start.unwrap_or(0), limit.unwrap_or(CI_LOG_RANGE_DEFAULT))
        .map_err(|error| usage(error.to_string()))?;
    Ok(CliCommand::Logs {
        run_id: (*run_id).to_string(),
        job_id,
        range,
    })
}

fn parse_watch(args: &[&str]) -> Result<CliCommand, CliParseError> {
    let (run_id, flags) = args
        .split_first()
        .ok_or_else(|| usage("`ci watch` needs a <run> and `--job <job>`"))?;
    validate_uuid("run", run_id)?;
    let mut job_id = None;
    let mut index = 0;
    while index < flags.len() {
        let flag = flags[index];
        let value = flags
            .get(index + 1)
            .ok_or_else(|| usage(format!("`ci watch {flag}` needs a value")))?;
        match flag {
            "--job" if job_id.is_none() => {
                validate_uuid("job", value)?;
                job_id = Some((*value).to_string());
            }
            "--job" => return Err(usage("duplicate CI watch flag `--job`")),
            other => return Err(usage(format!("unknown CI watch flag `{other}`"))),
        }
        index += 2;
    }
    Ok(CliCommand::Watch {
        run_id: (*run_id).to_string(),
        job_id: job_id.ok_or_else(|| usage("`ci watch` requires `--job <job>`"))?,
    })
}

fn parse_canonical_u32(flag: &str, value: &str) -> Result<u32, CliParseError> {
    let parsed = value
        .parse::<u32>()
        .map_err(|_| usage(format!("{flag} must be a canonical non-negative integer")))?;
    if parsed.to_string() != value {
        return Err(usage(format!(
            "{flag} must be a canonical non-negative integer"
        )));
    }
    Ok(parsed)
}

fn parse_canonical_i64(flag: &str, value: &str) -> Result<i64, CliParseError> {
    let parsed = value
        .parse::<i64>()
        .map_err(|_| usage(format!("{flag} must be a canonical integer")))?;
    if parsed.to_string() != value {
        return Err(usage(format!("{flag} must be a canonical integer")));
    }
    Ok(parsed)
}

fn validate_cursor(value: &str) -> Result<(), CliParseError> {
    let encoded = value
        .strip_prefix(CI_RUN_CURSOR_PREFIX)
        .filter(|encoded| !encoded.is_empty())
        .ok_or_else(|| usage("--cursor must be an opaque `cr1_` CI cursor"))?;
    let frame = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| usage("--cursor must be an opaque `cr1_` CI cursor"))?;
    if value.len() > 256
        || frame.len() != 60
        || frame[0] != 1
        || base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(frame) != encoded
    {
        return Err(usage("--cursor must be an opaque `cr1_` CI cursor"));
    }
    Ok(())
}

fn validate_uuid(kind: &str, value: &str) -> Result<(), CliParseError> {
    if value.len() != 36
        || !value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte),
        })
    {
        return Err(usage(format!(
            "CI {kind} id must be a canonical lowercase UUID"
        )));
    }
    Ok(())
}

fn usage(message: impl Into<String>) -> CliParseError {
    CliParseError(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN: &str = "91000000-0000-4000-8000-000000000001";
    const JOB: &str = "92000000-0000-4000-8000-000000000001";

    fn canonical_cursor() -> String {
        let mut frame = [0_u8; 60];
        frame[0] = 1;
        format!(
            "cr1_{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(frame)
        )
    }

    #[test]
    fn parses_exact_durable_read_commands() {
        assert_eq!(
            parse_cli(&["list"]).unwrap(),
            CliCommand::List(
                CiRunPageRequest::new(CiRunStateFilter::All, CI_RUN_PAGE_DEFAULT, None).unwrap()
            )
        );
        let cursor = canonical_cursor();
        assert_eq!(
            parse_cli(&["list", "--status", "failed", "--limit", "1", "--cursor", &cursor])
                .unwrap(),
            CliCommand::List(
                CiRunPageRequest::new(CiRunStateFilter::Failed, 1, Some(cursor)).unwrap()
            )
        );
        assert_eq!(
            parse_cli(&["show", RUN]).unwrap(),
            CliCommand::View { run_id: RUN.into() }
        );
        assert_eq!(
            parse_cli(&["logs", RUN, "--job", JOB, "--start", "9", "--limit", "7"]).unwrap(),
            CliCommand::Logs {
                run_id: RUN.into(),
                job_id: JOB.into(),
                range: CiLogRangeRequest::new(9, 7).unwrap(),
            }
        );
        assert_eq!(
            parse_cli(&["watch", RUN, "--job", JOB]).unwrap(),
            CliCommand::Watch {
                run_id: RUN.into(),
                job_id: JOB.into(),
            }
        );
    }

    #[test]
    fn rejects_noncanonical_or_ambiguous_input() {
        for invalid in [
            vec!["view", "NOT-A-UUID"],
            vec!["view", RUN, RUN],
            vec!["list", "--status", "passed"],
            vec!["list", "--limit", "01"],
            vec!["list", "--cursor", "1"],
            vec!["list", "--cursor", "cr1_Abc-_09"],
            vec!["list", "--cursor", "cr1_bad;command"],
            vec!["list", "--status", "all", "--status", "failed"],
            vec!["logs", RUN],
            vec!["logs", RUN, "--job", "NOT-A-UUID"],
            vec!["logs", RUN, "--job", JOB, "--start", "-1"],
            vec!["logs", RUN, "--job", JOB, "--limit", "0"],
            vec!["watch", RUN],
            vec!["watch", RUN, "--job", "NOT-A-UUID"],
            vec!["watch", RUN, "--job", JOB, "--job", JOB],
            vec!["watch", RUN, "--job", JOB, "--cursor", "1"],
        ] {
            assert!(parse_cli(&invalid).is_err(), "{invalid:?} must be refused");
        }
    }
}
