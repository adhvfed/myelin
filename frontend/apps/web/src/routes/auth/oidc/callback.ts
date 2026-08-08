import type { APIEvent } from "@solidjs/start/server";

import { completeOidcLogin } from "~/server/oidc";

export async function GET(event: APIEvent): Promise<Response> {
  const success = await completeOidcLogin(event.request.url).catch(() => false);
  return new Response(null, {
    status: 302,
    headers: {
      location: success ? "/git/repos" : "/login?error=sso_failed",
      "cache-control": "no-store",
      pragma: "no-cache",
    },
  });
}
