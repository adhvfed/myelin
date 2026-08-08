import { MetaProvider, Title } from "@solidjs/meta";
import { Router, useLocation } from "@solidjs/router";
import { FileRoutes } from "@solidjs/start/router";
import { createEffect, onMount, Suspense, type ParentProps } from "solid-js";
import { isServer } from "solid-js/web";
import { ToastProvider } from "@myelin/design-system";
import "./app.css";
import { restoreTheme } from "./lib/theme";

function AppRoot(props: ParentProps) {
  const location = useLocation();
  onMount(() => restoreTheme());

  if (!isServer) {
    createEffect(() => {
      const currentUrl = location.pathname + location.search;
      queueMicrotask(() => {
        if (currentUrl !== location.pathname + location.search) return;
        // Hydration can leave the server-rendered title ahead of the active route title.
        const titles = document.head.querySelectorAll(":scope > title");
        for (let index = 0; index < titles.length - 1; index += 1) {
          titles[index]?.remove();
        }
      });
    });
  }

  return (
    <MetaProvider>
      <Title>Myelin</Title>
      <ToastProvider>
        <Suspense>{props.children}</Suspense>
      </ToastProvider>
    </MetaProvider>
  );
}

export default function App() {
  return (
    <Router root={AppRoot}>
      <FileRoutes />
    </Router>
  );
}
