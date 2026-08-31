import { randomUUID } from "node:crypto";

import type { SystemTestClient } from "../client.js";
import { array, record, string, type JsonRecord } from "../json.js";

export interface AgentThreadWorkspace {
  id: string;
  generation: number;
  state: string;
  retention_days: number;
  expires_at: string;
  repository: AgentThreadWorkspaceRepository | null;
  environment: AgentThreadWorkspaceEnvironment | null;
}

export interface AgentThreadWorkspaceRepository {
  slug: string;
  base_ref: string;
  base_oid: string;
  branch: string;
}

export interface AgentThreadWorkspaceEnvironment {
  provider: string;
  config_path: string;
  content_digest: string;
  state: string;
}

export interface PrivateAgentThread {
  id: string;
  ref: string;
  name: string;
  agent_id: string;
  agent_ref: string;
  project_id: string | null;
  conversation_id: string;
  conversation_ref: string;
  review_state: "draft" | "ready";
  review_ready_at: string | null;
  workspace: AgentThreadWorkspace;
  created_at: string;
  updated_at: string;
}

export interface WorkspaceSshAccess {
  host: string;
  port: number;
  username: string;
  expires_at: string;
  public_key_fingerprint: string;
  host_public_key: string;
  host_key_fingerprint: string;
}

export interface WorkspaceSshGrant {
  access: WorkspaceSshAccess;
  workspace: { id: string; generation: number };
  created: boolean;
}

export async function startPrivateAgentThread(
  client: SystemTestClient,
  intent: {
    name: string;
    agentId: string;
    projectId?: string;
    retentionDays?: number;
    idempotencyKey?: string;
    expectedStatus?: 200 | 201;
    workspace?: {
      repository: string;
      baseRef?: string;
      branch?: string;
    };
  },
): Promise<{ thread: PrivateAgentThread; created: boolean }> {
  const response = await client.json("/v1/agent-threads", {
    method: "POST",
    body: {
      name: intent.name,
      agent_id: intent.agentId,
      ...(intent.projectId === undefined ? {} : { project_id: intent.projectId }),
      ...(intent.retentionDays === undefined ? {} : { retention_days: intent.retentionDays }),
      ...(intent.workspace === undefined
        ? {}
        : {
            workspace: {
              repository: intent.workspace.repository,
              ...(intent.workspace.baseRef === undefined
                ? {}
                : { base_ref: intent.workspace.baseRef }),
              ...(intent.workspace.branch === undefined
                ? {}
                : { branch: intent.workspace.branch }),
            },
          }),
    },
    idempotencyKey: intent.idempotencyKey ?? `agent-thread-${randomUUID()}`,
    expectedStatus: intent.expectedStatus ?? 201,
  });
  return {
    thread: parsePrivateAgentThread(response.body.thread),
    created: response.body.created === true,
  };
}

export async function listPrivateAgentThreads(
  client: SystemTestClient,
): Promise<PrivateAgentThread[]> {
  const response = await client.json("/v1/agent-threads?limit=100");
  return array(response.body.items, "private agent threads")
    .map((item) => parsePrivateAgentThread(item));
}

export async function retryPrivateAgentThread(
  client: SystemTestClient,
  threadId: string,
): Promise<{ thread: PrivateAgentThread; created: boolean }> {
  const response = await client.json(
    `/v1/agent-threads/${encodeURIComponent(threadId)}/retry`,
    {
      method: "POST",
      body: {},
      expectedStatus: 200,
    },
  );
  return {
    thread: parsePrivateAgentThread(response.body.thread),
    created: response.body.created === true,
  };
}

export async function markAgentThreadReadyForReview(
  client: SystemTestClient,
  threadId: string,
): Promise<{ thread: PrivateAgentThread; changed: boolean }> {
  const response = await client.json(
    `/v1/agent-threads/${encodeURIComponent(threadId)}/ready`,
    { method: "POST", body: {}, expectedStatus: 200 },
  );
  return {
    thread: parsePrivateAgentThread(response.body.thread),
    changed: response.body.changed === true,
  };
}

