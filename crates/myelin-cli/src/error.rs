use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliError {
    Usage(String),
    NotAuthenticated(String),
    Unauthorized(String),
    Edge {
        status: u16,
        code: String,
        message: String,
    },
    Transport(String),
    Unsupported(String),
    Config(String),
}

impl CliError {
    pub fn code(&self) -> i32 {
        match self {
            CliError::Usage(_) => 2,
            CliError::NotAuthenticated(_) | CliError::Unauthorized(_) => 3,
            CliError::Unsupported(_) => 4,
            CliError::Edge { .. } | CliError::Transport(_) | CliError::Config(_) => 1,
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CliError::Usage(m) => {
                f.write_str("usage error: ")?;
                write_terminal_safe(f, m)
            }
            CliError::NotAuthenticated(m) => {
                f.write_str("not authenticated: ")?;
                write_terminal_safe(f, m)?;
                f.write_str(
                    "\n  hint: run `myelin auth login` (or provide $MYELIN_TOKEN for automation)",
                )
            }
            CliError::Unauthorized(m) => {
                f.write_str("not authenticated / token invalid: ")?;
                write_terminal_safe(f, m)
            }
            CliError::Edge {
                status,
                code,
                message,
            } => {
                write!(f, "edge error ({status} ")?;
                write_terminal_safe(f, code)?;
                f.write_str("): ")?;
                write_terminal_safe(f, message)
            }
            CliError::Transport(m) => {
                f.write_str("could not reach the edge: ")?;
                write_terminal_safe(f, m)
            }
            CliError::Unsupported(m) => write_terminal_safe(f, m),
            CliError::Config(m) => {
                f.write_str("configuration error: ")?;
                write_terminal_safe(f, m)
            }
        }
    }
}

fn write_terminal_safe(f: &mut fmt::Formatter<'_>, value: &str) -> fmt::Result {
    for character in value.chars() {
        match character {
            '\n' => f.write_str("\\n")?,
            '\r' => f.write_str("\\r")?,
            '\t' => f.write_str("\\t")?,
            '\u{2028}' => f.write_str("\\u{2028}")?,
            '\u{2029}' => f.write_str("\\u{2029}")?,
            character if character.is_control() => {
                let codepoint = character as u32;
                if codepoint <= 0xff {
                    write!(f, "\\x{codepoint:02x}")?;
                } else {
                    write!(f, "\\u{{{codepoint:x}}}")?;
                }
            }
            character => write!(f, "{character}")?,
        }
    }
    Ok(())
}

impl std::error::Error for CliError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_are_the_stable_contract() {
        assert_eq!(CliError::Usage("x".into()).code(), 2);
        assert_eq!(CliError::NotAuthenticated("x".into()).code(), 3);
        assert_eq!(CliError::Unauthorized("x".into()).code(), 3);
        assert_eq!(CliError::Unsupported("x".into()).code(), 4);
        assert_eq!(
            CliError::Edge {
                status: 404,
                code: "not_found".into(),
                message: "no".into()
            }
            .code(),
            1
        );
        assert_eq!(CliError::Transport("x".into()).code(), 1);
        assert_eq!(CliError::Config("x".into()).code(), 1);
    }

    #[test]
    fn no_error_variant_carries_a_token_field() {
        let e = CliError::Unauthorized("bearer rejected".into());
        assert!(!e.to_string().to_lowercase().contains("paseto"));
        assert!(!e.to_string().contains("v4.public"));
    }

    #[test]
    fn edge_control_sequences_are_visible_ascii_not_terminal_instructions() {
        let error = CliError::Edge {
            status: 409,
            code: "conflict\u{1b}".into(),
            message: "retry\r\n\u{1b}]52;clipboard".into(),
        }
        .to_string();
        assert!(!error.contains('\u{1b}'));
        assert!(!error.contains('\r'));
        assert_eq!(error.lines().count(), 1);
        assert!(error.contains("conflict\\x1b"));
        assert!(error.contains("retry\\r\\n\\x1b]52;clipboard"));
    }
}
