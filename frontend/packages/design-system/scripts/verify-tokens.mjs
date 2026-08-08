// Keep generated tokens aligned with the package's reviewed CSS reference.
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import { readFileSync } from "node:fs";

const __dirname = dirname(fileURLToPath(import.meta.url));
const GENERATED = resolve(__dirname, "../generated/tokens.css");
const CANONICAL = resolve(__dirname, "../tokens/tokens.css");

// Extract a { "selector" -> { varName -> value } } map from a CSS file (flat, top-level blocks).
function parseBlocks(rawCss) {
  // Strip CSS comments first — canonical tokens.css annotates declarations with inline /* ratios */.
  const css = rawCss.replace(/\/\*[\s\S]*?\*\//g, "");
  const blocks = {};
  const re = /([^{}]+)\{([^{}]*)\}/g;
  let m;
  while ((m = re.exec(css))) {
    const selector = m[1].trim().replace(/\s+/g, " ");
    const body = m[2];
    const vars = {};
    for (const decl of body.split(";")) {
      const i = decl.indexOf(":");
      if (i === -1) continue;
      const name = decl.slice(0, i).trim();
      if (!name.startsWith("--")) continue;
      vars[name] = decl.slice(i + 1).trim();
    }
    if (Object.keys(vars).length) blocks[selector] = { ...(blocks[selector] || {}), ...vars };
  }
  return blocks;
}

// Find the block whose selector mentions the theme, merged across :root variants.
function themeVars(blocks, theme) {
  const out = {};
  for (const [sel, vars] of Object.entries(blocks)) {
    const isDark = theme === "dark" && (sel === ":root" || /data-theme="dark"/.test(sel));
    const isOther = theme !== "dark" && new RegExp(`data-theme="${theme}"`).test(sel);
    if (isDark || isOther) Object.assign(out, vars);
  }
  return out;
}

const gen = parseBlocks(readFileSync(GENERATED, "utf8"));
const can = parseBlocks(readFileSync(CANONICAL, "utf8"));

const SEMANTIC = [
  "--surface", "--surface-raised", "--surface-overlay", "--surface-hover",
  "--text-primary", "--text-muted", "--text-subtle",
  "--border", "--border-strong",
  "--accent", "--on-accent", "--focus-ring",
  "--success", "--on-success", "--success-subtle",
  "--warning", "--on-warning", "--warning-subtle",
  "--danger", "--on-danger", "--danger-subtle",
  "--info", "--on-info", "--info-subtle",
  "--agent", "--on-agent", "--agent-subtle",
];
const ZSCALE = ["--z-base", "--z-chrome", "--z-popover", "--z-modal", "--z-toast"];

const mismatches = [];
const norm = (v) => (v || "").toLowerCase().replace(/\s+/g, "");

for (const theme of ["dark", "light", "high-contrast"]) {
  const g = themeVars(gen, theme);
  const c = themeVars(can, theme);
  for (const name of SEMANTIC) {
    if (norm(g[name]) !== norm(c[name])) {
      mismatches.push(`[${theme}] ${name}: generated="${g[name]}" canonical="${c[name]}"`);
    }
  }
}
// z-index is theme-independent (:root primitives).
const gz = themeVars(gen, "dark");
const cz = themeVars(can, "dark");
for (const name of ZSCALE) {
  if (norm(gz[name]) !== norm(cz[name])) {
    mismatches.push(`[z-index] ${name}: generated="${gz[name]}" canonical="${cz[name]}"`);
  }
}

if (mismatches.length) {
  console.error("generated tokens.css differs from the reviewed token reference:");
  for (const x of mismatches) console.error("  " + x);
  process.exit(1);
}
console.log(
  `tokens verify: ${SEMANTIC.length} semantic vars x 3 themes and ${ZSCALE.length} z-index vars match.`,
);
