use std::sync::Arc;

use myelin_git::pg_pr_store::PgPrStore;
use myelin_storage::{KmsEngine, SubstrateProvider};
use serde_json::json;

const DEFAULT_BATCH_SIZE: u32 = 100;
const MAX_BATCH_SIZE: u32 = 1_000;

#[derive(Debug, PartialEq, Eq)]
struct Command {
    tenant: String,
    batch_size: u32,
}

impl Command {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut tenant = None;
        let mut batch_size = None;
        let mut index = 0;
        while index < args.len() {
            let name = args[index].as_str();
            let value = args
                .get(index + 1)
                .filter(|value| !value.starts_with("--"))
                .ok_or_else(|| format!("{name} requires a value"))?;
            let slot = match name {
                "--tenant" => &mut tenant,
                "--batch-size" => &mut batch_size,
                other => return Err(format!("unknown flag `{other}`")),
            };
            if slot.replace(value.clone()).is_some() {
                return Err(format!("duplicate flag `{name}`"));
            }
            index += 2;
        }

        let tenant = tenant
            .filter(|value| !value.is_empty() && value.trim() == value && value.len() <= 255)
            .ok_or_else(|| "--tenant is required and must name one tenant".to_string())?;
        let batch_size = batch_size
            .map(|value| {
                value
                    .parse::<u32>()
                    .ok()
                    .filter(|parsed| {
                        (1..=MAX_BATCH_SIZE).contains(parsed) && parsed.to_string() == value
                    })
                    .ok_or_else(|| {
                        format!(
                            "--batch-size must be a canonical integer between 1 and {MAX_BATCH_SIZE}"
                        )
                    })
            })
            .transpose()?
            .unwrap_or(DEFAULT_BATCH_SIZE);
        Ok(Self { tenant, batch_size })
    }
}

pub async fn run(
    provider: SubstrateProvider,
    kms: Arc<KmsEngine>,
    runtime: tokio::runtime::Handle,
    args: &[String],
) {
    let command = Command::parse(args).unwrap_or_else(|error| refuse(error, 2));
    let store =
        PgPrStore::new(provider, kms, runtime).unwrap_or_else(|error| refuse(error.to_string(), 1));
    let mut rows_rekeyed = 0_u64;
    let mut batches = 0_u64;
    loop {
        let batch = store
            .rekey_legacy_pr_text_batch(&command.tenant, command.batch_size)
            .unwrap_or_else(|error| refuse(error.to_string(), 1));
        if batch.rows_rekeyed == 0 {
            break;
        }
        rows_rekeyed = rows_rekeyed
            .checked_add(u64::from(batch.rows_rekeyed))
            .unwrap_or_else(|| refuse("rekey count overflowed".into(), 1));
        batches = batches
            .checked_add(1)
            .unwrap_or_else(|| refuse("batch count overflowed".into(), 1));
    }

    println!(
        "{}",
        json!({
            "git_pr_text_rekey": {
                "tenant": command.tenant,
                "batch_size": command.batch_size,
                "batches": batches,
                "rows_rekeyed": rows_rekeyed,
                "complete": true,
            }
        })
    );
}

fn refuse(message: String, exit_code: i32) -> ! {
    eprintln!("edge git-pr-text-rekey: {message}");
    std::process::exit(exit_code);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_tenant_and_a_bounded_canonical_batch_are_accepted() {
        assert_eq!(
            Command::parse(&[
                "--tenant".into(),
                "acme".into(),
                "--batch-size".into(),
                "250".into(),
            ])
            .unwrap(),
            Command {
                tenant: "acme".into(),
                batch_size: 250,
            }
        );
        assert_eq!(
            Command::parse(&["--tenant".into(), "acme".into()])
                .unwrap()
                .batch_size,
            DEFAULT_BATCH_SIZE
        );
    }

    #[test]
    fn ambiguous_or_unbounded_rekey_commands_are_refused() {
        for args in [
            Vec::new(),
            vec!["--tenant".into(), " acme".into()],
            vec![
                "--tenant".into(),
                "acme".into(),
                "--tenant".into(),
                "other".into(),
            ],
            vec![
                "--tenant".into(),
                "acme".into(),
                "--batch-size".into(),
                "0".into(),
            ],
            vec![
                "--tenant".into(),
                "acme".into(),
                "--batch-size".into(),
                "010".into(),
            ],
            vec!["--surprise".into(), "value".into()],
        ] {
            assert!(Command::parse(&args).is_err(), "ambiguous args: {args:?}");
        }
    }
}
