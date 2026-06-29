import { render } from "solid-js/web";
// Tier-0 design tokens (the only styling vocabulary) — re-skins via [data-theme] on <html>.
import "@myelin/design-system/tokens.css";
import { App } from "./App";

const root = document.getElementById("root");
if (!root) throw new Error("MR-018 shell: missing #root mount node");

render(() => <App />, root);
