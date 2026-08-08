// The authenticated app LAYOUT (pathless `(app)` group): the auth guard + the shell chrome wrapping
// every app route. `requireViewer` throws a `/login` redirect server-side when there is no session, so
// no app route ever renders without a verified viewer. `/login` lives OUTSIDE this group.
import { Show, Suspense, type JSX } from "solid-js";
import { createAsync } from "@solidjs/router";
import { AppShell } from "../components/AppShell";
import { requireViewer } from "../lib/auth";

export default function AppLayout(props: { children?: JSX.Element }) {
  const viewer = createAsync(() => requireViewer(), { deferStream: true });
  return (
    <Suspense>
      <Show when={viewer()}>
        {(v) => <AppShell viewer={v()}>{props.children}</AppShell>}
      </Show>
    </Suspense>
  );
}
