//! The Myelin Tauri 2 shell — the Rust side of the "one Rust core, three shells" seam (MR-018).
//!
//! Every `#[tauri::command]` here is a THIN call into a shared Myelin crate; none re-implements
//! Myelin logic in the shell. This is the load-bearing proof that the desktop/mobile apps share a
//! Rust core with the server, not a parallel JS island:
//! - [`render_markdown`] → `myelin_content`'s FROZEN render path (`render_parse`/`render_serialize`,
//!   the same single source compiled native + to `wasm32` for the editor — KN-D2);
//! - [`core_info`] → `myelin_content::corpus::corpus_pass_rate` + `myelin_client::ResilientConfig`,
//!   proving BOTH shared crates link into the shell.
//!
//! Mobile readiness: this crate + the two shared crates use only std/serde — no desktop-only
//! dependency — so the shared core is structurally mobile-clean (the validation MR-018 calls for).

use serde::Serialize;

/// IPC payload ceiling shared with the browser bridge. The command returns both input and output,
/// so this bound also caps the response and the parser's proportional intermediate allocations.
pub const MAX_RENDER_MARKDOWN_BYTES: usize = 64 * 1024;

/// The result of round-tripping a markdown-subset string through the shared myelin-content path.
/// Serialized to the Solid side as `{ input, output, roundTrips }`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderResult {
    /// The input markdown-subset string.
    pub input: String,
    /// `serialize_inline(parse_inline(input))` — the canonical re-serialization.
    pub output: String,
    /// `true` iff `output == input` (the frozen KN-D2 round-trip invariant held for this input).
    pub round_trips: bool,
}

/// Stable, serializable Tauri rejection. No submitted markdown is reflected in the error channel.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RenderError {
    pub code: &'static str,
    pub max_bytes: usize,
}

/// The pure core of the [`render_markdown`] command (no Tauri types, so it is unit-testable
/// directly). REUSES `myelin_content`'s frozen render path — it does not parse markdown itself.
pub fn render_markdown_core(md: &str) -> Result<RenderResult, RenderError> {
    if md.len() > MAX_RENDER_MARKDOWN_BYTES {
        return Err(RenderError {
            code: "markdown_too_large",
            max_bytes: MAX_RENDER_MARKDOWN_BYTES,
        });
    }
    // The positional structured-node array sized to the input's object-replacement count, exactly
    // as the editor supplies it (reused from the content crate so the binding stays single-sourced).
    let nodes = myelin_content::corpus::synthetic_nodes_for(md);
    // The ONE render path — the same `render_parse`/`render_serialize` the server + WASM editor use.
    let inline = myelin_content::wasm::render_parse(md, &nodes);
    let output = myelin_content::wasm::render_serialize(&inline);
    if output.len() > MAX_RENDER_MARKDOWN_BYTES {
        return Err(RenderError {
            code: "markdown_output_too_large",
            max_bytes: MAX_RENDER_MARKDOWN_BYTES,
        });
    }
    let round_trips = output == md;
    Ok(RenderResult {
        input: md.to_string(),
        output,
        round_trips,
    })
}

/// Tauri command: round-trip `md` through the shared myelin-content render path.
#[tauri::command]
fn render_markdown(md: String) -> Result<RenderResult, RenderError> {
    render_markdown_core(&md)
}

/// Liveness facts read straight from the linked shared crates (proves both link into the shell).
/// Serialized as `{ contentCorpusPassed, contentCorpusTotal, clientTimeoutMs }`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreInfo {
    /// `myelin_content::corpus::corpus_pass_rate()` passed count (the KN-D2 telemetry signal).
    pub content_corpus_passed: usize,
    /// `myelin_content::corpus::corpus_pass_rate()` total count.
    pub content_corpus_total: usize,
    /// `myelin_client::ResilientConfig::default().timeout_ms` — proves myelin-client links too.
    pub client_timeout_ms: u64,
}

