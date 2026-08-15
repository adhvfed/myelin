import { action, json, query, redirect } from "@solidjs/router";
import { edgeGet, edgePost, GatewayError, isUnauthorized } from "../server/gateway";
import {
  parseNewProjectInput,
  parseProjectCreation,
  parseProjectListInput,
  parseProjectPage,
  projectListSearchParams,
  type NewProjectInput,
  type ProjectCreationReceipt,
  type ProjectListInput,
  type ProjectPage,
} from "./project-contract";

export type ProjectErrorKind = "bad-input" | "conflict" | "not-found" | "unavailable" | "error";
export type ProjectListResult =
  | { ok: true; page: ProjectPage }
  | { ok: false; error: ProjectErrorKind };
export type ProjectCreateResult =
  | { ok: true; receipt: ProjectCreationReceipt }
  | { ok: false; error: ProjectErrorKind };

function errorKind(error: unknown): ProjectErrorKind {
  if (!(error instanceof GatewayError)) return "error";
  if (error.status === 400) return "bad-input";
  if (error.status === 404 || error.status === 403) return "not-found";
  if (error.status === 409) return "conflict";
  if (error.status === 503) return "unavailable";
  return "error";
}

export const getProjects = query(async (value: ProjectListInput = {}): Promise<ProjectListResult> => {
  "use server";
  const input = parseProjectListInput(value);
  if (!input) return { ok: false, error: "bad-input" };
  try {
    const queryString = projectListSearchParams(input).toString();
    const page = parseProjectPage(await edgeGet(`/v1/projects${queryString ? `?${queryString}` : ""}`));
    return page ? { ok: true, page } : { ok: false, error: "error" };
  } catch (error) {
    if (isUnauthorized(error)) throw redirect("/login");
    return { ok: false, error: errorKind(error) };
  }
}, "projects-list");

export const createProject = action(async (value: NewProjectInput) => {
  "use server";
  const result = (response: ProjectCreateResult) => json(response, { revalidate: [] });
  const input = parseNewProjectInput(value);
  if (!input) return result({ ok: false, error: "bad-input" });
  try {
    const receipt = parseProjectCreation(await edgePost(
      "/v1/projects",
      { name: input.name, issue_prefix: input.issuePrefix },
      { idempotencyKey: input.clientNonce },
    ));
    return result(receipt ? { ok: true, receipt } : { ok: false, error: "error" });
  } catch (error) {
    if (isUnauthorized(error)) throw redirect("/login");
    return result({ ok: false, error: errorKind(error) });
  }
}, "projects-create");
