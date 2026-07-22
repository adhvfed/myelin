const DEVELOPMENT_EDGE_ORIGIN = "http://127.0.0.1:8787";

export interface EdgeOriginOptions {
  production: boolean;
  configured: string | undefined;
}

/** Resolve the one trusted server-side edge origin. Production must opt in explicitly so a missing
 * deployment setting can never route product traffic to the local development seam. */
export function canonicalEdgeOrigin(options: EdgeOriginOptions): string {
  const configured = options.configured?.trim();
  if (!configured) {
    if (options.production) {
      throw new Error("MYELIN_EDGE_URL is required in production");
    }
    return DEVELOPMENT_EDGE_ORIGIN;
  }

  let url: URL;
  try {
    url = new URL(configured);
  } catch {
    throw new Error("MYELIN_EDGE_URL must be an absolute HTTP(S) origin");
  }

  if (url.protocol !== "http:" && url.protocol !== "https:") {
    throw new Error("MYELIN_EDGE_URL must use http:// or https://");
  }
  if (options.production && url.protocol !== "https:") {
    throw new Error("MYELIN_EDGE_URL must use https:// in production");
  }
  if (url.username || url.password) {
    throw new Error("MYELIN_EDGE_URL must not contain credentials");
  }
  if (url.pathname !== "/" || url.search || url.hash) {
    throw new Error("MYELIN_EDGE_URL must contain only an origin, without path, query, or fragment");
  }
  return url.origin;
}

export function edgeOrigin(): string {
  return canonicalEdgeOrigin({
    production: process.env.NODE_ENV === "production",
    configured: process.env.MYELIN_EDGE_URL,
  });
}
