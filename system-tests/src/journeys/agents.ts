// Agent journeys: speak MCP through one short-lived run identity while keeping
// its bearer out of every assertion and rendered response. These helpers make
// a story describe what the agent reads or changes, not JSON-RPC ceremony.
import { randomUUID } from "node:crypto";

import { expect } from "vitest";

import { systemClient } from "../context.js";
import { array, record, string, type JsonRecord } from "../json.js";
import type { SystemTestClient } from "../client.js";

export interface AgentRunEnvelope {
  run: {
    id: string;
    ref: string;
    agent_id: string;
    agent_ref: string;
    principal_id: string;
    trigger_actor: string;
    selected_tools: Array<{ name: string; version: number; ref: string }>;
    effective_grants: string[];
    state: string;
    issued_at: string;
    expires_at: string;
  };
  credential: { scheme: string; token: string; expires_at: string };
  created: boolean;
  durable: boolean;
}

export interface AgentThreadRunEnvelope extends AgentRunEnvelope {
  run: AgentRunEnvelope["run"] & {
    context: {
      thread_id: string;
      thread_ref: string;
      conversation_id: string;
      conversation_ref: string;
      workspace: {
        id: string;
        generation: number;
        expires_at: string;
      };
    };
  };
}

export interface ActivatedAgentEnvelope {
  agent: {
    id: string;
    ref: string;
    principal_id: string;
    name: string;
    runtime_ref: string;
    on_behalf_of: string;
    status: string;
    selected_tools: Array<{ name: string; version: number; ref: string }>;
    effective_tools: Array<{ name: string; version: number; ref: string }>;
    grants: string[];
  };
  created: boolean;
  durable: boolean;
}

export async function activateExternalAgent(
  client: SystemTestClient,
  name: string,
  tools: string[],
): Promise<ActivatedAgentEnvelope> {
  const response = await client.json("/v1/agents", {
    method: "POST",
    body: { name, tools },
    idempotencyKey: `agent-${randomUUID()}`,
    expectedStatus: 201,
  });
  return response.body as unknown as ActivatedAgentEnvelope;
}

export async function beginAgentRun(
  client: SystemTestClient,
  agentId: string,
): Promise<AgentRunEnvelope> {
  const response = await client.json(`/v1/agents/${encodeURIComponent(agentId)}/runs`, {
    method: "POST",
    body: {},
    idempotencyKey: `agent-run-${randomUUID()}`,
    expectedStatus: 201,
  });
  return response.body as unknown as AgentRunEnvelope;
}

export async function beginAgentThreadRun(
  client: SystemTestClient,
  threadId: string,
  options: { idempotencyKey?: string; expectedStatus?: 200 | 201 } = {},
): Promise<AgentThreadRunEnvelope> {
  const response = await client.json(
    `/v1/agent-threads/${encodeURIComponent(threadId)}/runs`,
    {
      method: "POST",
      body: {},
      idempotencyKey: options.idempotencyKey ?? `agent-thread-run-${randomUUID()}`,
      expectedStatus: options.expectedStatus ?? 201,
    },
  );
  return response.body as unknown as AgentThreadRunEnvelope;
}

export async function closeAgentRun(run: AgentRunEnvelope): Promise<void> {
  await systemClient.json(`/v1/agent-runs/${encodeURIComponent(run.run.id)}/close`, {
    method: "POST",
    body: {},
    token: run.credential.token,
    tokenScheme: "agent",
    expectedStatus: 200,
  });
}

export async function findAgentPageItem(
  run: AgentRunEnvelope,
  firstRequestId: number,
  tool: string,
  arguments_: JsonRecord,
  predicate: (item: JsonRecord) => boolean,
  description: string,
): Promise<JsonRecord> {
  let cursor: string | undefined;
  const visited = new Set<string>();
  for (let pageNumber = 0; pageNumber < 100; pageNumber += 1) {
    const payload = await askAgent(run, firstRequestId + pageNumber, tool, {
      ...arguments_,
      ...(cursor === undefined ? {} : { cursor }),
    });
    const found = array(payload.items, `${description} items`)
      .map((item) => record(item, description))
      .find(predicate);
    if (found) return found;

    const next = record(payload.page, `${description} page`).next_cursor;
    if (next === null) break;
    cursor = string(next, `${description} next cursor`);
    if (visited.has(cursor)) throw new Error(`${description} repeated cursor ${cursor}`);
    visited.add(cursor);
  }
  throw new Error(`${description} was absent after walking every agent-visible page`);
}

