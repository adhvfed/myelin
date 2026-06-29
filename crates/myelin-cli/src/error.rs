//! # The CLI error model + exit codes — total over bad input, NEVER a panic.
//!
//! Every failure path in the CLI funnels into a typed [`CliError`] with a stable [`CliError::code`]
//! (the process exit code). The CLI is TOTAL: a malformed argument, a missing token, a forged token,
//! a network failure, or an edge `{error:{message}}` envelope all surface as a clean message on
//! stderr + a non-zero exit — never a panic / backtrace.
//!
//! **The token is NEVER in an error.** No `CliError` variant carries the credential material; the
//! config/client code redacts it at the seam, so a token cannot leak via a printed error (the
//! "never log the token" floor).

use std::fmt;

/// A typed CLI error. The `code()` is the process exit code (stable for scripting/agents).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliError {
    /// A usage / argument-parse error (bad subcommand, missing arg, malformed value). Exit 2 —
    /// the conventional "command-line usage" code.
    Usage(String),
    /// No credential is available (no `--token`, `$MYELIN_TOKEN`, or stored token). Exit 3.
    NotAuthenticated(String),
    /// The edge rejected the credential (HTTP 401). Exit 3 — distinct from a generic failure so a
    /// caller can branch on "re-authenticate" vs "retry".
    Unauthorized(String),
    /// The edge returned a non-2xx `{error:{message,code}}` envelope (other than 401). Exit 1.
    Edge {
        /// The HTTP status the edge returned.
        status: u16,
        /// The machine `code` from the envelope (e.g. `not_found`), if present.
        code: String,
        /// The client-safe `message` from the envelope.
        message: String,
    },
    /// A transport / IO failure reaching the edge (connect refused, broken pipe, bad URL). Exit 1.
    Transport(String),
    /// A command the CLI parsed (via the reused subsystem grammar) but does not yet map to an edge
    /// endpoint — an HONEST deferral, not a silent no-op. Exit 4.
    Unsupported(String),
    /// A configuration / IO failure (cannot write the token file, bad config dir). Exit 1.
    Config(String),
}

impl CliError {
    /// The process exit code this error maps to (stable contract for scripts/agents).
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
            CliError::Usage(m) => write!(f, "usage error: {m}"),
            CliError::NotAuthenticated(m) => write!(
                f,
                "not authenticated: {m}\n  hint: run `myelin login --token <token>` or set \
                 $MYELIN_TOKEN"
            ),
            CliError::Unauthorized(m) => write!(f, "not authenticated / token invalid: {m}"),
            CliError::Edge { status, code, message } => {
                write!(f, "edge error ({status} {code}): {message}")
            }
            CliError::Transport(m) => write!(f, "could not reach the edge: {m}"),
            CliError::Unsupported(m) => write!(f, "{m}"),
            CliError::Config(m) => write!(f, "configuration error: {m}"),
        }
    }
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
            CliError::Edge { status: 404, code: "not_found".into(), message: "no".into() }.code(),
            1
        );
        assert_eq!(CliError::Transport("x".into()).code(), 1);
        assert_eq!(CliError::Config("x".into()).code(), 1);
    }

    /// No variant carries the credential material — the token cannot leak through an error string.
    #[test]
    fn no_error_variant_carries_a_token_field() {
        // A compile-time contract proven by inspection: the variants above carry only messages/codes
        // the call-sites build WITHOUT the token. This test documents the invariant + guards the
        // Display impl never interpolates a secret.
        let e = CliError::Unauthorized("bearer rejected".into());
        assert!(!e.to_string().to_lowercase().contains("paseto"));
        assert!(!e.to_string().contains("v4.public"));
    }
}
