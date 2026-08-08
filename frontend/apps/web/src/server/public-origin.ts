export interface PublicOriginOptions {
  production: boolean;
  configured?: string;
  requestUrl?: string;
}

function httpUrl(value: string, label: string): URL {
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    throw new Error(`${label} must be an absolute HTTP(S) URL`);
  }
  if (url.protocol !== "http:" && url.protocol !== "https:") {
    throw new Error(`${label} must be an absolute HTTP(S) URL`);
  }
  return url;
}

/** Resolve the one public origin against which unsafe browser requests are verified. */
export function canonicalPublicOrigin(options: PublicOriginOptions): string {
  const configured = options.configured?.trim();
  if (configured) {
    const url = httpUrl(configured, "MYELIN_PUBLIC_ORIGIN");
    if (
      url.username ||
      url.password ||
      url.pathname !== "/" ||
      url.search ||
      url.hash
    ) {
      throw new Error("MYELIN_PUBLIC_ORIGIN must contain only scheme, host, and optional port");
    }
    if (options.production && url.protocol !== "https:") {
      throw new Error("MYELIN_PUBLIC_ORIGIN must use HTTPS in production");
    }
    return url.origin;
  }

  if (options.production) {
    throw new Error("MYELIN_PUBLIC_ORIGIN is required in production");
  }
  if (!options.requestUrl) {
    throw new Error("request URL is required when MYELIN_PUBLIC_ORIGIN is not configured");
  }
  return httpUrl(options.requestUrl, "request URL").origin;
}
