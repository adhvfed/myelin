/** Process liveness: dependency failures belong to /readyz so an orchestrator can remove the replica
 * from service without converting a recoverable Valkey outage into a restart loop. */
export function GET(): Response {
  return Response.json(
    { status: "ok" },
    { headers: { "cache-control": "no-store" } },
  );
}
