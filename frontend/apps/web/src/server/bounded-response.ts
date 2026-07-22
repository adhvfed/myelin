/** Read an upstream response without allowing an unbounded body to accumulate in process memory. */
export async function readLimitedBytes(
  response: Response,
  maxBytes: number,
): Promise<Uint8Array<ArrayBuffer>> {
  if (!Number.isSafeInteger(maxBytes) || maxBytes < 0) {
    throw new RangeError("response byte limit must be a non-negative safe integer");
  }

  const declared = response.headers.get("content-length")?.trim();
  if (declared && /^\d+$/.test(declared) && BigInt(declared) > BigInt(maxBytes)) {
    throw new Error("upstream response exceeded the byte limit");
  }
  if (!response.body) return new Uint8Array();

  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    size += value.byteLength;
    if (size > maxBytes) {
      await reader.cancel().catch(() => undefined);
      throw new Error("upstream response exceeded the byte limit");
    }
    chunks.push(value);
  }

  const body = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) {
    body.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return body;
}

/** Read a bounded, strictly valid UTF-8 response body. */
export async function readLimitedText(response: Response, maxBytes: number): Promise<string> {
  return new TextDecoder("utf-8", { fatal: true }).decode(
    await readLimitedBytes(response, maxBytes),
  );
}
