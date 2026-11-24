//! `edge invite` and `edge reset-link`: the operator's way to hand a person a link. The link goes
//! to stdout alone so a script can capture it; everything else goes to stderr.

use myelin_edge::{AccountError, AccountService};

pub(crate) enum LinkKind {
    Join,
    Reset,
}

impl LinkKind {
    fn command(&self) -> &'static str {
        match self {
            Self::Join => "invite",
            Self::Reset => "reset-link",
        }
    }
}

struct Command {
    tenant: String,
    email: String,
    region: Option<String>,
    ttl_secs: Option<i64>,
}

impl Command {
    fn parse(args: &[String]) -> Result<Self, String> {
        let (mut tenant, mut email, mut region, mut ttl_hours) = (None, None, None, None);
        let mut pairs = args.chunks(2);
        for pair in pairs.by_ref() {
            let [name, value] = pair else {
                return Err(format!("{} requires a value", pair[0]));
            };
            if value.starts_with("--") {
                return Err(format!("{name} requires a value"));
            }
            let slot = match name.as_str() {
                "--tenant" => &mut tenant,
                "--email" => &mut email,
                "--region" => &mut region,
                "--ttl-hours" => &mut ttl_hours,
                other => return Err(format!("unknown flag `{other}`")),
            };
            if slot.replace(value.clone()).is_some() {
                return Err(format!("duplicate flag `{name}`"));
            }
        }
        let ttl_secs = ttl_hours
            .map(|hours: String| {
                hours
                    .parse::<i64>()
                    .ok()
                    .filter(|hours| (1..=720).contains(hours))
                    .map(|hours| hours * 3600)
                    .ok_or_else(|| "--ttl-hours must be an integer between 1 and 720".to_string())
            })
            .transpose()?;
        Ok(Self {
            tenant: tenant
                .filter(|value| !value.trim().is_empty())
                .ok_or("--tenant is required")?,
            email: email.ok_or("--email is required")?,
            region,
            ttl_secs,
        })
    }
}

pub(crate) async fn run(
    kind: LinkKind,
    accounts: AccountService,
    cell_region: &str,
    args: &[String],
) {
    let name = kind.command();
    let command = Command::parse(args).unwrap_or_else(|error| {
        eprintln!("edge {name}: {error}");
        eprintln!("usage: edge {name} --tenant T --email E [--region R] [--ttl-hours N]");
        std::process::exit(2);
    });
    if let Some(region) = command.region.as_deref() {
        if region != cell_region {
            eprintln!("edge {name}: this cell serves region `{cell_region}`, not `{region}`");
            std::process::exit(2);
        }
    }
    let issued = match kind {
        LinkKind::Join => {
            accounts
                .operator_join_link(&command.tenant, &command.email, command.ttl_secs)
                .await
        }
        LinkKind::Reset => {
            accounts
                .operator_reset_link(&command.tenant, &command.email, command.ttl_secs)
                .await
        }
    };
    let issued = issued.unwrap_or_else(|error| {
        eprintln!("edge {name}: {error}");
        std::process::exit(match error {
            AccountError::InvalidEmail => 2,
            _ => 1,
        });
    });
    println!("{}", issued.url);
    eprintln!(
        "edge {name}: {} link for tenant `{}` in region `{cell_region}`, valid until unix {}",
        match kind {
            LinkKind::Join => "join",
            LinkKind::Reset => "password reset",
        },
        command.tenant,
        issued.expires_at_unix
    );
    eprintln!("  send it to the person yourself; anyone holding it can use it once");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn flags_are_exact() {
        let command = Command::parse(&args(&[
            "--tenant",
            "acme",
            "--email",
            "a@b.c",
            "--ttl-hours",
            "2",
        ]))
        .unwrap();
        assert_eq!(command.tenant, "acme");
        assert_eq!(command.ttl_secs, Some(7200));
        for invalid in [
            &["--tenant", "acme"][..],
            &["--email", "a@b.c"],
            &["--tenant", "acme", "--email"],
            &["--tenant", "acme", "--email", "--region"],
            &["--tenant", "acme", "--email", "a@b.c", "--tenant", "other"],
            &["--tenant", "acme", "--email", "a@b.c", "--role", "admin"],
            &["--tenant", "acme", "--email", "a@b.c", "--ttl-hours", "0"],
            &["--tenant", "acme", "--email", "a@b.c", "--ttl-hours", "721"],
        ] {
            assert!(
                Command::parse(&args(invalid)).is_err(),
                "accepted {invalid:?}"
            );
        }
    }
}
