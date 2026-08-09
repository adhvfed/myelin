pub mod ci_watch;
pub mod client;
pub mod config;
pub mod context;
mod credential_store;
pub mod device_auth;
pub mod dispatch;
pub mod error;
pub mod git_credential;
pub mod mcp_bridge;
mod profiles;
pub mod render;

pub use config::EdgeConfig;
pub use dispatch::{EdgeCall, HttpMethod};
pub use error::CliError;
