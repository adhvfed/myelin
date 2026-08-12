// Tauri bridge to the shared Rust content and client crates.
import { invoke } from "@tauri-apps/api/core";

export const MAX_RENDER_MARKDOWN_BYTES = 64 * 1024;
const utf8 = new TextEncoder();

/** The result of round-tripping a markdown-subset string through myelin-content. */
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
 * + `render_serialize` in the shared Rust crate.
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
  /** `myelin_client::ResilientConfig::default().timeout_ms`. */
  clientTimeoutMs: number;
}

/** Fetch the shared-core liveness facts from the Tauri Rust side. */
export async function coreInfo(): Promise<CoreInfo> {
  return await invoke<CoreInfo>("core_info");
}
