// The middleware's server-function guard (lib/csrf.ts) knows the handler is mounted at
// `/_server` under an app served from `/`. Check that at startup, so a changed base URL fails
// loudly instead of quietly moving the handler out from under the guard. Both the routers' `base`
// and Nitro's `baseURL` (`NITRO_APP_BASE_URL`, applied when the server starts) can move it, so the
// check also reads where the server actually mounted it.

interface RouterConfig {
  name?: string;
  base?: string;
}

/** Throws unless the page and server-function routers sit where the guard expects them, and the
 * server mounted the handler at exactly `/_server`. `mounts` are the server app's handler routes. */
export function checkServerFunctionMount(
  routers: readonly RouterConfig[] | undefined,
  mounts: readonly string[] | undefined,
): void {
  if (!routers || !mounts) throw new Error("the app's router configuration is unavailable at startup");
  const base = (name: string) => routers.find((router) => router.name === name)?.base;
  if (base("ssr") !== "/" || base("server-fns") !== "/_server") {
    throw new Error(
      "the app must be served from / with server functions at /_server; the server-function " +
        `guard depends on it (pages at ${base("ssr")}, server functions at ${base("server-fns")})`,
    );
  }
  const serverFunctionMounts = mounts.filter((route) => route.endsWith("/_server"));
  if (serverFunctionMounts.length === 0 || serverFunctionMounts.some((route) => route !== "/_server")) {
    throw new Error(
      "server functions must be mounted at /_server, not under a base URL (NITRO_APP_BASE_URL); " +
        `the server-function guard depends on it (mounted at ${serverFunctionMounts.join(", ") || "nowhere"})`,
    );
  }
}
