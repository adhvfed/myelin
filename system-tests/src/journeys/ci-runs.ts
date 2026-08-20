import type { SystemTestClient } from "../client.js";
import { eventually } from "../eventually.js";
import type { GitProject } from "../git-project.js";
import { array, integer, record, string, type JsonRecord } from "../json.js";
import { findPaged } from "../paging.js";

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
  return eventually(async () => findPaged(
    client,
    "/v1/ci/runs?state=all",
    (run) => run.commit_oid === commitOid,
  ), {
    description: `CI to begin for ${project.slug} at ${commitOid}`,
  });
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
