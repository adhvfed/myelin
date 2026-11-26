//! `edge invite`, `edge reset-link` and `edge role`: the operator's way to hand a person a link
//! or give them a role. A link goes to stdout alone so a script can capture it; everything else
//! goes to stderr.

use myelin_edge::{AccountError, AccountService, OrgRole};

pub(crate) enum AccountCommand {
    Join,
    Reset,
    Role,
}

impl AccountCommand {
    pub(crate) fn named(name: &str) -> Option<Self> {
        match name {
            "invite" => Some(Self::Join),
            "reset-link" => Some(Self::Reset),
            "role" => Some(Self::Role),
            _ => None,
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Self::Join => "invite",
            Self::Reset => "reset-link",
            Self::Role => "role",
        }
    }

    fn usage(&self) -> &'static str {
        match self {
            Self::Join => {
                "--tenant T --email E [--role owner|admin|member] [--region R] [--ttl-hours N]"
            }
            Self::Reset => "--tenant T --email E [--region R] [--ttl-hours N]",
            Self::Role => "--tenant T --email E --role owner|admin|member [--region R]",
        }
    }

    fn takes(&self, flag: &str) -> bool {
        match flag {
            "--tenant" | "--email" | "--region" => true,
            "--ttl-hours" => !matches!(self, Self::Role),
            "--role" => !matches!(self, Self::Reset),
            _ => false,
        }
    }
}

struct Command {
    tenant: String,
    email: String,
    region: Option<String>,
    ttl_secs: Option<i64>,
    role: Option<OrgRole>,
}

impl Command {
    fn parse(command: &AccountCommand, args: &[String]) -> Result<Self, String> {
        let (mut tenant, mut email, mut region, mut ttl_hours, mut role) =
            (None, None, None, None, None);
        let mut pairs = args.chunks(2);
        for pair in pairs.by_ref() {
            let [name, value] = pair else {
                return Err(format!("{} requires a value", pair[0]));
            };
            if value.starts_with("--") {
                return Err(format!("{name} requires a value"));
            }
            if !command.takes(name) {
                return Err(format!("unknown flag `{name}`"));
            }
            let slot = match name.as_str() {
                "--tenant" => &mut tenant,
                "--email" => &mut email,
                "--region" => &mut region,
                "--ttl-hours" => &mut ttl_hours,
                _ => &mut role,
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
        let role = role
            .map(|role: String| {
                OrgRole::parse(&role).ok_or("--role must be owner, admin or member".to_string())
            })
            .transpose()?;
        if matches!(command, AccountCommand::Role) && role.is_none() {
            return Err("--role is required".into());
        }
        Ok(Self {
            tenant: tenant
                .filter(|value| !value.trim().is_empty())
                .ok_or("--tenant is required")?,
            email: email.ok_or("--email is required")?,
            region,
            ttl_secs,
            role,
        })
    }
}

pub(crate) async fn run(
    kind: AccountCommand,
    accounts: AccountService,
    cell_region: &str,
    args: &[String],
) {
    let name = kind.name();
    let command = Command::parse(&kind, args).unwrap_or_else(|error| {
        eprintln!("edge {name}: {error}");
        eprintln!("usage: edge {name} {}", kind.usage());
        std::process::exit(2);
    });
    if let Some(region) = command.region.as_deref() {
        if region != cell_region {
            eprintln!("edge {name}: this cell serves region `{cell_region}`, not `{region}`");
            std::process::exit(2);
        }
    }
    let fail = |error: AccountError| -> ! {
        eprintln!("edge {name}: {error}");
        std::process::exit(match error {
            AccountError::InvalidEmail => 2,
            _ => 1,
        });
    };
    let role = command.role.unwrap_or(OrgRole::Owner);
    let issued = match kind {
        AccountCommand::Join => {
            accounts
                .operator_join_link(&command.tenant, &command.email, command.ttl_secs, role)
                .await
        }
        AccountCommand::Reset => {
            accounts
                .operator_reset_link(&command.tenant, &command.email, command.ttl_secs)
                .await
        }
        AccountCommand::Role => {
            accounts
                .operator_set_role(&command.tenant, &command.email, role)
                .await
                .unwrap_or_else(|error| fail(error));
            eprintln!(
                "edge role: {} is now {} of tenant `{}`",
                command.email,
                role.as_str(),
                command.tenant
            );
            return;
        }
    };
    let issued = issued.unwrap_or_else(|error| fail(error));
    println!("{}", issued.url);
    eprintln!(
        "edge {name}: {} link for tenant `{}` in region `{cell_region}`, valid until unix {}",
        match kind {
            AccountCommand::Join => format!("join (as {})", role.as_str()),
            _ => "password reset".to_string(),
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
        let command = Command::parse(
            &AccountCommand::Join,
            &args(&["--tenant", "acme", "--email", "a@b.c", "--ttl-hours", "2"]),
        )
        .unwrap();
        assert_eq!(command.tenant, "acme");
        assert_eq!(command.ttl_secs, Some(7200));
        assert_eq!(command.role, None, "an operator's invitation grants owner");
        for invalid in [
            &["--tenant", "acme"][..],
            &["--email", "a@b.c"],
            &["--tenant", "acme", "--email"],
            &["--tenant", "acme", "--email", "--region"],
            &["--tenant", "acme", "--email", "a@b.c", "--tenant", "other"],
            &["--tenant", "acme", "--email", "a@b.c", "--role", "root"],
            &["--tenant", "acme", "--email", "a@b.c", "--ttl-hours", "0"],
            &["--tenant", "acme", "--email", "a@b.c", "--ttl-hours", "721"],
        ] {
            assert!(
                Command::parse(&AccountCommand::Join, &args(invalid)).is_err(),
                "accepted {invalid:?}"
            );
        }
    }

    #[test]
    fn each_command_takes_only_its_own_flags() {
        let member = Command::parse(
            &AccountCommand::Join,
            &args(&["--tenant", "acme", "--email", "a@b.c", "--role", "member"]),
        )
        .unwrap();
        assert_eq!(member.role, Some(OrgRole::Member));
        let role = Command::parse(
            &AccountCommand::Role,
            &args(&["--tenant", "acme", "--email", "a@b.c", "--role", "admin"]),
        )
        .unwrap();
        assert_eq!(role.role, Some(OrgRole::Admin));
        for (command, invalid) in [
            (
                AccountCommand::Reset,
                &["--tenant", "acme", "--email", "a@b.c", "--role", "admin"][..],
            ),
            (
                AccountCommand::Role,
                &["--tenant", "acme", "--email", "a@b.c"],
            ),
            (
                AccountCommand::Role,
                &[
                    "--tenant",
                    "acme",
                    "--email",
                    "a@b.c",
                    "--role",
                    "admin",
                    "--ttl-hours",
                    "1",
                ],
            ),
        ] {
            assert!(
                Command::parse(&command, &args(invalid)).is_err(),
                "edge {} accepted {invalid:?}",
                command.name()
            );
        }
    }
}
