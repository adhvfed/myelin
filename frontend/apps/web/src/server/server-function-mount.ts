// The middleware's server-function guard (lib/csrf.ts) knows the handler is mounted at
// `/_server` under an app served from `/`. Check that at startup, so a changed base URL fails
// loudly instead of quietly moving the handler out from under the guard.

interface RouterConfig {
  name?: string;
  base?: string;
}

/** Throws unless the page and server-function routers sit where the guard expects them. */
export function checkServerFunctionMount(routers: readonly RouterConfig[] | undefined): void {
  if (!routers) throw new Error("the app's router configuration is unavailable at startup");
  const base = (name: string) => routers.find((router) => router.name === name)?.base;
  if (base("ssr") !== "/" || base("server-fns") !== "/_server") {
    throw new Error(
      "the app must be served from / with server functions at /_server; the server-function " +
        `guard depends on it (pages at ${base("ssr")}, server functions at ${base("server-fns")})`,
    );
  }
}