/// The pure core of the [`core_info`] command (unit-testable without Tauri).
pub fn core_info_core() -> CoreInfo {
    let (passed, total) = myelin_content::corpus::corpus_pass_rate();
    let cfg = myelin_client::ResilientConfig::default();
    CoreInfo {
        content_corpus_passed: passed,
        content_corpus_total: total,
        client_timeout_ms: cfg.timeout_ms,
    }
}

/// Tauri command: read the shared-core liveness facts.
#[tauri::command]
fn core_info() -> CoreInfo {
    core_info_core()
}

/// The shared entry point. `main.rs` calls this on desktop; the `mobile_entry_point` attribute
/// makes it the JNI/Obj-C entry on iOS/Android — ONE source for all three shells.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![render_markdown, core_info])
        .run(tauri::generate_context!())
        .expect("error while running the Myelin Tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The DESKTOP bridge proof, at the unit level: "hello, shared myelin-content" round-trips
    /// THROUGH the shared crate's frozen render path on the Rust side. If this passes, the command
    /// genuinely calls myelin-content (not a stub).
    #[test]
    fn render_markdown_round_trips_through_shared_myelin_content() {
        let r = render_markdown_core("**hello, shared myelin-content**").unwrap();
        assert_eq!(
            r.output, "**hello, shared myelin-content**",
            "the shared myelin-content render path must re-serialize canonical bold markdown byte-for-byte"
        );
        assert!(r.round_trips, "the KN-D2 round-trip invariant must hold");
        assert_eq!(r.input, "**hello, shared myelin-content**");
    }

    /// A plain-text payload also round-trips (the simplest "hello").
    #[test]
    fn render_markdown_round_trips_plain_text() {
        let r = render_markdown_core("hello world, a plain paragraph.").unwrap();
        assert!(r.round_trips);
        assert_eq!(r.output, "hello world, a plain paragraph.");
    }

    #[test]
    fn render_markdown_rejects_oversized_ipc_without_reflecting_input() {
        let md = "ø".repeat(MAX_RENDER_MARKDOWN_BYTES / 2 + 1);
        let error =
            render_markdown_core(&md).expect_err("UTF-8 bytes, not characters, set the cap");
        assert_eq!(
            error,
            RenderError {
                code: "markdown_too_large",
                max_bytes: MAX_RENDER_MARKDOWN_BYTES,
            }
        );
    }

    /// `core_info` reads BOTH shared crates: the myelin-content corpus is fully green and the
    /// myelin-client default timeout is the M0 floor (2000ms). Proves both link, not just content.
    #[test]
    fn core_info_reads_both_shared_crates() {
        let i = core_info_core();
        assert_eq!(
            i.content_corpus_passed, i.content_corpus_total,
            "the shared myelin-content corpus must round-trip 100%"
        );
        assert!(
            i.content_corpus_total >= 18,
            "the frozen corpus must not be shrunk"
        );
        assert_eq!(
            i.client_timeout_ms, 2000,
            "myelin-client's default per-target timeout (the M0 floor) must be reachable from the shell"
        );
    }

    #[test]
    fn production_webview_policy_is_enabled_and_fail_closed() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let security = &config["app"]["security"];
        let csp = security["csp"]
            .as_str()
            .expect("production CSP must be configured");
        for directive in [
            "default-src 'none'",
            "connect-src ipc: http://ipc.localhost",
            "object-src 'none'",
            "frame-ancestors 'none'",
            "form-action 'none'",
        ] {
            assert!(csp.contains(directive), "production CSP lacks {directive}");
        }
        assert!(!csp.contains("unsafe-eval"));
        assert_eq!(
            csp.matches("http://").count(),
            1,
            "only the Tauri IPC origin is allowed"
        );
        assert!(!csp.contains("https:"));
        assert!(!csp.contains("ws:"));
        assert_eq!(security["headers"]["X-Content-Type-Options"], "nosniff");
    }
}
