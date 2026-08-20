import type { SystemTestClient } from "../client.js";
import { eventually } from "../eventually.js";
import type { GitProject } from "../git-project.js";
import { array, integer, record, string, type JsonRecord } from "../json.js";
import { walkPaged } from "../paging.js";

const runnerImage =
  "myelin.local/linux-small-v1-rootfs@sha256:65f0f6f242cd4412b4ad56250eadb0a459a59a71b49d21485e68da6a3d5cb975";

export interface CiRunPage {
  items: JsonRecord[];
  nextCursor: string | null;
  limit: number;
}

export function passingPushPipeline(): string {
  return `schema_version = 2
on = "push"

[execution]
profile = "linux-small-v1"

[[jobs]]
name = "contract"
image = "${runnerImage}"
command = ["true"]
`;
}

export async function pushPipelineAndAwaitRun(
  client: SystemTestClient,
  project: GitProject,
  pipeline: string,
): Promise<JsonRecord> {
  const commitOid = (await project.writeFile("main", ".myelin/ci.toml", pipeline)).commitOid;
  return awaitTheOnlyCiRun(
    client,
    (run) => run.commit_oid === commitOid,
    `CI to begin for ${project.slug} at ${commitOid}`,
  );
}

export async function ciRunsMatching(
  client: SystemTestClient,
  predicate: (run: JsonRecord) => boolean,
): Promise<JsonRecord[]> {
  const matches: JsonRecord[] = [];
  for await (const run of walkPaged(client, "/v1/ci/runs?state=all")) {
    if (predicate(run)) matches.push(run);
  }
  return matches;
}

export async function awaitTheOnlyCiRun(
  client: SystemTestClient,
  predicate: (run: JsonRecord) => boolean,
  description: string,
  timeoutMs?: number,
): Promise<JsonRecord> {
  return eventually(async () => {
    const matches = await ciRunsMatching(client, predicate);
    if (matches.length === 0) return undefined;
    if (matches.length > 1) {
      throw new Error(`${description} produced ${matches.length} CI runs instead of one`);
    }
    return matches[0];
  }, { description, ...(timeoutMs === undefined ? {} : { timeoutMs }) });
}

export async function readCiRunPage(
  client: SystemTestClient,
  query: Record<string, string | number>,
): Promise<CiRunPage> {
  const search = new URLSearchParams(
    Object.entries(query).map(([name, value]) => [name, String(value)]),
  );
  const response = await client.json(`/v1/ci/runs?${search.toString()}`);
  const page = record(response.body.page, "CI run page");
  return {
    items: array(response.body.items, "CI run page items")
      .map((item) => record(item, "CI run page item")),
    nextCursor: page.next_cursor === null
      ? null
      : string(page.next_cursor, "CI run next cursor"),
    limit: integer(page.limit, "CI run page limit"),
  };
}
