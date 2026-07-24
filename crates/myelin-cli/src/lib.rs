//! # `myelin-cli` — the `myelin` CLI core (E0.9 / MR-020)
//!
//! The ONE local command surface a HUMAN or an AGENT drives the product through. It is an edge
//! CLIENT: it presents a REAL capability token as `Authorization: Bearer <paseto>` and calls the
//! MR-014 gateway's `/v1/...` endpoints, under the SAME authentication + audit a browser session is.
//! This is the near-term "agents, but driven locally" path (the CLI + MCP): the CLI acts as a
//! principal through the product edge with no auth carve-out — agent governance from day one.
//!
//! ## The command framework (REUSE, don't re-derive)
//! `myelin <subsystem> <command> [args]`. The top-level shell (clap) owns ONLY the global flags
//! (`--json`/`--edge`/`--token`/`--scheme`) + `login`/`whoami`; the per-subsystem command SET is
//! REUSED from the subsystem crates — git's [`myelin_git::api::parse_cli`] / [`myelin_git::api::CliCommand`]
//! Issues' [`myelin_issues::api`] grammar, CI's [`myelin_ci_controlplane::cli`] durable-read/live
//! grammar, and notif's [`myelin_notif::cli`] grammar — so a new subsystem command flows to the CLI
//! without re-declaring it here. See [`dispatch`] for the "how a subsystem adds CLI commands"
//! convention (the mirror of the MR-014 edge plug-in convention).
//!
//! ## Auth (real vs the named seam)
//! REAL end-to-end: the Bearer presentation of a capability token + the edge round-trip that verifies
//! its Ed25519 signature against the cell. The CLI presents an UNBOUND short-lived machine/capability
//! token (DPoP-bound PATs are not threaded through the edge yet — the MR-014 follow-up). The NAMED
//! SEAM: the full human-login MINT (exchanging an IdP assertion for a fresh token) is MR-012-deferred
//! (the edge `login` route refuses-not-mocks); `myelin login` stores a token the operator already
//! holds. The token is NEVER logged (see [`config`]).
//!
//! ## DAG position
//! A LEAF BINARY: it depends on the subsystem command grammars + a tiny hyper HTTP client, and NOTHING
//! in the production crate DAG depends back on it. Like `myelin-edge` it is NOT a node in the
//! eleven-crate library DAG modelled by `myelin_substrate::crate_graph` (substrate_is_root() /
//! identity_is_sink() are unaffected — a CLI is a terminal consumer).

pub mod ci_watch;
pub mod client;
pub mod config;
pub mod dispatch;
pub mod error;
pub mod render;

pub use config::EdgeConfig;
pub use dispatch::{EdgeCall, HttpMethod};
pub use error::CliError;
