import type { SystemTestClient } from "../client.js";
import type { GitProject } from "../git-project.js";
import { array, integer, record, string, type JsonRecord } from "../json.js";

export interface PullRequestProposal {
  branch: string;
  path: string;
  contents: string;
  title: string;
  commitMessage?: string;
  reviewers?: string[];
}

export interface OpenedPullRequest {
  repository: string;
  number: number;
  headOid: string;
  pullRequest: JsonRecord;
  receipt: JsonRecord;
}

export interface PullRequestPage {
  items: JsonRecord[];
  nextCursor: string | null;
  previousCursor: string | null;
  limit: number;
  total: number;
  counts: JsonRecord;
}

export async function proposeChange(
  client: SystemTestClient,
  project: GitProject,
  proposal: PullRequestProposal,
): Promise<OpenedPullRequest> {
  const headOid = (await project.writeFile(
    proposal.branch,
    proposal.path,
    proposal.contents,
    {
      startRef: "main",
      ...(proposal.commitMessage === undefined ? {} : { message: proposal.commitMessage }),
    },
  )).commitOid;
  const response = await client.json(`${project.path}/prs`, {
    method: "POST",
    body: {
      title: proposal.title,
      base_ref: "refs/heads/main",
      head_ref: `refs/heads/${proposal.branch}`,
      head_oid: headOid,
      reviewers: proposal.reviewers ?? [],
    },
    expectedStatus: 201,
  });
  const receipt = record(response.body, "pull request receipt");
  const pullRequest = record(
    record(receipt.applied, "pull request receipt.applied").pr,
    "opened pull request",
  );
  return {
    repository: project.slug,
    number: integer(pullRequest.number, "opened pull request number"),
    headOid,
    pullRequest,
    receipt,
  };
}

export async function readPullRequestPage(
  client: SystemTestClient,
  path: string,
  query: Record<string, string | number>,
): Promise<PullRequestPage> {
  const search = new URLSearchParams(
    Object.entries(query).map(([name, value]) => [name, String(value)]),
  );
  const response = await client.json(`${path}?${search.toString()}`);
  const page = record(response.body.page, "pull request page");
  const nextCursor = nullableCursor(page.next_cursor, "pull request next cursor");
  const previousCursor = nullableCursor(page.prev_cursor, "pull request previous cursor");
  return {
    items: array(response.body.items, "pull request page items")
      .map((item) => record(item, "pull request page item")),
    nextCursor,
    previousCursor,
    limit: integer(page.limit, "pull request page limit"),
    total: integer(page.total, "pull request page total"),
    counts: record(response.body.counts, "pull request counts"),
  };
}

function nullableCursor(value: unknown, context: string): string | null {
  return value === null ? null : string(value, context);
}