export async function requestWorkspaceSshAccess(
  client: SystemTestClient,
  threadId: string,
  publicKey: string,
  options: {
    idempotencyKey?: string;
    expectedStatus?: 200 | 201;
  } = {},
): Promise<WorkspaceSshGrant> {
  const response = await client.json(
    `/v1/agent-threads/${encodeURIComponent(threadId)}/ssh-access`,
    {
      method: "POST",
      body: { public_key: publicKey },
      idempotencyKey: options.idempotencyKey ?? `workspace-ssh-${randomUUID()}`,
      expectedStatus: options.expectedStatus ?? 201,
    },
  );
  const access = record(response.body.access, "workspace SSH access");
  const workspace = record(response.body.workspace, "workspace SSH workspace");
  return {
    access: {
      host: string(access.host, "workspace SSH host"),
      port: positiveInteger(access.port, "workspace SSH port"),
      username: string(access.username, "workspace SSH username"),
      expires_at: string(access.expires_at, "workspace SSH expiry"),
      public_key_fingerprint: string(
        access.public_key_fingerprint,
        "workspace SSH public key fingerprint",
      ),
      host_public_key: string(access.host_public_key, "workspace SSH host public key"),
      host_key_fingerprint: string(
        access.host_key_fingerprint,
        "workspace SSH host key fingerprint",
      ),
    },
    workspace: {
      id: string(workspace.id, "workspace SSH workspace id"),
      generation: positiveInteger(workspace.generation, "workspace SSH workspace generation"),
    },
    created: response.body.created === true,
  };
}

export function parsePrivateAgentThread(value: unknown): PrivateAgentThread {
  const thread = record(value, "private agent thread");
  const workspace = record(thread.workspace, "private agent workspace");
  const repository = workspace.repository === null
    ? null
    : record(workspace.repository, "private agent workspace repository");
  const environment = workspace.environment === null
    ? null
    : record(workspace.environment, "private agent workspace environment");
  return {
    id: string(thread.id, "agent thread id"),
    ref: string(thread.ref, "agent thread ref"),
    name: string(thread.name, "agent thread name"),
    agent_id: string(thread.agent_id, "thread agent id"),
    agent_ref: string(thread.agent_ref, "thread agent ref"),
    project_id: thread.project_id === null ? null : string(thread.project_id, "thread project id"),
    conversation_id: string(thread.conversation_id, "thread conversation id"),
    conversation_ref: string(thread.conversation_ref, "thread conversation ref"),
    review_state: reviewState(thread.review_state),
    review_ready_at: thread.review_ready_at === null
      ? null
      : string(thread.review_ready_at, "thread review-ready time"),
    workspace: {
      id: string(workspace.id, "agent workspace id"),
      generation: positiveInteger(workspace.generation, "agent workspace generation"),
      state: string(workspace.state, "agent workspace state"),
      retention_days: positiveInteger(workspace.retention_days, "workspace retention days"),
      expires_at: string(workspace.expires_at, "workspace expiry"),
      repository: repository === null
        ? null
        : {
            slug: string(repository.slug, "workspace repository slug"),
            base_ref: string(repository.base_ref, "workspace repository base ref"),
            base_oid: string(repository.base_oid, "workspace repository base oid"),
            branch: string(repository.branch, "workspace repository branch"),
          },
      environment: environment === null
        ? null
        : {
            provider: string(environment.provider, "workspace environment provider"),
            config_path: string(environment.config_path, "workspace environment config path"),
            content_digest: string(
              environment.content_digest,
              "workspace environment content digest",
            ),
            state: string(environment.state, "workspace environment state"),
          },
    },
    created_at: string(thread.created_at, "agent thread creation time"),
    updated_at: string(thread.updated_at, "agent thread update time"),
  };
}

function reviewState(value: unknown): "draft" | "ready" {
  if (value !== "draft" && value !== "ready") {
    throw new TypeError("agent thread review state must be draft or ready");
  }
  return value;
}

function positiveInteger(value: unknown, context: string): number {
  if (!Number.isSafeInteger(value) || (value as number) < 1) {
    throw new TypeError(`${context} must be a positive integer`);
  }
  return value as number;
}
