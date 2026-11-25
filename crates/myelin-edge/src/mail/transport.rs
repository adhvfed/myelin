//! The SMTP relay. `MYELIN_SMTP_URL` names it; the username and password live in their own
//! variables so a logged URL never carries a secret. Plain SMTP may only reach this machine.

use std::time::Duration;

use lettre::message::Mailbox;
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::transport::smtp::Error as SmtpError;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use zeroize::Zeroizing;

use crate::web_origin::split_host_port;

const SEND_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Security {
    /// `smtp://` to a loopback host: no TLS at all.
    Plain,
    /// `smtp://…?tls=required`: STARTTLS, and the message is never sent without it.
    StartTls,
    /// `smtps://`: TLS from the first byte.
    Implicit,
}

/// Where and as whom mail is sent. Debug output never shows the password.
#[derive(Clone)]
pub struct MailSettings {
    host: String,
    port: u16,
    security: Security,
    credentials: Option<(String, Zeroizing<String>)>,
    from: Mailbox,
}

impl core::fmt::Debug for MailSettings {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("MailSettings")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("security", &self.security)
            .field("authenticated", &self.credentials.is_some())
            .field("from", &self.from.to_string())
            .finish()
    }
}

impl MailSettings {
    /// `None` when `MYELIN_SMTP_URL` is unset: mail is off.
    pub fn from_env() -> Result<Option<Self>, String> {
        let var = |name: &str| {
            std::env::var(name)
                .ok()
                .filter(|value| !value.trim().is_empty())
        };
        Self::parse(
            var("MYELIN_SMTP_URL"),
            var("MYELIN_SMTP_USERNAME"),
            var("MYELIN_SMTP_PASSWORD").map(Zeroizing::new),
            var("MYELIN_MAIL_FROM"),
        )
    }

    pub fn parse(
        url: Option<String>,
        username: Option<String>,
        password: Option<Zeroizing<String>>,
        from: Option<String>,
    ) -> Result<Option<Self>, String> {
        let Some(url) = url else {
            if username.is_some() || password.is_some() || from.is_some() {
                return Err(
                    "MYELIN_SMTP_USERNAME, MYELIN_SMTP_PASSWORD and MYELIN_MAIL_FROM take \
                            effect only with MYELIN_SMTP_URL; set it or unset them"
                        .into(),
                );
            }
            return Ok(None);
        };
        let (host, port, security) = parse_url(url.trim())?;
        let credentials = match (username, password) {
            (Some(username), Some(password)) => Some((username.trim().to_string(), password)),
            (None, None) => None,
            _ => {
                return Err(
                    "MYELIN_SMTP_USERNAME and MYELIN_SMTP_PASSWORD must be set together".into(),
                )
            }
        };
        let from = from.ok_or(
            "MYELIN_MAIL_FROM is required with MYELIN_SMTP_URL, e.g. `Myelin <no-reply@example.com>`",
        )?;
        let from: Mailbox = from.trim().parse().map_err(|_| {
            "MYELIN_MAIL_FROM must be an address such as `Myelin <no-reply@example.com>`"
                .to_string()
        })?;
        Ok(Some(Self {
            host,
            port,
            security,
            credentials,
            from,
        }))
    }
}

/// `smtp://host[:port]` (loopback only), `smtp://host[:port]?tls=required`, or
/// `smtps://host[:port]`. Nothing else may appear: no credentials, path, or other options.
fn parse_url(url: &str) -> Result<(String, u16, Security), String> {
    const SHAPE: &str = "MYELIN_SMTP_URL must look like smtp://127.0.0.1:1025, \
                         smtp://host:587?tls=required, or smtps://host:465";
    let (scheme, rest) = url.split_once("://").ok_or(SHAPE)?;
    let (rest, query) = match rest.split_once('?') {
        Some((rest, query)) => (rest, Some(query)),
        None => (rest, None),
    };
    let authority = rest.strip_suffix('/').unwrap_or(rest);
    if authority.contains(['@', '/', '#', '\\', '%']) {
        return Err(format!(
            "{SHAPE}; put the username and password in MYELIN_SMTP_USERNAME and \
             MYELIN_SMTP_PASSWORD, never in the URL"
        ));
    }
    let (host, port) = split_host_port(authority).ok_or(SHAPE)?;
    let security = match (scheme.to_ascii_lowercase().as_str(), query) {
        ("smtp", None) => Security::Plain,
        ("smtp", Some("tls=required")) => Security::StartTls,
        ("smtps", None) => Security::Implicit,
        _ => return Err(SHAPE.into()),
    };
    if security == Security::Plain && !host.is_loopback() {
        return Err(
            "MYELIN_SMTP_URL sends plain SMTP to another machine, which would expose links and \
             credentials; add ?tls=required for STARTTLS or use smtps:// for implicit TLS"
                .into(),
        );
    }
    let port = port.unwrap_or(match security {
        Security::Plain => 25,
        Security::StartTls => 587,
        Security::Implicit => 465,
    });
    Ok((host.unbracketed(), port, security))
}

