import { describe, expect, it } from "vitest";
import { issueTargetFromEnv } from "./issue-target";

const configured = {
  MYELIN_ISSUES_PROJECT: "20aee030-c7fa-4757-8243-700faf528690",
  MYELIN_ISSUES_TYPE: "7d457754-f6a1-4cd8-8738-21751570b627",
  MYELIN_ISSUES_PREFIX: "MYL",
};

describe("issueTargetFromEnv", () => {
  it("returns the configured default issue destination", () => {
    expect(issueTargetFromEnv(configured)).toEqual({
      project_id: configured.MYELIN_ISSUES_PROJECT,
      type_id: configured.MYELIN_ISSUES_TYPE,
      prefix: configured.MYELIN_ISSUES_PREFIX,
    });
  });

  it("trims deployment values", () => {
    expect(issueTargetFromEnv({
      MYELIN_ISSUES_PROJECT: ` ${configured.MYELIN_ISSUES_PROJECT} `,
      MYELIN_ISSUES_TYPE: ` ${configured.MYELIN_ISSUES_TYPE} `,
      MYELIN_ISSUES_PREFIX: " MYL ",
    })).toEqual({
      project_id: configured.MYELIN_ISSUES_PROJECT,
      type_id: configured.MYELIN_ISSUES_TYPE,
      prefix: "MYL",
    });
  });

  it.each([
    {},
    { MYELIN_ISSUES_PROJECT: configured.MYELIN_ISSUES_PROJECT },
    { ...configured, MYELIN_ISSUES_PROJECT: "not-a-uuid" },
    { ...configured, MYELIN_ISSUES_TYPE: "7D457754-F6A1-4CD8-8738-21751570B627" },
    { ...configured, MYELIN_ISSUES_PREFIX: "myl" },
    { ...configured, MYELIN_ISSUES_PREFIX: "A" },
    { ...configured, MYELIN_ISSUES_PREFIX: "TOO-LONG-PREFIX" },
  ])("rejects missing, partial, or malformed configuration", (env) => {
    expect(issueTargetFromEnv(env)).toBeNull();
  });
});
