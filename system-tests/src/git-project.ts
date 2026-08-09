import { randomUUID } from "node:crypto";

import type { SystemTestClient } from "./client.js";
import { array, integer, record, string, type JsonRecord } from "./json.js";

export interface CodeSearchHit extends JsonRecord {
  repo: string;
  ref: string;
  snapshot_oid: string;
  path: string;
  line: number;
  excerpt: string;
}

export interface GitFile {
  contents: string;
  baseOid: string;
}

function codeSearchHit(value: unknown): CodeSearchHit {
  const hit = record(value, "code search item");
  return {
    ...hit,
    repo: string(hit.repo, "code search item.repo"),
    ref: string(hit.ref, "code search item.ref"),
    snapshot_oid: string(hit.snapshot_oid, "code search item.snapshot_oid"),
    path: string(hit.path, "code search item.path"),
    line: integer(hit.line, "code search item.line"),
    excerpt: string(hit.excerpt, "code search item.excerpt"),
  };
}

function segment(value: string): string {
  return encodeURIComponent(value);
}

function nestedPath(value: string): string {
  return value.split("/").map(segment).join("/");
}

export class GitProject {
  readonly path: string;

  constructor(
    readonly slug: string,
    private readonly client: SystemTestClient,
  ) {
    this.path = `/v1/git/repos/${segment(slug)}`;
  }

  async create(idempotencyKey: string = randomUUID()): Promise<JsonRecord> {
    return (await this.client.json("/v1/git/repos", {
      method: "POST",
      body: { slug: this.slug },
      expectedStatus: [200, 201],
      idempotencyKey,
    })).body;
  }

  async writeFile(
    ref: string,
    path: string,
    contents: string,
    options: { baseOid?: string; startRef?: string } = {},
  ): Promise<{ receipt: JsonRecord; commitOid: string }> {
    const receipt = (await this.client.json(
      `${this.path}/blob/${segment(ref)}/${nestedPath(path)}`,
      {
        method: "POST",
        body: {
          base_oid: options.baseOid ?? "",
          contents,
          ...(options.startRef === undefined ? {} : { start_ref: options.startRef }),
        },
      },
    )).body;
    const applied = record(receipt.applied, "git write receipt.applied");
    return {
      receipt,
      commitOid: string(applied.new_oid, "git write receipt.applied.new_oid"),
    };
  }

  async readFile(ref: string, path: string): Promise<GitFile> {
    const body = (await this.client.json(
      `${this.path}/blob/${segment(ref)}/${nestedPath(path)}`,
    )).body;
    return {
      contents: string(body.contents, "git file.contents"),
      baseOid: string(body.base_oid, "git file.base_oid"),
    };
  }

  async updateFile(
    ref: string,
    path: string,
    contents: string,
    options: { startRef?: string } = {},
  ): Promise<{ receipt: JsonRecord; commitOid: string }> {
    const sourceRef = options.startRef ?? ref;
    const current = await this.readFile(sourceRef, path);
    return this.writeFile(ref, path, contents, {
      baseOid: current.baseOid,
      ...(options.startRef === undefined ? {} : { startRef: options.startRef }),
    });
  }

  async searchCode(query: string): Promise<{ items: CodeSearchHit[]; complete: boolean }> {
    const params = new URLSearchParams({ repo: this.slug, q: query });
    const body = (await this.client.json(`/v1/git/search/code?${params}`)).body;
    if (typeof body.complete !== "boolean") {
      throw new TypeError("code search response.complete must be a boolean");
    }
    return {
      items: array(body.items, "code search items").map(codeSearchHit),
      complete: body.complete,
    };
  }
}
