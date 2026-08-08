import { sessionStoreReady } from "~/server/session";
import { oidcTransactionStoreReady } from "~/server/oidc";

/** Traffic readiness includes the shared session backend: accepting authenticated requests without
 * access to revocation/rotation state would be both misleading and unsafe. */
export async function GET(): Promise<Response> {
  try {
    await Promise.all([sessionStoreReady(), oidcTransactionStoreReady()]);
    return Response.json(
      { status: "ready" },
      { headers: { "cache-control": "no-store" } },
    );
  } catch {
    return Response.json(
      { status: "unavailable" },
      {
        status: 503,
        headers: { "cache-control": "no-store", "retry-after": "5" },
      },
    );
  }
}
