import { randomUUID } from "node:crypto";

import { describe, expect, test } from "vitest";

import { reviewerClient, systemClient, uniqueName } from "../src/context.js";
import { awaitAuthorizedIssue } from "../src/issues.js";
import { awaitActiveIssue, expectOpaqueIssueAuthor } from "../src/journeys/issues.js";
import { findProject } from "../src/journeys/projects.js";
import { awaitBacklink, awaitBacklinkGone } from "../src/journeys/refs.js";
import { array, record, string } from "../src/json.js";
import { systemTestConfig } from "../src/config.js";

describe("issue lifecycle", () => {
  test("lets a founder start a project and its first issue without operator-provided IDs", async () => {
    const name = uniqueName("Developer experience");
    const issuePrefix = `P${randomUUID().replaceAll("-", "").slice(0, 7).toUpperCase()}`;
    const retryKey = `project-${randomUUID()}`;
    const created = await systemClient.json("/v1/projects", {
      method: "POST",
      body: { name, issue_prefix: issuePrefix },
      idempotencyKey: retryKey,
      expectedStatus: 201,
    });
    const project = record(created.body.project, "created project");
    const projectId = string(project.id, "project id");
    const projectRef = string(project.ref, "project ref");
    const defaultIssueTypeId = string(project.default_issue_type_id, "default issue type id");

    expect(created.body).toMatchObject({ created: true, durable: true });
    expect(project).toMatchObject({ name, issue_prefix: issuePrefix });
    expect(projectId).toMatch(/^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/);
    expect(defaultIssueTypeId).toMatch(/^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/);
    expect(projectRef).toBe(
      `myelin://${systemTestConfig.tenant}/identity/project/${projectId}`,
    );

    const replay = await systemClient.json("/v1/projects", {
      method: "POST",
      body: { name, issue_prefix: issuePrefix },
      idempotencyKey: retryKey,
      expectedStatus: 200,
    });
    expect(replay.body).toMatchObject({
      created: false,
      durable: true,
      project: { id: projectId, ref: projectRef },
    });

    const conflictingReplay = await systemClient.json("/v1/projects", {
      method: "POST",
      body: { name: `${name} changed`, issue_prefix: issuePrefix },
      idempotencyKey: retryKey,
      expectedStatus: 409,
    });
    expect(conflictingReplay.body).toMatchObject({ error: { code: "conflict" } });

    const rediscovered = await systemClient.json(`/v1/projects/${projectId}`);
    expect(rediscovered.body).toMatchObject({ project: { id: projectId, ref: projectRef, name } });

    await expect(findProject(systemClient, projectId)).resolves.toMatchObject({
      id: projectId,
      ref: projectRef,
      name,
    });

    const firstIssueTitle = uniqueName("Make the first project useful");
    const firstIssueRetryKey = `first-project-issue-${randomUUID()}`;
    const firstIssue = await systemClient.json("/v1/issues", {
      method: "POST",
      body: { project_id: projectId, title: firstIssueTitle },
      idempotencyKey: firstIssueRetryKey,
      expectedStatus: 202,
    });
    const issue = record(firstIssue.body.issue, "first project issue");
    const issueAuthorization = record(firstIssue.body.authorization, "first project issue authorization");
    expect(issue).toMatchObject({
      project_id: projectId,
      key: expect.stringMatching(new RegExp(`^${issuePrefix}-\\d+$`)),
    });
    expect(firstIssue.body).not.toHaveProperty("type_id");
    const requestEventId = string(issueAuthorization.request_event_id, "first issue authorization id");
    const retriedFirstIssue = await systemClient.json("/v1/issues", {
      method: "POST",
      body: { project_id: projectId, title: firstIssueTitle },
      idempotencyKey: firstIssueRetryKey,
      expectedStatus: 200,
    });
    expect(retriedFirstIssue.body).toMatchObject({
      created: false,
      durable: true,
      issue: { id: issue.id, key: issue.key, project_id: projectId },
      authorization: { status: "pending", request_event_id: requestEventId },
    });
    const activeIssue = await awaitAuthorizedIssue(
      systemClient,
      requestEventId,
      "the first project issue to become ordinary visible work",
    );
    expect(activeIssue).toMatchObject({
      id: issue.id,
      project_id: projectId,
      title: firstIssueTitle,
    });

    const hiddenFromPeer = await reviewerClient.json(`/v1/projects/${projectId}`, {
      expectedStatus: 404,
    });
    expect(hiddenFromPeer.body).toMatchObject({ error: { code: "not_found" } });
    await expect(findProject(reviewerClient, projectId)).resolves.toBeUndefined();
  });

  test("moves an issue through authorization, discovery, and completion", async () => {
    const title = uniqueName("Ship the backend lifecycle suite");
    const proposed = await systemClient.json("/v1/issues", {
      method: "POST",
      body: {
        project_id: systemTestConfig.issues.projectId,
        type_id: systemTestConfig.issues.typeId,
        prefix: systemTestConfig.issues.prefix,
        title,
      },
      expectedStatus: 202,
    });
    const summary = record(proposed.body.issue, "issue proposal");
    const authorization = record(proposed.body.authorization, "issue authorization");
    const issueId = string(summary.id, "issue id");
    const issueKey = string(summary.key, "issue key");
    const requestEventId = string(authorization.request_event_id, "authorization request id");
    expect(issueKey).toMatch(/^MYL-\d+$/);
    expect(authorization.status).toBe("pending");

    const active = await awaitAuthorizedIssue(
      systemClient,
      requestEventId,
      `issue authorization ${requestEventId}`,
    );
    expect(active).toMatchObject({ id: issueId, title, state_category: "unstarted" });

    const viewed = await reviewerClient.json(`/v1/issues/${encodeURIComponent(issueKey)}`);
    expect(viewed.body).toMatchObject({ id: issueId, title, state_category: "unstarted" });

    const open = await systemClient.json("/v1/issues?state=open&limit=100");
    expect(array(open.body.items, "open issues")).toEqual(
      expect.arrayContaining([expect.objectContaining({ id: issueId, title })]),
    );

    const closed = await systemClient.json(`/v1/issues/${encodeURIComponent(issueKey)}/close`, {
      method: "POST",
      body: {},
    });
    expect(closed.body).toMatchObject({ id: issueId, title, state_category: "completed" });

    const retry = await systemClient.json(`/v1/issues/${encodeURIComponent(issueKey)}/close`, {
      method: "POST",
      body: {},
    });
    expect(retry.body).toMatchObject({ id: issueId, state_category: "completed" });

    const closedList = await systemClient.json("/v1/issues?state=closed&limit=100");
    expect(array(closedList.body.items, "closed issues")).toEqual(
      expect.arrayContaining([expect.objectContaining({ id: issueId, title })]),
    );
  });

  test("lets one issue block another until their dependency is removed", async () => {
    const planning = await awaitActiveIssue(systemClient, uniqueName("Plan the shared release"));
    const delivery = await awaitActiveIssue(systemClient, uniqueName("Ship the shared release"));
    const planningKey = string(planning.key, "planning issue key");
    const planningRef = string(planning.ref, "planning issue ref");
    const deliveryRef = string(delivery.ref, "delivery issue ref");
    const intent = { target_ref: deliveryRef, relation: "blocks" };

    const created = await systemClient.json(
      `/v1/issues/${encodeURIComponent(planningKey)}/relations`,
      {
        method: "POST",
        body: intent,
        idempotencyKey: `issue-relation-${randomUUID()}`,
        expectedStatus: 201,
      },
    );
    const relation = record(created.body.relation, "created issue dependency");
    const relationId = string(relation.id, "issue dependency id");
    expect(created.body).toMatchObject({ created: true, durable: true });
    expect(relation).toMatchObject({
      source_ref: planningRef,
      target_ref: deliveryRef,
      relation: "blocks",
      creator_kind: "human",
    });
    const publicRelationAuthor = expectOpaqueIssueAuthor(
      relation.created_by,
      "created issue dependency author",
    );
    expect(publicRelationAuthor).not.toContain(systemTestConfig.principal);

    const replay = await systemClient.json(
      `/v1/issues/${encodeURIComponent(planningKey)}/relations`,
      { method: "POST", body: intent, expectedStatus: 200 },
    );
    expect(replay.body).toMatchObject({
      created: false,
      relation: { id: relationId },
    });

    const listed = await reviewerClient.json(
      `/v1/issues/${encodeURIComponent(planningKey)}/relations`,
    );
    const visibleRelations = array(listed.body.items, "visible issue dependencies");
    expect(visibleRelations).toEqual([
      expect.objectContaining({
        id: relationId,
        target_ref: deliveryRef,
        relation: "blocks",
        created_by: publicRelationAuthor,
        creator_kind: "human",
      }),
    ]);
    expect(JSON.stringify(visibleRelations)).not.toContain(systemTestConfig.principal);

    expect(await awaitBacklink(systemClient, deliveryRef, planningRef, "blocks")).toMatchObject({
      relation_class: "lifecycle",
      target_ref: deliveryRef,
    });
    expect(await awaitBacklink(systemClient, planningRef, deliveryRef, "blocked_by")).toMatchObject({
      relation_class: "lifecycle",
      target_ref: planningRef,
    });

    const removed = await systemClient.json(
      `/v1/issues/${encodeURIComponent(planningKey)}/relations/${encodeURIComponent(relationId)}`,
      { method: "DELETE" },
    );
    expect(removed.body).toMatchObject({ removed: true, durable: true });

    await awaitBacklinkGone(systemClient, deliveryRef, planningRef, "blocks");
    await awaitBacklinkGone(systemClient, planningRef, deliveryRef, "blocked_by");

    const removalReplay = await systemClient.json(
      `/v1/issues/${encodeURIComponent(planningKey)}/relations/${encodeURIComponent(relationId)}`,
      { method: "DELETE" },
    );
    expect(removalReplay.body).toMatchObject({ relation_id: relationId, removed: false });
  });

});
