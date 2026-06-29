// Copy the self-hosted icon sprite into the package's generated/ output (no CDN — sovereignty/GDPR).
// ANTI-DUPLICATION: the SOURCE OF TRUTH stays at design-planning/08-design-system/04-icons/dist;
// we copy it as a build artifact (gitignored) so the <Icon> wrapper has a runtime asset to point at.
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import { copyFileSync, mkdirSync } from "node:fs";

const __dirname = dirname(fileURLToPath(import.meta.url));
const ICONS = resolve(__dirname, "../../../../design-planning/08-design-system/04-icons/dist");
const OUT = resolve(__dirname, "../generated/assets");

mkdirSync(OUT, { recursive: true });
for (const f of ["sprite.svg", "manifest.json"]) {
  copyFileSync(resolve(ICONS, f), resolve(OUT, f));
}
console.log("assets: copied sprite.svg + manifest.json ->", OUT);
