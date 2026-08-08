// Copy package-owned icons into the generated runtime output.
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import { copyFileSync, mkdirSync } from "node:fs";

const __dirname = dirname(fileURLToPath(import.meta.url));
const ICONS = resolve(__dirname, "../assets");
const OUT = resolve(__dirname, "../generated/assets");

mkdirSync(OUT, { recursive: true });
for (const f of ["sprite.svg", "manifest.json"]) {
  copyFileSync(resolve(ICONS, f), resolve(OUT, f));
}
console.log("assets: copied sprite.svg + manifest.json ->", OUT);
