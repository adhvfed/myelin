import { query, redirect } from "@solidjs/router";

import { edgeGet, isUnauthorized } from "../server/gateway";
import { parseGitPullRequestRef } from "./artifact-ref";
import { isKnowledgeUlid, parseKnowledgePage } from "./knowledge-response";
import { parseIssuesPage } from "./mutation-response";
import {
  parseRelatedRefsPage,
  RELATED_REFS_PAGE_LIMIT,
  type RelatedRefsPage,
} from "./related-ref-response";

export interface LinkedIssueVM {
  id: string;
  key: string;
  title: string;
  state: string;
}

export interface LinkedDocumentVM {
  id: string;
  title: string;
}

export interface PrContextVM {
  issues: LinkedIssueVM[];
  documents: LinkedDocumentVM[];
  issues_unavailable: boolean;
  documents_unavailable: boolean;
  truncated: boolean;
}

type ReadResult<T> = { value: T; failed: false } | { value: null; failed: true };

async function read<T>(path: string, parse: (value: unknown) => T | null): Promise<ReadResult<T>> {
  try {
    const value = parse(await edgeGet(path));
    return value === null ? { value: null, failed: true } : { value, failed: false };
  } catch (error) {
    if (isUnauthorized(error)) throw redirect("/login");
    return { value: null, failed: true };
  }
}

function prRef(value: unknown): value is string {
  return parseGitPullRequestRef(value)?.sub === null;
}

function issueKey(root: string): string | null {
  const match = /^myelin:\/\/[^/]+\/issue\/issue\/([A-Z0-9]{2,10}-[1-9][0-9]{0,18})$/.exec(root);
  return match?.[1] ?? null;
}

function documentId(root: string): string | null {
  const match = /^myelin:\/\/[^/]+\/knowledge\/page\/([^/#]+)$/.exec(root);
  return match && isKnowledgeUlid(match[1]) ? match[1] : null;
}

function distinct<T>(values: Iterable<T>): T[] {
  return [...new Set(values)];
}

async function related(pr: string, direction: "links" | "backlinks"): Promise<ReadResult<RelatedRefsPage>> {
  const page = await read(
    `/v1/refs/${direction}?ref=${encodeURIComponent(pr)}&limit=${RELATED_REFS_PAGE_LIMIT}`,
    parseRelatedRefsPage,
  );
  if (!page.failed && page.value.ref !== pr) return { value: null, failed: true };
  return page;
}

async function resolveIssue(key: string): Promise<ReadResult<LinkedIssueVM>> {
  const page = await read(
    `/v1/issues?state=all&key=${encodeURIComponent(key)}&limit=2`,
    parseIssuesPage,
  );
  const issue = page.value?.items.find((item) => item.key === key);
  return issue
    ? {
      value: { id: issue.id, key: issue.key, title: issue.title, state: issue.state },
      failed: false,
    }
    : { value: null, failed: true };
}

async function resolveDocument(id: string): Promise<ReadResult<LinkedDocumentVM>> {
  const response = await read(
    `/v1/knowledge/pages/${encodeURIComponent(id)}`,
    parseKnowledgePage,
  );
  return response.value
    ? { value: { id: response.value.id, title: response.value.title }, failed: false }
    : { value: null, failed: true };
}

export const getPrContext = query(async (pr: string): Promise<PrContextVM> => {
  "use server";
  if (!prRef(pr)) {
    return {
      issues: [],
      documents: [],
      issues_unavailable: true,
      documents_unavailable: true,
      truncated: false,
    };
  }

  const [links, backlinks] = await Promise.all([
    related(pr, "links"),
    related(pr, "backlinks"),
  ]);
  const pages = [links.value, backlinks.value].filter((page): page is RelatedRefsPage => page !== null);
  const roots = distinct(pages.flatMap((page) => page.items.map((item) => item.root_ref)));
  const issueKeys = distinct(roots.map(issueKey).filter((key): key is string => key !== null));
  const documentIds = distinct(roots.map(documentId).filter((id): id is string => id !== null));

  const [issueResults, documentResults] = await Promise.all([
    Promise.all(issueKeys.map(resolveIssue)),
    Promise.all(documentIds.map(resolveDocument)),
  ]);
  const graphUnavailable = links.failed || backlinks.failed;
  return {
    issues: issueResults.flatMap((result) => result.value ? [result.value] : []),
    documents: documentResults.flatMap((result) => result.value ? [result.value] : []),
    issues_unavailable: graphUnavailable || issueResults.some((result) => result.failed),
    documents_unavailable: graphUnavailable || documentResults.some((result) => result.failed),
    truncated: pages.some((page) => page.page.next_cursor !== null),
  };
}, "git-pr-context");
