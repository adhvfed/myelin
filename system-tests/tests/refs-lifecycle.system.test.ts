// The reference graph as its own product surface: fan-out, paging discipline,
// and the visibility gate on the TARGET of a query. (That a private source is
// filtered out of a visible target's backlinks is pinned in the chat
// lifecycle; here the probe is aimed at the private artifact itself.)
import { describe, expect, test } from "vitest";

import { reviewerClient, systemClient, uniqueName } from "../src/context.js";
import { eventually } from "../src/eventually.js";
import { Conversation } from "../src/journeys/chat.js";
import { awaitActiveIssue } from "../src/journeys/issues.js";
import { createProject } from "../src/journeys/projects.js";
import { walkPaged } from "../src/paging.js";
import { string } from "../src/json.js";
import { systemTestConfig } from "../src/config.js";

function messageRef(messageId: string): string {
  return `myelin://${systemTestConfig.tenant}/chat/message/${messageId}`;
}

describe("reference graph lifecycle", () => {
  test("walks a fanned-out backlink surface page by page without loss or repetition", async () => {
    const issue = await awaitActiveIssue(systemClient, uniqueName("Widely referenced work"));
    const issueRef = string(issue.ref, "widely referenced issue ref");
    const room = await Conversation.open(systemClient, {
      projectId: systemTestConfig.issues.projectId,
      channel: uniqueName("refs-fanout"),
      topic: "Every message here points at one issue",
    });

    const sourceRefs: string[] = [];
    for (let index = 1; index <= 12; index += 1) {
      const messageId = await room.post(
        systemClient,
        `Update ${index} on the widely referenced work: ￼`,
        { references: [issueRef] },
      );
      sourceRefs.push(messageRef(messageId));
    }

    const walkBacklinks = async (): Promise<Map<string, number>> => {
      const seen = new Map<string, number>();
      for await (const item of walkPaged(
        systemClient,
        `/v1/refs/backlinks?ref=${encodeURIComponent(issueRef)}`,
        { limit: 5, maxPages: 50 },
      )) {
        const root = string(item.root_ref, "backlink root ref");
        seen.set(root, (seen.get(root) ?? 0) + 1);
      }
      return seen;
    };

    const seen = await eventually(async () => {
      const walked = await walkBacklinks();
      return sourceRefs.every((ref) => walked.has(ref)) ? walked : undefined;
    }, {
      description: "all twelve referencing messages to appear as backlinks",
      timeoutMs: 30_000,
      intervalMs: 1_000,
    });
    for (const ref of sourceRefs) {
      expect(seen.get(ref), `${ref} must appear exactly once across the pages`).toBe(1);
    }
  }, 90_000);

  test("walks a message's outbound references the same way", async () => {
    const targets: string[] = [];
    for (const name of ["First cited work", "Second cited work", "Third cited work"]) {
      const issue = await awaitActiveIssue(systemClient, uniqueName(name));
      targets.push(string(issue.ref, `${name} ref`));
    }
    const room = await Conversation.open(systemClient, {
      projectId: systemTestConfig.issues.projectId,
      channel: uniqueName("refs-outbound"),
      topic: "One message cites three works",
    });
    const citing = messageRef(await room.post(
      systemClient,
      "This decision rests on three pieces of work: ￼ ￼ ￼",
      { references: targets },
    ));

    const links = await eventually(async () => {
      const walked = new Set<string>();
      for await (const item of walkPaged(
        systemClient,
        `/v1/refs/links?ref=${encodeURIComponent(citing)}`,
        { limit: 2, maxPages: 20 },
      )) {
        walked.add(string(item.root_ref, "outbound link root ref"));
      }
      return targets.every((ref) => walked.has(ref)) ? walked : undefined;
    }, {
      description: "the citing message to expose all three outbound references",
      timeoutMs: 30_000,
      intervalMs: 1_000,
    });
    for (const ref of targets) expect(links.has(ref)).toBe(true);
  }, 60_000);

  test("refuses the reference surface of an artifact the viewer cannot read", async () => {
    const privateProject = await createProject(systemClient, uniqueName("Reference privacy"));
    const privateRoom = await Conversation.open(systemClient, {
      projectId: privateProject.id,
      channel: uniqueName("refs-private"),
      topic: "Nobody outside may even probe this",
    });
    const issue = await awaitActiveIssue(systemClient, uniqueName("Shared work cited privately"));
    const issueRef = string(issue.ref, "shared issue ref");
    const privateMessage = messageRef(await privateRoom.post(
      systemClient,
      "Privately citing shared work: ￼",
      { references: [issueRef] },
    ));

    // the owner sees the private message's outbound reference surface
    const ownerLinks = await eventually(async () => {
      const walked = new Set<string>();
      for await (const item of walkPaged(
        systemClient,
        `/v1/refs/links?ref=${encodeURIComponent(privateMessage)}`,
      )) {
        walked.add(string(item.root_ref, "owner-visible link root"));
      }
      return walked.has(issueRef) ? walked : undefined;
    }, { description: "the owner-visible outbound reference", timeoutMs: 30_000, intervalMs: 1_000 });
    expect(ownerLinks.has(issueRef)).toBe(true);

    // a peer probing the private artifact's surface learns nothing - not even
    // that the artifact exists (404 on both directions, indistinguishable
    // from a reference that was never minted)
    for (const direction of ["links", "backlinks"]) {
      const probed = await reviewerClient.json(
        `/v1/refs/${direction}?ref=${encodeURIComponent(privateMessage)}`,
        { expectedStatus: 404 },
      );
      expect(probed.body).toMatchObject({ error: { code: "not_found" } });
    }
  }, 60_000);
});
