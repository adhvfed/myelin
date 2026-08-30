// Copy package-owned icons into the generated runtime output.
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import { copyFileSync, mkdirSync, readFileSync } from "node:fs";

const __dirname = dirname(fileURLToPath(import.meta.url));
const ICONS = resolve(__dirname, "../assets");
const OUT = resolve(__dirname, "../generated/assets");

mkdirSync(OUT, { recursive: true });
for (const f of ["sprite.svg", "manifest.json"]) {
  const source = resolve(ICONS, f);
  const destination = resolve(OUT, f);
  let unchanged = false;
  try {
    unchanged = readFileSync(source).equals(readFileSync(destination));
  } catch (error) {
    if (error?.code !== "ENOENT") throw error;
  }
  if (!unchanged) copyFileSync(source, destination);
}
console.log("assets: copied sprite.svg + manifest.json ->", OUT);
