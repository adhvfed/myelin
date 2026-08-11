import { randomUUID } from "node:crypto";

import type { SystemTestConfig } from "./config.js";
import { SystemEventStream } from "./event-stream.js";
import { record, type JsonRecord } from "./json.js";

type Method = "GET" | "HEAD" | "POST" | "PUT" | "PATCH" | "DELETE";

export interface RequestOptions {
  method?: Method;
  body?: unknown;
  authenticated?: boolean;
  token?: string;
  tokenScheme?: string;
  expectedStatus?: number | readonly number[];
  idempotencyKey?: string | false;
  headers?: Readonly<Record<string, string>>;
  timeoutMs?: number;
}

export interface SystemResponse<T> {
  status: number;
  headers: Headers;
  body: T;
}

export class UnexpectedSystemResponse extends Error {
  constructor(
    readonly method: Method,
    readonly path: string,
    readonly status: number,
    readonly responseBody: string,
    readonly expectedStatuses: readonly number[],
  ) {
    super(
      `${method} ${path} returned ${status}; expected ${expectedStatuses.join(" or ")}: ${responseBody}`,
    );
    this.name = "UnexpectedSystemResponse";
  }
}

export class SystemTestClient {
  constructor(private readonly config: SystemTestConfig) {}

  async request(path: string, options: RequestOptions = {}): Promise<SystemResponse<string>> {
    const method = options.method ?? "GET";
    const expectedStatuses = Array.isArray(options.expectedStatus)
      ? options.expectedStatus
      : [options.expectedStatus ?? 200];
    const authenticated = options.authenticated ?? true;
    const headers = new Headers(options.headers);
    headers.set("accept", "application/json");
    if (authenticated) {
      headers.set("authorization", `Bearer ${options.token ?? this.config.token}`);
      headers.set("x-myelin-token-scheme", options.tokenScheme ?? this.config.tokenScheme);
    }
    if (options.body !== undefined) headers.set("content-type", "application/json");
    if (method === "POST" || method === "PUT" || method === "PATCH" || method === "DELETE") {
      const key = options.idempotencyKey === undefined ? randomUUID() : options.idempotencyKey;
      if (key !== false) headers.set("idempotency-key", key);
    }

    const response = await fetch(new URL(path, `${this.config.edgeUrl}/`), {
      method,
      headers,
      ...(options.body === undefined ? {} : { body: JSON.stringify(options.body) }),
      redirect: "error",
      signal: AbortSignal.timeout(options.timeoutMs ?? 15_000),
    });
    const responseBody = method === "HEAD" ? "" : await response.text();
    if (!expectedStatuses.includes(response.status)) {
      throw new UnexpectedSystemResponse(
        method,
        path,
        response.status,
        responseBody,
        expectedStatuses,
      );
    }
    return { status: response.status, headers: response.headers, body: responseBody };
  }

  async json(path: string, options: RequestOptions = {}): Promise<SystemResponse<JsonRecord>> {
    const response = await this.request(path, options);
    let parsed: unknown;
    try {
      parsed = JSON.parse(response.body);
    } catch (error) {
      const detail = error instanceof Error ? error.message : String(error);
      throw new Error(`${options.method ?? "GET"} ${path} returned invalid JSON: ${detail}`);
    }
    return { ...response, body: record(parsed, `${options.method ?? "GET"} ${path} response`) };
  }

  async eventStream(path: string): Promise<{
    headers: Headers;
    stream: SystemEventStream;
  }> {
    const abortController = new AbortController();
    const response = await fetch(new URL(path, `${this.config.edgeUrl}/`), {
      headers: {
        accept: "text/event-stream",
        authorization: `Bearer ${this.config.token}`,
        "x-myelin-token-scheme": this.config.tokenScheme,
      },
      redirect: "error",
      signal: abortController.signal,
    });
    if (response.status !== 200 || !response.body) {
      const responseBody = await response.text();
      abortController.abort();
      throw new UnexpectedSystemResponse("GET", path, response.status, responseBody, [200]);
    }
    return {
      headers: response.headers,
      stream: new SystemEventStream(response.body, abortController),
    };
  }
}