export async function askAgent(
  run: AgentRunEnvelope,
  id: number,
  tool: string,
  arguments_: JsonRecord,
): Promise<JsonRecord> {
  const result = await callAgent(run, id, tool, arguments_);
  expect(result, `${tool} MCP call failed: ${JSON.stringify(result)}`).toMatchObject({
    isError: false,
    _meta: { tool },
  });
  const content = array(result.content, `${tool} MCP content`);
  expect(content).toHaveLength(1);
  const text = string(record(content[0], `${tool} MCP content item`).text, `${tool} MCP text`);
  expect(text).not.toContain(run.credential.token);
  return record(JSON.parse(text), `${tool} payload`);
}

export async function askAgentToAct(
  run: AgentRunEnvelope,
  id: number,
  tool: string,
  arguments_: JsonRecord,
  idempotencyKey: string = `system-${tool}-${randomUUID()}`,
  approvalGateId?: string,
): Promise<JsonRecord> {
  const result = await callAgent(run, id, tool, arguments_, {
    idempotencyKey,
    ...(approvalGateId === undefined ? {} : { approvalGateId }),
  });
  expect(result, `${tool} MCP call failed: ${publicMcpContent(result, tool)}`).toMatchObject({
    isError: false,
    _meta: { tool, eventId: expect.any(String) },
  });
  const metadata = record(result._meta, `${tool} MCP result metadata`);
  const receipt = record(result.structuredContent, `${tool} MCP structured receipt`);
  expect(receipt).toMatchObject({ event_id: metadata.eventId });
  return receipt;
}

function publicMcpContent(result: JsonRecord, tool: string): string {
  try {
    return array(result.content, `${tool} MCP content`)
      .map((item) => string(record(item, `${tool} MCP content item`).text, `${tool} MCP text`))
      .join("\n");
  } catch {
    return "malformed MCP result";
  }
}

export async function askAgentToRequestApproval(
  run: AgentRunEnvelope,
  id: number,
  tool: string,
  arguments_: JsonRecord,
  idempotencyKey: string,
): Promise<string> {
  const result = await callAgent(run, id, tool, arguments_, { idempotencyKey });
  expect(result).toMatchObject({
    isError: false,
    _meta: { tool, gateId: expect.any(String) },
  });
  expect(result.structuredContent).toBeUndefined();
  return string(record(result._meta, `${tool} approval metadata`).gateId, `${tool} gate id`);
}

export async function askAgentToBeDenied(
  run: AgentRunEnvelope,
  id: number,
  tool: string,
  arguments_: JsonRecord,
): Promise<string> {
  const result = await callAgent(run, id, tool, arguments_, {
    idempotencyKey: `system-denied-${tool}-${randomUUID()}`,
  });
  expect(result).toMatchObject({ isError: true, _meta: { tool } });
  const content = array(result.content, `${tool} denied MCP content`);
  expect(content).toHaveLength(1);
  return string(record(content[0], `${tool} denied MCP content item`).text, `${tool} denial`);
}

interface AgentCallOptions {
  idempotencyKey?: string;
  approvalGateId?: string;
}

async function callAgent(
  run: AgentRunEnvelope,
  id: number,
  tool: string,
  arguments_: JsonRecord,
  options: AgentCallOptions = {},
): Promise<JsonRecord> {
  const response = await systemClient.json(`/v1/agent-runs/${run.run.id}/mcp`, {
    method: "POST",
    body: {
      jsonrpc: "2.0",
      id,
      method: "tools/call",
      params: {
        name: tool,
        arguments: arguments_,
        ...(options.approvalGateId === undefined
          ? {}
          : { approval: { gateId: options.approvalGateId } }),
        ...(options.idempotencyKey === undefined
          ? {}
          : { _meta: { "com.myelin/idempotencyKey": options.idempotencyKey } }),
      },
    },
    token: run.credential.token,
    tokenScheme: "agent",
    expectedStatus: 200,
  });
  expect(JSON.stringify(response.body)).not.toContain(run.credential.token);
  return record(response.body.result, `${tool} MCP result`);
}
