import { action, json, query, redirect } from "@solidjs/router";
import { edgeGet, edgePost, edgePut, GatewayError, isUnauthorized } from "../server/gateway";
import {
  isKnowledgeUlid, parseKnowledgeCreateDraft, parseKnowledgeCreateReceipt, parseKnowledgePage,
  parseKnowledgePages, parseKnowledgeSaveDraft, parseKnowledgeSaveReceipt,
  type KnowledgeCreateDraft, type KnowledgeCreateReceipt, type KnowledgePage,
  type KnowledgePageList, type KnowledgeSaveDraft, type KnowledgeSaveReceipt,
} from "./knowledge-response";

export * from "./knowledge-response";
export type KnowledgeErrorKind = "bad-input" | "not-found" | "conflict" | "unavailable" | "error";

export class KnowledgeRouteError extends Error {
  constructor(readonly kind: KnowledgeErrorKind) { super(`KNOWLEDGE_ERR:${kind}`); }
}

async function authed<T>(run: () => Promise<T>): Promise<T> {
  try { return await run(); } catch (error) {
    if (isUnauthorized(error)) throw redirect("/login");
    if (error instanceof GatewayError) {
      if (error.status === 400) throw new KnowledgeRouteError("bad-input");
      if (error.status === 404) throw new KnowledgeRouteError("not-found");
      if (error.status === 409) throw new KnowledgeRouteError("conflict");
      if (error.status === 503) throw new KnowledgeRouteError("unavailable");
    }
    if (error instanceof KnowledgeRouteError) throw error;
    throw new KnowledgeRouteError("error");
  }
}

export const getKnowledgePages = query(async (request: { cursor?: string; limit?: number } = {}): Promise<KnowledgePageList> => {
  "use server";
  if (!request || typeof request !== "object" || Array.isArray(request) ||
      Object.keys(request).some((key) => !["cursor", "limit"].includes(key)) ||
      (request.cursor !== undefined && !isKnowledgeUlid(request.cursor)) ||
      (request.limit !== undefined && (!Number.isSafeInteger(request.limit) || request.limit < 1 || request.limit > 100))) {
    throw new KnowledgeRouteError("bad-input");
  }
  const search = new URLSearchParams();
  if (request.cursor) search.set("cursor", request.cursor);
  if (request.limit) search.set("limit", String(request.limit));
  return authed(async () => {
    const parsed = parseKnowledgePages(await edgeGet(`/v1/knowledge/pages${search.size ? `?${search}` : ""}`));
    if (!parsed) throw new KnowledgeRouteError("error");
    return parsed;
  });
}, "knowledge-pages");

export const getKnowledgePage = query(async (pageId: string): Promise<KnowledgePage> => {
  "use server";
  if (!isKnowledgeUlid(pageId)) throw new KnowledgeRouteError("bad-input");
  return authed(async () => {
    const parsed = parseKnowledgePage(await edgeGet(`/v1/knowledge/pages/${encodeURIComponent(pageId)}`));
    if (!parsed || parsed.id !== pageId) throw new KnowledgeRouteError("error");
    return parsed;
  });
}, "knowledge-page");

export type KnowledgeMutation = ({ op: "create" } & KnowledgeCreateDraft) | ({ op: "save" } & KnowledgeSaveDraft);
export type KnowledgeMutationResult =
  | { ok: true; op: "create"; receipt: KnowledgeCreateReceipt }
  | { ok: true; op: "save"; receipt: KnowledgeSaveReceipt }
  | { ok: false; error: KnowledgeErrorKind };

export const knowledgeMutate = action(async (mutation: KnowledgeMutation) => {
  "use server";
  const respond = (value: KnowledgeMutationResult) => json(value, { revalidate: [] });
  try {
    if (!mutation || typeof mutation !== "object") return respond({ ok: false, error: "bad-input" });
    if (mutation.op === "create") {
      const draft = parseKnowledgeCreateDraft({ title: mutation.title, template: mutation.template, visibility: mutation.visibility, clientNonce: mutation.clientNonce });
      if (!draft || Object.keys(mutation).some((key) => !["op", "title", "template", "visibility", "clientNonce"].includes(key))) return respond({ ok: false, error: "bad-input" });
      const receipt = await authed(async () => {
        const parsed = parseKnowledgeCreateReceipt(await edgePost("/v1/knowledge/pages", {
          title: draft.title, template: draft.template, visibility: draft.visibility,
        }, { idempotencyKey: draft.clientNonce }));
        if (!parsed) throw new KnowledgeRouteError("error");
        return parsed;
      });
      return respond({ ok: true, op: "create", receipt });
    }
    if (mutation.op === "save") {
      const draft = parseKnowledgeSaveDraft({ pageId: mutation.pageId, expectedVersion: mutation.expectedVersion, title: mutation.title, visibility: mutation.visibility, blocks: mutation.blocks });
      if (!draft || Object.keys(mutation).some((key) => !["op", "pageId", "expectedVersion", "title", "visibility", "blocks"].includes(key))) return respond({ ok: false, error: "bad-input" });
      const receipt = await authed(async () => {
        const parsed = parseKnowledgeSaveReceipt(await edgePut(`/v1/knowledge/pages/${encodeURIComponent(draft.pageId)}`, {
          expected_version: draft.expectedVersion,
          title: draft.title,
          visibility: draft.visibility,
          blocks: draft.blocks.map((block) => ({
            id: block.id,
            type: block.type,
            markdown: block.markdown,
            references: block.references ?? [],
            state: block.state ?? "active",
          })),
        }));
        if (!parsed) throw new KnowledgeRouteError("error");
        return parsed;
      });
      return respond({ ok: true, op: "save", receipt });
    }
    return respond({ ok: false, error: "bad-input" });
  } catch (error) {
    if (error instanceof KnowledgeRouteError) return respond({ ok: false, error: error.kind });
    throw error;
  }
}, "knowledge-mutate");
