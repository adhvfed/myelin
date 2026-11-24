// The frame shared by the logged-out pages: the sheath on one side, a quiet form on the other.
import { Show, Suspense, type JSX } from "solid-js";
import { createAsync } from "@solidjs/router";
import { getDeploymentRegion } from "../../lib/auth";
import { Sheath, type SheathCut } from "./Sheath";

export function AuthShell(props: {
  cut: SheathCut;
  fill?: (ring: number) => number;
  guides?: boolean;
  busy?: boolean;
  children: JSX.Element;
}) {
  const region = createAsync(() => getDeploymentRegion());
  return (
    <main class="auth-main">
      <div class="auth-art">
        <Sheath cut={props.cut} fill={props.fill} guides={props.guides} busy={props.busy} />
        <p class="auth-wordmark">Myelin</p>
        <Suspense>
          <Show when={region()}>
            <p class="auth-region">
              Data region: <strong>{region()}</strong>
            </p>
          </Show>
        </Suspense>
      </div>
      <div class="auth-pane">{props.children}</div>
    </main>
  );
}
