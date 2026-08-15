import { describe, expect, it } from "vitest";
import {
  parseNewProjectInput,
  parseProjectCreation,
  parseProjectListInput,
  parseProjectPage,
  projectNameError,
  projectPrefixError,
} from "./project-contract";

const project = {
  id: "11111111-1111-4111-8111-111111111111",
  ref: "myelin://acme/identity/project/11111111-1111-4111-8111-111111111111",
  name: "Developer experience",
  issue_prefix: "DX",
  default_issue_type_id: "22222222-2222-4222-8222-222222222222",
  created_at: "2026-08-10T12:00:00.000Z",
};

describe("project browser contract", () => {
  it("admits one exact authorized project page and creation receipt", () => {
    expect(parseProjectPage({
      items: [project],
      page: { next_cursor: project.id, limit: 50 },
    })).toEqual({ items: [project], page: { next_cursor: project.id, limit: 50 } });
    expect(parseProjectCreation({ project, created: true, durable: true }))
      .toEqual({ project, created: true });
  });

  it("rejects response drift, foreign references, and malformed pagination", () => {
    expect(parseProjectPage({ items: [{ ...project, secret: "no" }], page: { next_cursor: null, limit: 50 } }))
      .toBeNull();
    expect(parseProjectPage({
      items: [{ ...project, ref: `myelin://acme/identity/project/aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa` }],
      page: { next_cursor: null, limit: 50 },
    })).toBeNull();
    expect(parseProjectPage({ items: [project], page: { next_cursor: "opaque", limit: 50 } }))
      .toBeNull();
    expect(parseProjectCreation({ project, created: true, durable: false })).toBeNull();
  });

  it("bounds list and create inputs before a URL or write can be built", () => {
    expect(parseProjectListInput({ cursor: project.id, limit: 100 }))
      .toEqual({ cursor: project.id, limit: 100 });
    expect(parseProjectListInput({ cursor: project.id, limit: 101 })).toBeNull();
    expect(parseProjectListInput({ limit: 50, tenant: "other" })).toBeNull();
    expect(parseNewProjectInput({ name: "Developer experience", issuePrefix: "DX", clientNonce: "project_1" }))
      .toEqual({ name: "Developer experience", issuePrefix: "DX", clientNonce: "project_1" });
    expect(parseNewProjectInput({ name: "Developer experience", issuePrefix: "dx", clientNonce: "project_1" })).toBeNull();
    expect(parseNewProjectInput({ name: "Project", issuePrefix: "DX" })).toBeNull();
    expect(parseNewProjectInput({ name: "Project", issuePrefix: "DX", clientNonce: "has spaces" })).toBeNull();
    expect(parseNewProjectInput({ name: "Project", issuePrefix: "DX", clientNonce: "project_1", tenant: "other" })).toBeNull();
  });

  it("gives field-level guidance without silently normalizing identity", () => {
    expect(projectNameError(" Project")).not.toBeNull();
    expect(projectNameError("Project")).toBeNull();
    expect(projectPrefixError("D")).not.toBeNull();
    expect(projectPrefixError("DX2")).toBeNull();
  });
});
