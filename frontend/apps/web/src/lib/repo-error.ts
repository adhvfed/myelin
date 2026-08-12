/** Repository-route states. Capacity is distinct from a transient failure: retrying an
 * intentionally bounded interactive diff cannot make it smaller. */
export type RepoErrorKind =
  | "no-access"
  | "not-found"
  | "stale-tree"
  | "diff-too-large"
  | "error";

/** Message prefix carrying the kind across the server→client serialization boundary. */
export const REPO_ERR_PREFIX = "REPO_ERR:";

/** A git-surface route error carrying only a safe presentation category, never raw Edge detail. */
export class RepoRouteError extends Error {
  readonly kind: RepoErrorKind;

  constructor(kind: RepoErrorKind) {
    super(`${REPO_ERR_PREFIX}${kind}`);
    this.name = "RepoRouteError";
    this.kind = kind;
  }
}

/** Shared repository-read mapping. Anti-oracle policy may deliberately collapse a deny to 404. */
export function mapStatusToKind(status: number): RepoErrorKind {
  if (status === 403) return "no-access";
  if (status === 404) return "not-found";
  return "error";
}

/** PR diffs additionally distinguish the Edge's bounded interactive-capacity response. */
export function mapPrDiffStatusToKind(status: number): RepoErrorKind {
  return status === 413 ? "diff-too-large" : mapStatusToKind(status);
}