/// What became of one send. The SMTP reply text is never kept: it can quote the recipient.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SendFailure {
    pub kind: &'static str,
    pub code: Option<String>,
}

impl From<SmtpError> for SendFailure {
    fn from(error: SmtpError) -> Self {
        let kind = if error.is_timeout() {
            "timeout"
        } else if error.is_tls() {
            "tls"
        } else if error.is_permanent() {
            "rejected"
        } else if error.is_transient() {
            "deferred"
        } else {
            "connection"
        };
        Self {
            kind,
            code: error.status().map(|code| code.to_string()),
        }
    }
}

#[derive(Clone)]
pub(crate) struct Mailer {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
}

impl Mailer {
    pub(crate) fn new(settings: &MailSettings) -> Result<Self, String> {
        let tls = |host: &str| {
            TlsParameters::new(host.to_string())
                .map_err(|_| format!("cannot prepare TLS for SMTP host `{host}`"))
        };
        let tls = match settings.security {
            Security::Plain => Tls::None,
            Security::StartTls => Tls::Required(tls(&settings.host)?),
            Security::Implicit => Tls::Wrapper(tls(&settings.host)?),
        };
        let mut builder = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&settings.host)
            .port(settings.port)
            .tls(tls)
            .timeout(Some(SEND_TIMEOUT));
        if let Some((username, password)) = &settings.credentials {
            builder = builder.credentials(Credentials::new(username.clone(), password.to_string()));
        }
        Ok(Self {
            transport: builder.build(),
            from: settings.from.clone(),
        })
    }

    pub(crate) fn from(&self) -> &Mailbox {
        &self.from
    }

    pub(crate) async fn send(&self, message: Message) -> Result<(), SendFailure> {
        self.transport
            .send(message)
            .await
            .map(drop)
            .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(url: &str) -> Result<Option<MailSettings>, String> {
        MailSettings::parse(
            Some(url.into()),
            None,
            None,
            Some("Myelin <no-reply@example.com>".into()),
        )
    }

    #[test]
    fn plain_smtp_only_reaches_this_machine() {
        for (url, host, port, security) in [
            ("smtp://127.0.0.1:1025", "127.0.0.1", 1025, Security::Plain),
            ("smtp://localhost", "localhost", 25, Security::Plain),
            ("smtp://[::1]:2525/", "::1", 2525, Security::Plain),
            (
                "smtp://smtp.tem.scw.cloud:2587?tls=required",
                "smtp.tem.scw.cloud",
                2587,
                Security::StartTls,
            ),
            (
                "smtp://smtp.tem.scw.cloud?tls=required",
                "smtp.tem.scw.cloud",
                587,
                Security::StartTls,
            ),
            (
                "SMTPS://smtp.tem.scw.cloud",
                "smtp.tem.scw.cloud",
                465,
                Security::Implicit,
            ),
        ] {
            let parsed = settings(url).unwrap().unwrap();
            assert_eq!(
                (parsed.host.as_str(), parsed.port, parsed.security),
                (host, port, security),
                "{url}"
            );
        }
        for refused in [
            "smtp://smtp.tem.scw.cloud:587",
            "smtp://10.0.0.5:25",
            "smtp://smtp.example?tls=opportunistic",
            "smtps://smtp.example?tls=required",
            "smtp://user:secret@smtp.example:587?tls=required",
            "smtp://smtp.example:587/path?tls=required",
            "http://127.0.0.1:1025",
            "127.0.0.1:1025",
            "smtp://:25",
        ] {
            assert!(settings(refused).is_err(), "accepted {refused}");
        }
    }

    #[test]
    fn a_refusal_never_repeats_a_secret_from_the_url() {
        let error = settings("smtp://user:hunter2@smtp.example:587").unwrap_err();
        assert!(!error.contains("hunter2"), "{error}");
    }

    #[test]
    fn mail_is_off_only_when_nothing_is_set() {
        assert!(MailSettings::parse(None, None, None, None)
            .unwrap()
            .is_none());
        assert!(MailSettings::parse(None, None, None, Some("a@b.c".into())).is_err());
        assert!(
            MailSettings::parse(Some("smtp://127.0.0.1:1025".into()), None, None, None).is_err()
        );
        assert!(MailSettings::parse(
            Some("smtp://127.0.0.1:1025".into()),
            Some("project".into()),
            None,
            Some("a@b.c".into())
        )
        .is_err());
        assert!(settings("smtp://127.0.0.1:1025").is_ok());
        assert!(MailSettings::parse(
            Some("smtp://127.0.0.1:1025".into()),
            None,
            None,
            Some("not an address".into())
        )
        .is_err());
    }

    #[test]
    fn debug_output_hides_the_password() {
        let parsed = MailSettings::parse(
            Some("smtps://smtp.tem.scw.cloud".into()),
            Some("project-id".into()),
            Some(Zeroizing::new("api-secret-key".into())),
            Some("Myelin <no-reply@example.com>".into()),
        )
        .unwrap()
        .unwrap();
        assert!(!format!("{parsed:?}").contains("api-secret-key"));
    }
}
