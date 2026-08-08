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
    expectedStatus: number,
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
    expect(response.status, `${method} ${path} returned ${response.status}: ${text}`).toBe(
      expectedStatus,
    );
    return object(JSON.parse(text));
  }

  test("creates, browses, commits, and opens a review through durable services", async () => {
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
  });
});
