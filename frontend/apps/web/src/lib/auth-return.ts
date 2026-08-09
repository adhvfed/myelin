export const DEFAULT_AUTH_RETURN_TO = "/git/repos";
export const MAX_AUTH_RETURN_TO_BYTES = 2_048;

const LOCAL_ORIGIN = "https://myelin.invalid";

/**
 * Reduce a caller-supplied post-auth destination to a canonical, same-origin path.
 *
 * Authentication entry points accept this value from a browser form, so it must never become an
 * open redirect or a response-header injection primitive. Invalid input quietly falls back to the
 * normal application landing page; callers should not have to implement their own partial checks.
 */
export function safeAuthReturnTo(value: unknown): string {
  if (typeof value !== "string" || value.length === 0 || value !== value.trim()) {
    return DEFAULT_AUTH_RETURN_TO;
  }
  if (
    value.length > MAX_AUTH_RETURN_TO_BYTES ||
    new TextEncoder().encode(value).byteLength > MAX_AUTH_RETURN_TO_BYTES ||
    !value.startsWith("/") ||
    value.startsWith("//") ||
    value.includes("\\") ||
    value.includes("#") ||
    hasControlCharacter(value)
  ) {
    return DEFAULT_AUTH_RETURN_TO;
  }

  try {
    const url = new URL(value, LOCAL_ORIGIN);
    if (
      url.origin !== LOCAL_ORIGIN ||
      url.username ||
      url.password ||
      url.hash ||
      /%(?:2f|5c)/i.test(url.pathname)
    ) {
      return DEFAULT_AUTH_RETURN_TO;
    }
    return `${url.pathname}${url.search}`;
  } catch {
    return DEFAULT_AUTH_RETURN_TO;
  }
}

/** Preserve the safe return path while an authentication attempt returns to the login page. */
export function authFailureDestination(error: string, returnTo: unknown): string {
  const params = new URLSearchParams({ error });
  const safeReturnTo = safeAuthReturnTo(returnTo);
  if (safeReturnTo !== DEFAULT_AUTH_RETURN_TO) params.set("return_to", safeReturnTo);
  return `/login?${params.toString()}`;
}

/** Send a logged-out browser to the chooser, retaining only a non-default local destination. */
export function authenticationDestination(returnTo: unknown): string {
  const safeReturnTo = safeAuthReturnTo(returnTo);
  if (safeReturnTo === DEFAULT_AUTH_RETURN_TO) return "/login";
  return `/login?${new URLSearchParams({ return_to: safeReturnTo }).toString()}`;
}

function hasControlCharacter(value: string): boolean {
  for (const character of value) {
    const codePoint = character.codePointAt(0)!;
    if (codePoint <= 0x1f || codePoint === 0x7f) return true;
  }
  return false;
}
