// @refresh reload
import { mount, StartClient } from "@solidjs/start/client";

const root = document.getElementById("app");
if (!root) throw new Error("SolidStart hydration root #app is missing");

// Vinxi 0.5.11's virtual client handler re-exports this module's default value. Exporting mount's
// disposer satisfies that generated contract and keeps a missing client entry export from being
// downgraded to a Rollup warning during an otherwise-successful production build.
export default mount(() => <StartClient />, root);
