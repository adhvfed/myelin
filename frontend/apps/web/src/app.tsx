import { MetaProvider, Title } from "@solidjs/meta";
import { Router } from "@solidjs/router";
import { FileRoutes } from "@solidjs/start/router";
import { Suspense } from "solid-js";
import { ToastProvider } from "@myelin/design-system";
import "./app.css";

// The app root: MetaProvider (titles), the file-based Router, and the ONE ToastProvider (MR-017) the
// whole shell shares — toasts host undo + announce via a live region, never steal focus. Every route
// renders inside <Suspense> so loaders/`createAsync` can stream.
export default function App() {
  return (
    <Router
      root={(props) => (
        <MetaProvider>
          <Title>Myelin</Title>
          <ToastProvider>
            <Suspense>{props.children}</Suspense>
          </ToastProvider>
        </MetaProvider>
      )}
    >
      <FileRoutes />
    </Router>
  );
}
