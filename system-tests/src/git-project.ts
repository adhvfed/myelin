import { randomUUID } from "node:crypto";

import type { SystemTestClient } from "./client.js";
import { record, string, type JsonRecord } from "./json.js";

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
}
