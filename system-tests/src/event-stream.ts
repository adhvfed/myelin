export interface ServerSentEvent {
  event?: string;
  id?: string;
  data: string;
}

export class SystemEventStream {
  private readonly reader: ReadableStreamDefaultReader<Uint8Array>;
  private readonly decoder = new TextDecoder();
  private readonly queued: ServerSentEvent[] = [];
  private buffer = "";
  private closed = false;

  constructor(
    body: ReadableStream<Uint8Array>,
    private readonly abortController: AbortController,
  ) {
    this.reader = body.getReader();
  }

  async waitFor(
    predicate: (event: ServerSentEvent) => boolean,
    options: { description: string; timeoutMs?: number },
  ): Promise<ServerSentEvent> {
    const timeoutMs = options.timeoutMs ?? 10_000;
    const timeout = setTimeout(() => this.abortController.abort(), timeoutMs);
    try {
      while (true) {
        const queued = this.takeMatching(predicate);
        if (queued) return queued;

        let chunk: ReadableStreamReadResult<Uint8Array>;
        try {
          chunk = await this.reader.read();
        } catch (error) {
          if (this.abortController.signal.aborted) {
            throw new Error(`timed out after ${timeoutMs}ms waiting for ${options.description}`);
          }
          throw error;
        }
        if (chunk.done) {
          throw new Error(`event stream closed while waiting for ${options.description}`);
        }
        this.buffer += this.decoder.decode(chunk.value, { stream: true });
        this.parseCompleteFrames();
      }
    } finally {
      clearTimeout(timeout);
    }
  }

  close(): void {
    if (this.closed) return;
    this.closed = true;
    this.abortController.abort();
  }

  private takeMatching(predicate: (event: ServerSentEvent) => boolean): ServerSentEvent | undefined {
    const index = this.queued.findIndex(predicate);
    if (index < 0) return undefined;
    return this.queued.splice(index, 1)[0];
  }

  private parseCompleteFrames(): void {
    this.buffer = this.buffer.replaceAll("\r\n", "\n");
    let boundary = this.buffer.indexOf("\n\n");
    while (boundary >= 0) {
      const frame = this.buffer.slice(0, boundary);
      this.buffer = this.buffer.slice(boundary + 2);
      const event = parseFrame(frame);
      if (event) this.queued.push(event);
      boundary = this.buffer.indexOf("\n\n");
    }
  }
}

function parseFrame(frame: string): ServerSentEvent | undefined {
  let event: string | undefined;
  let id: string | undefined;
  const data: string[] = [];

  for (const line of frame.split("\n")) {
    if (!line || line.startsWith(":")) continue;
    const separator = line.indexOf(":");
    const field = separator < 0 ? line : line.slice(0, separator);
    const rawValue = separator < 0 ? "" : line.slice(separator + 1);
    const value = rawValue.startsWith(" ") ? rawValue.slice(1) : rawValue;
    if (field === "event") event = value;
    if (field === "id") id = value;
    if (field === "data") data.push(value);
  }

  if (data.length === 0) return undefined;
  return {
    ...(event === undefined ? {} : { event }),
    ...(id === undefined ? {} : { id }),
    data: data.join("\n"),
  };
}
