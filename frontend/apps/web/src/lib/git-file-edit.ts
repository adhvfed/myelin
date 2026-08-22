import { action, json, redirect } from "@solidjs/router";

import { edgePost, GatewayError, isUnauthorized } from "../server/gateway";
import {
  parseGitFileEditDraft,
  parseGitFileEditReceipt,
  type GitFileEditResult,
} from "./git-file-edit-contract";

export * from "./git-file-edit-contract";

function nestedPath(path: string): string {
  return path.split("/").map(encodeURIComponent).join("/");
}

export const commitGitFile = action(async (value: unknown) => {
  "use server";
  const respond = (result: GitFileEditResult) => json(result, { revalidate: [] });
  const draft = parseGitFileEditDraft(value);
  if (!draft) return respond({ ok: false, error: "bad-input" });
  try {
    const receipt = parseGitFileEditReceipt(await edgePost(
      `/v1/git/repos/${encodeURIComponent(draft.repo)}/blob/${encodeURIComponent(draft.ref)}/${nestedPath(draft.path)}`,
      {
        base_oid: draft.baseOid,
        contents: draft.contents,
        message: draft.message,
      },
      { idempotencyKey: draft.clientNonce },
    ));
    return respond(receipt ? { ok: true, receipt } : { ok: false, error: "error" });
  } catch (error) {
    if (isUnauthorized(error)) throw redirect("/login");
    if (error instanceof GatewayError) {
      if (error.status === 400) return respond({ ok: false, error: "bad-input" });
      if (error.status === 403) return respond({ ok: false, error: "forbidden" });
      if (error.status === 404) return respond({ ok: false, error: "not-found" });
      if (error.status === 409) return respond({ ok: false, error: "conflict" });
      if (error.status === 413) return respond({ ok: false, error: "too-large" });
      if (error.status === 503) return respond({ ok: false, error: "unavailable" });
    }
    return respond({ ok: false, error: "error" });
  }
}, "git-file-commit");
