export type TransportVerdict = "allow" | "redirect" | "reject";

/**
 * Production runs behind a private TLS terminator that replaces `X-Forwarded-Proto`. Redirect only
 * idempotent navigations; never replay an unsafe request body from HTTP to HTTPS.
 */
export function transportVerdict(
  production: boolean,
  method: string,
  forwardedProto: string | null,
): TransportVerdict {
  if (!production) return "allow";
  if (forwardedProto?.trim().toLowerCase() === "https") return "allow";
  const normalized = method.toUpperCase();
  return normalized === "GET" || normalized === "HEAD" ? "redirect" : "reject";
}
