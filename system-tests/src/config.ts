import { randomUUID } from "node:crypto";

function required(name: string): string {
  const value = process.env[name]?.trim();
  if (!value) {
    throw new Error(`${name} is required; run the suite through \`fed test:system\``);
  }
  return value;
}

function url(name: string): string {
  const value = required(name).replace(/\/$/, "");
  try {
    return new URL(value).toString().replace(/\/$/, "");
  } catch {
    throw new Error(`${name} must be an absolute URL, received ${JSON.stringify(value)}`);
  }
}

export interface SystemTestConfig {
  edgeUrl: string;
  token: string;
  reviewerToken: string;
  tokenScheme: string;
  tenant: string;
  region: string;
  principal: string;
  reviewerPrincipal: string;
  natsUrl: string;
  runId: string;
  issues: {
    projectId: string;
    typeId: string;
    prefix: string;
  };
}

export const systemTestConfig: SystemTestConfig = Object.freeze({
  edgeUrl: url("MYELIN_SYSTEM_TEST_EDGE_URL"),
  token: required("MYELIN_SYSTEM_TEST_TOKEN"),
  reviewerToken: required("MYELIN_SYSTEM_TEST_REVIEWER_TOKEN"),
  tokenScheme: process.env.MYELIN_SYSTEM_TEST_TOKEN_SCHEME?.trim() || "agent",
  tenant: required("MYELIN_SYSTEM_TEST_TENANT"),
  region: process.env.MYELIN_SYSTEM_TEST_REGION?.trim() || "fr-par",
  principal: required("MYELIN_SYSTEM_TEST_PRINCIPAL"),
  reviewerPrincipal: required("MYELIN_SYSTEM_TEST_REVIEWER_PRINCIPAL"),
  natsUrl: url("NATS_URL"),
  runId: process.env.MYELIN_SYSTEM_TEST_RUN_ID?.trim() || randomUUID(),
  issues: Object.freeze({
    projectId: required("MYELIN_SYSTEM_TEST_ISSUES_PROJECT"),
    typeId: required("MYELIN_SYSTEM_TEST_ISSUES_TYPE"),
    prefix: required("MYELIN_SYSTEM_TEST_ISSUES_PREFIX"),
  }),
});

export function gitRepositoryUrl(slug: string): string {
  const path = [systemTestConfig.tenant, systemTestConfig.region, `${slug}.git`]
    .map(encodeURIComponent)
    .join("/");
  return new URL(`/${path}`, systemTestConfig.edgeUrl).toString();
}
