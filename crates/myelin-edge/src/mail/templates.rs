//! What Myelin writes to people. Each letter is short plain text with a simple HTML twin: no
//! images, no tracking, and any link written out in full.

use serde::{Deserialize, Serialize};

/// One email, as queued. It holds the recipient and any link, so it is sealed at rest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "letter", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Letter {
    Invitation {
        to: String,
        organization: String,
        link: String,
        expires_at_unix: i64,
    },
    PasswordReset {
        to: String,
        link: String,
        expires_at_unix: i64,
    },
    PasswordChanged {
        to: String,
        /// Where to ask for a reset; absent when the web app's origin is not set.
        reset: Option<String>,
    },
}

pub(crate) struct Rendered {
    pub subject: String,
    pub text: String,
    pub html: String,
}

impl Letter {
    pub(crate) fn to(&self) -> &str {
        match self {
            Self::Invitation { to, .. }
            | Self::PasswordReset { to, .. }
            | Self::PasswordChanged { to, .. } => to,
        }
    }

    pub(crate) fn render(&self) -> Rendered {
        match self {
            Self::Invitation {
                organization,
                link,
                expires_at_unix,
                ..
            } => compose(
                format!("Join {organization} on Myelin"),
                &[
                    Line::Text(format!(
                        "You've been invited to join {organization} on Myelin."
                    )),
                    Line::Text("Open this link to choose your name and a password:".into()),
                    Line::Link(link),
                    Line::Text(format!(
                        "The link works once and expires on {}. If you weren't expecting this, \
                         you can ignore this email.",
                        when(*expires_at_unix)
                    )),
                ],
            ),
            Self::PasswordReset {
                link,
                expires_at_unix,
                ..
            } => compose(
                "Reset your Myelin password".into(),
                &[
                    Line::Text(
                        "Someone asked to reset the Myelin password for this email address.".into(),
                    ),
                    Line::Text("Open this link to choose a new password:".into()),
                    Line::Link(link),
                    Line::Text(format!(
                        "The link works once and expires on {}. If you didn't ask for this, \
                         ignore this email; your password stays as it is.",
                        when(*expires_at_unix)
                    )),
                ],
            ),
            Self::PasswordChanged { to, reset } => {
                let mut lines = vec![Line::Text(format!(
                    "The password for your Myelin account, {to}, was just changed. All other \
                     sessions were signed out."
                ))];
                match reset {
                    Some(reset) => {
                        lines.push(Line::Text(
                            "If this wasn't you, reset your password:".into(),
                        ));
                        lines.push(Line::Link(reset));
                    }
                    None => lines.push(Line::Text(
                        "If this wasn't you, ask whoever runs your Myelin deployment to reset \
                         your password."
                            .into(),
                    )),
                }
                compose("Your Myelin password was changed".into(), &lines)
            }
        }
    }
}

enum Line<'a> {
    Text(String),
    Link(&'a str),
}

fn compose(subject: String, lines: &[Line<'_>]) -> Rendered {
    let text = lines
        .iter()
        .map(|line| match line {
            Line::Text(text) => text.as_str(),
            Line::Link(link) => link,
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let body = lines
        .iter()
        .map(|line| match line {
            Line::Text(text) => format!("<p>{}</p>\n", escape(text)),
            Line::Link(link) => {
                let link = escape(link);
                format!("<p><a href=\"{link}\">{link}</a></p>\n")
            }
        })
        .collect::<String>();
    let html = format!(
        "<!doctype html>\n<html><head><meta charset=\"utf-8\"><title>{}</title></head>\n\
         <body style=\"font-family:system-ui,sans-serif;font-size:15px;line-height:1.5;\
         color:#1a1a1a;max-width:560px;margin:24px auto;padding:0 16px\">\n{body}</body></html>\n",
        escape(&subject)
    );
    Rendered {
        subject,
        text: format!("{text}\n"),
        html,
    }
}

/// A moment in UTC, written for people: `2 December 2026 at 14:05 UTC`.
fn when(unix: i64) -> String {
    chrono::DateTime::from_timestamp(unix, 0).map_or_else(
        || "an unknown date".into(),
        |moment| moment.format("%-d %B %Y at %H:%M UTC").to_string(),
    )
}

fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            other => escaped.push(other),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINK: &str = "https://myelin.example/join/abc_DEF-123";

    #[test]
    fn an_invitation_names_the_organization_the_link_and_its_expiry() {
        let rendered = Letter::Invitation {
            to: "grace@example.com".into(),
            organization: "acme".into(),
            link: LINK.into(),
            expires_at_unix: 1_796_220_300,
        }
        .render();
        assert_eq!(rendered.subject, "Join acme on Myelin");
        assert!(rendered.text.contains(&format!("\n\n{LINK}\n\n")));
        assert!(rendered
            .text
            .contains("expires on 2 December 2026 at 14:05 UTC"));
        assert!(rendered
            .html
            .contains(&format!(r#"<a href="{LINK}">{LINK}</a>"#)));
        assert!(!rendered.html.contains("<img"));
    }

    #[test]
    fn html_never_carries_markup_from_the_data() {
        let rendered = Letter::Invitation {
            to: "grace@example.com".into(),
            organization: "<script>x</script>".into(),
            link: LINK.into(),
            expires_at_unix: 0,
        }
        .render();
        assert!(!rendered.html.contains("<script>"));
        assert!(rendered.html.contains("&lt;script&gt;"));
    }

    #[test]
    fn letters_round_trip_through_their_queued_form() {
        for letter in [
            Letter::PasswordReset {
                to: "ada@example.com".into(),
                link: LINK.into(),
                expires_at_unix: 1,
            },
            Letter::PasswordChanged {
                to: "ada@example.com".into(),
                reset: None,
            },
        ] {
            let queued = serde_json::to_vec(&letter).unwrap();
            assert_eq!(serde_json::from_slice::<Letter>(&queued).unwrap(), letter);
        }
    }

    #[test]
    fn the_change_notice_says_the_other_sessions_ended_and_where_to_reset() {
        let rendered = Letter::PasswordChanged {
            to: "ada@example.com".into(),
            reset: Some("https://myelin.example/login?help=password".into()),
        }
        .render();
        assert!(rendered.text.contains("ada@example.com"));
        assert!(rendered
            .text
            .contains("All other sessions were signed out."));
        assert!(rendered.text.contains(
            "If this wasn't you, reset your password:\n\nhttps://myelin.example/login?help=password"
        ));
    }
}
