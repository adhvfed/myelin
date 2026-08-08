import { randomUUID } from "node:crypto";
import { describe, expect, test } from "vitest";

type JsonObject = Record<string, unknown>;

function requiredEnvironment(name: string): string {
  const value = process.env[name]?.trim();
  if (!value) throw new Error(`${name} is required; run this test with fed test:integration`);
  return value;
}

function object(value: unknown): JsonObject {
  expect(value).toBeTypeOf("object");
  expect(value).not.toBeNull();
  expect(Array.isArray(value)).toBe(false);
  return value as JsonObject;
}

describe("assembled product edge", () => {
  const edgeUrl = requiredEnvironment("MYELIN_INTEGRATION_EDGE_URL").replace(/\/$/, "");
  const token = requiredEnvironment("MYELIN_INTEGRATION_TOKEN");
  const tenant = requiredEnvironment("MYELIN_INTEGRATION_TENANT");

  async function request(
    method: "GET" | "POST",
    path: string,
    expectedStatus: number | readonly number[],
    body?: unknown,
  ): Promise<JsonObject> {
    const response = await fetch(`${edgeUrl}${path}`, {
      method,
      headers: {
        accept: "application/json",
        authorization: `Bearer ${token}`,
        "x-myelin-token-scheme": "agent",
        ...(body === undefined ? {} : { "content-type": "application/json" }),
        ...(method === "POST" ? { "idempotency-key": randomUUID() } : {}),
      },
      body: body === undefined ? undefined : JSON.stringify(body),
      redirect: "error",
      signal: AbortSignal.timeout(15_000),
    });
    const text = await response.text();
    const acceptedStatuses = Array.isArray(expectedStatus) ? expectedStatus : [expectedStatus];
    expect(
      acceptedStatuses,
      `${method} ${path} returned ${response.status}: ${text}`,
    ).toContain(response.status);
    return object(JSON.parse(text));
  }

  async function waitForIssueActivation(requestEventId: string): Promise<JsonObject> {
    const deadline = Date.now() + 15_000;
    while (Date.now() < deadline) {
      const status = await request(
        "GET",
        `/v1/issues/authorization-requests/${encodeURIComponent(requestEventId)}`,
        [200, 202],
      );
      if (status.status === "active") return object(status.issue);
      expect(status.status).toBe("pending");
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    throw new Error(`issue authorization ${requestEventId} did not activate within 15 seconds`);
  }

  async function waitForCiRun(repoRef: string, commitOid: unknown): Promise<JsonObject> {
    const deadline = Date.now() + 15_000;
    while (Date.now() < deadline) {
      const runs = await request("GET", "/v1/ci/runs?state=all&limit=50", 200);
      expect(Array.isArray(runs.items)).toBe(true);
      const match = (runs.items as unknown[]).find((value) => {
        if (value === null || typeof value !== "object" || Array.isArray(value)) return false;
        const row = value as JsonObject;
        return row.repo_ref === repoRef && row.commit_oid === commitOid;
      });
      if (match !== undefined) return object(match);
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    throw new Error(`CI run for ${repoRef} at ${String(commitOid)} did not appear within 15 seconds`);
  }

  test("creates, browses, commits, reviews, and tracks work through durable services", async () => {
    const slug = `it-${Date.now().toString(36)}-${randomUUID().slice(0, 8)}`;
    const repoPath = `/v1/git/repos/${encodeURIComponent(slug)}`;

    const whoami = await request("GET", "/v1/whoami", 200);
    expect(whoami).toMatchObject({
      principal_id: "product-test",
      tenant,
      region: "fr-par",
      kind: "human",
    });

    const created = await request("POST", "/v1/git/repos", 201, { slug });
    expect(created).toMatchObject({
      durable: true,
      applied: { action: "git.repo.create", slug },
    });

    const emptyRepo = await request("GET", repoPath, 200);
    expect(emptyRepo).toMatchObject({ state: "empty", slug: `${tenant}/${slug}` });

    const readme = `# ${slug}\n\nCreated through the assembled Myelin edge.\n`;
    const mainCommit = await request("POST", `${repoPath}/blob/main/README.md`, 200, {
      base_oid: "",
      contents: readme,
    });
    const mainApplied = object(mainCommit.applied);
    expect(mainCommit.durable).toBe(true);
    expect(mainApplied.outcome).toBe("committed");
    expect(mainApplied.new_oid).toMatch(/^[0-9a-f]{40}$/);

    const blob = await request("GET", `${repoPath}/blob/main/README.md`, 200);
    expect(blob.contents).toBe(readme);
    expect(blob.base_oid).toMatch(/^[0-9a-f]{40}$/);
    expect(blob.base_oid).not.toBe(mainApplied.new_oid);

    const populatedRepo = await request("GET", repoPath, 200);
    expect(populatedRepo).toMatchObject({
      state: "populated",
      slug: `${tenant}/${slug}`,
      readme,
      snapshot_oid: mainApplied.new_oid,
    });
    expect(populatedRepo.entries).toEqual(
      expect.arrayContaining([expect.objectContaining({ name: "README.md", is_dir: false })]),
    );

    const pipeline = `on = "push"

[[jobs]]
name = "test"
image = "registry.example/test@sha256:ffeeddccbbaa0000000000000000000000000000000000000000000000000000"
command = ["true"]
`;
    const pipelineCommit = await request(
      "POST",
      `${repoPath}/blob/main/.myelin/ci.toml`,
      200,
      { base_oid: "", contents: pipeline },
    );
    const pipelineOid = object(pipelineCommit.applied).new_oid;
    expect(pipelineOid).toMatch(/^[0-9a-f]{40}$/);

    const ciRun = await waitForCiRun(`myelin://${tenant}/git/repo/${slug}`, pipelineOid);
    expect(ciRun).toMatchObject({
      commit_oid: pipelineOid,
      trigger_kind: "push",
      state: "queued",
    });
    expect(ciRun.run_id).toMatch(/^[0-9a-f-]{36}$/);

    const featureCommit = await request("POST", `${repoPath}/blob/feature/app.txt`, 200, {
      base_oid: "",
      contents: "export const ready = true;\n",
    });
    const featureOid = object(featureCommit.applied).new_oid;
    expect(featureOid).toMatch(/^[0-9a-f]{40}$/);

    const opened = await request("POST", `${repoPath}/prs`, 201, {
      title: "Add the first product change",
      base_ref: "refs/heads/main",
      head_ref: "refs/heads/feature",
      head_oid: featureOid,
    });
    expect(opened).toMatchObject({
      durable: true,
      applied: {
        action: "git.pr.open",
        pr: { number: 1, title: "Add the first product change" },
      },
    });

    const review = await request("GET", `${repoPath}/prs/1`, 200);
    expect(review).toMatchObject({
      number: 1,
      title: "Add the first product change",
      pr_state: "open",
      base_ref: "refs/heads/main",
      head_ref: "refs/heads/feature",
      head_oid: featureOid,
    });

    const reviews = await request("GET", `${repoPath}/prs?state=open&sort=updated`, 200);
    expect(reviews).toMatchObject({
      counts: { open: 1, all: 1 },
      page: { next_cursor: null, prev_cursor: null },
    });
    expect(reviews.items).toEqual([
      expect.objectContaining({
        number: 1,
        title: "Add the first product change",
        pr_state: "open",
        base_ref: "refs/heads/main",
        head_ref: "refs/heads/feature",
      }),
    ]);

    const issueTitle = `Ship ${slug}`;
    const issueReceipt = await request("POST", "/v1/issues", 202, {
      project_id: requiredEnvironment("MYELIN_INTEGRATION_ISSUES_PROJECT"),
      type_id: requiredEnvironment("MYELIN_INTEGRATION_ISSUES_TYPE"),
      prefix: requiredEnvironment("MYELIN_INTEGRATION_ISSUES_PREFIX"),
      title: issueTitle,
    });
    const issueSummary = object(issueReceipt.issue);
    const authorization = object(issueReceipt.authorization);
    expect(issueSummary.id).toMatch(/^[0-9a-f-]{36}$/);
    expect(issueSummary.key).toMatch(/^MYL-\d+$/);
    expect(authorization).toMatchObject({ status: "pending" });

    const activatedIssue = await waitForIssueActivation(String(authorization.request_event_id));
    expect(activatedIssue).toMatchObject({
      id: issueSummary.id,
      key: issueSummary.key,
      title: issueTitle,
      state_category: "unstarted",
    });

    const issueId = encodeURIComponent(String(issueSummary.id));
    const issue = await request("GET", `/v1/issues/${issueId}`, 200);
    expect(issue).toMatchObject(activatedIssue);

    const openIssues = await request("GET", "/v1/issues?state=open&limit=50", 200);
    expect(openIssues.items).toEqual(
      expect.arrayContaining([expect.objectContaining({ id: issueSummary.id, title: issueTitle })]),
    );

    const closed = await request("POST", `/v1/issues/${issueId}/close`, 200, {});
    expect(closed).toMatchObject({ id: issueSummary.id, state_category: "completed" });

    const closedIssues = await request("GET", "/v1/issues?state=closed&limit=50", 200);
    expect(closedIssues.items).toEqual(
      expect.arrayContaining([expect.objectContaining({ id: issueSummary.id, title: issueTitle })]),
    );
  });
});
