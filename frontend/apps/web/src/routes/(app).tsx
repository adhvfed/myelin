// The authenticated app LAYOUT (pathless `(app)` group): the auth guard + the shell chrome wrapping
// every app route. Viewer verification redirects a missing or invalid session to `/login`, so no app
// route renders without a verified viewer. Transient verification failures render a stable recovery
// page rather than destroying the user's location. `/login` lives OUTSIDE this group.
import { ErrorBoundary, Show, Suspense, type JSX } from "solid-js";
import { createAsync, useLocation } from "@solidjs/router";
import { AppShell } from "../components/AppShell";
import { AppUnavailable } from "../components/AppUnavailable";
import { requireViewer } from "../lib/auth";

export default function AppLayout(props: { children?: JSX.Element }) {
  const location = useLocation();
  const retryHref = () => `${location.pathname}${location.search}${location.hash}`;
  const viewer = createAsync(() => requireViewer(), { deferStream: true });

  return (
    <ErrorBoundary fallback={() => <AppUnavailable retryHref={retryHref()} />}>
      <Suspense>
        <Show
          when={viewer()}
          fallback={<AppUnavailable retryHref={retryHref()} />}
        >
          {(current) => <AppShell viewer={current()}>{props.children}</AppShell>}
        </Show>
      </Suspense>
    </ErrorBoundary>
  );
}
