import type { APIEvent } from "@solidjs/start/server";

import { authFailureDestination } from "~/lib/auth-return";
import { completeOidcLogin } from "~/server/oidc";

export async function GET(event: APIEvent): Promise<Response> {
  const outcome = await completeOidcLogin(event.request.url).catch(() => null);
  const destination = outcome?.authenticated
    ? outcome.returnTo
    : authFailureDestination("sso_failed", outcome?.returnTo);
  return new Response(null, {
    status: 302,
    headers: {
      location: destination,
      "cache-control": "no-store",
      pragma: "no-cache",
    },
  });
}
