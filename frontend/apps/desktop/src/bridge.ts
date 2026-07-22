// The Tauri Rust-core bridge (the load-bearing MR-018 proof, frontend side).
//
// Every function here `invoke`s a Tauri COMMAND implemented in `src-tauri`, whose Rust body
// REUSES a Myelin crate (`myelin-content`'s frozen render path; `myelin-client`'s config). This
// is the "one Rust core, three shells" seam: the native shell does not re-implement Myelin logic
// in JS — it calls the same Rust crates the server uses, over the Tauri IPC boundary.
import { invoke } from "@tauri-apps/api/core";

export const MAX_RENDER_MARKDOWN_BYTES = 64 * 1024;
const utf8 = new TextEncoder();

/** The result of round-tripping a markdown-subset string through the SHARED myelin-content path. */
export interface RenderResult {
  /** The input markdown-subset string. */
  input: string;
  /** `serialize_inline(parse_inline(input))` — the canonical re-serialization. */
  output: string;
  /** `true` iff `output === input` (the frozen KN-D2 round-trip invariant held for this input). */
  roundTrips: boolean;
}

/**
 * Round-trip `md` through the Tauri Rust side, which calls `myelin_content::wasm::render_parse`
 * + `render_serialize` — the SAME single render path compiled into the server and the WASM
 * editor. The "hello, shared myelin-content" proof: the string is parsed + re-serialized by the
 * shared Rust crate, not by JS.
 */
export async function renderMarkdown(md: string): Promise<RenderResult> {
  if (utf8.encode(md).byteLength > MAX_RENDER_MARKDOWN_BYTES) {
    throw new RangeError(`markdown exceeds ${MAX_RENDER_MARKDOWN_BYTES} bytes`);
  }
  const value: unknown = await invoke("render_markdown", { md });
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("render_markdown returned an invalid result");
  }
  const result = value as Record<string, unknown>;
  if (
    typeof result.input !== "string" ||
    typeof result.output !== "string" ||
    typeof result.roundTrips !== "boolean" ||
    utf8.encode(result.input).byteLength > MAX_RENDER_MARKDOWN_BYTES ||
    utf8.encode(result.output).byteLength > MAX_RENDER_MARKDOWN_BYTES ||
    result.input !== md ||
    result.roundTrips !== (result.output === result.input)
  ) {
    throw new Error("render_markdown returned an invalid result");
  }
  return {
    input: result.input,
    output: result.output,
    roundTrips: result.roundTrips,
  };
}

/** Liveness facts pulled straight from the shared crates, proving both link into the shell. */
export interface CoreInfo {
  /** `myelin_content::corpus::corpus_pass_rate()` passed count (the KN-D2 telemetry signal). */
  contentCorpusPassed: number;
  /** `myelin_content::corpus::corpus_pass_rate()` total count. */
  contentCorpusTotal: number;
  /** `myelin_client::ResilientConfig::default().timeout_ms` — proves myelin-client links too. */
  clientTimeoutMs: number;
}

/** Fetch the shared-core liveness facts from the Tauri Rust side. */
export async function coreInfo(): Promise<CoreInfo> {
  return await invoke<CoreInfo>("core_info");
}
