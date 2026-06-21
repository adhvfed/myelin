#!/usr/bin/env bash
# gen-index.sh — regenerate icons-index.html by INLINING the current svg/ files.
# Inlining (not <img>) is required so the icons inherit `currentColor` and the page
# can prove recolor. Run by build.sh after the SVGs are (re)generated so the contact
# sheet can never go stale relative to the shipped svg/.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SVG_DIR="$ROOT/svg"; OUT="$ROOT/icons-index.html"

{
cat <<'HEAD'
<!doctype html><html lang="en"><head><meta charset="utf-8">
<title>Myelin icons — live contact sheet</title>
<style>
  :root{ --ink:#1a1a1a; --bg:#ffffff; --cell:#f4f4f6; --size:32px; }
  *{box-sizing:border-box} body{margin:0;font:14px/1.4 ui-sans-serif,system-ui,sans-serif;background:var(--bg);color:var(--ink)}
  header{position:sticky;top:0;display:flex;gap:20px;align-items:center;flex-wrap:wrap;
    padding:14px 20px;background:var(--bg);border-bottom:1px solid color-mix(in srgb,var(--ink) 15%,transparent);z-index:2}
  header h1{font-size:14px;margin:0 12px 0 0;font-weight:600}
  label{display:flex;gap:6px;align-items:center;font-size:12px;opacity:.85}
  .grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(120px,1fr));gap:2px;padding:20px}
  figure{margin:0;display:flex;flex-direction:column;align-items:center;gap:8px;
    padding:18px 6px;background:var(--cell);border-radius:6px}
  figure svg{width:var(--size);height:var(--size);color:var(--ink);display:block}
  figcaption{font:11px ui-monospace,monospace;opacity:.7;text-align:center;word-break:break-all}
  body.dark{--bg:#0d1117;--ink:#e6e6e6;--cell:#161b22}
</style></head><body>
<header>
  <h1>Myelin icons — live (regenerated from <code>svg/</code>)</h1>
  <label>ink <input id=ink type=color value="#1a1a1a"></label>
  <label>size <input id=size type=range min=16 max=64 value=32> <span id=sv>32px</span></label>
  <label><input id=dark type=checkbox> dark bg</label>
  <label>count: <b id=count>0</b></label>
</header>
<div class="grid" id=grid>
HEAD

n=0
for f in "$SVG_DIR"/*.svg; do
  name="$(basename "$f" .svg)"
  # inline the svg: drop xmlns/width/height from the opening tag; keep viewBox + inner
  svg="$(sed -E 's/ xmlns="[^"]*"//; s/ width="[0-9.]*"//; s/ height="[0-9.]*"//' "$f" | tr -d '\n')"
  printf '  <figure>%s<figcaption>%s</figcaption></figure>\n' "$svg" "$name"
  n=$((n+1))
done

cat <<FOOT
</div>
<script>
  const r=document.documentElement.style;
  ink.oninput=e=>r.setProperty('--ink',e.target.value);
  size.oninput=e=>{r.setProperty('--size',e.target.value+'px');sv.textContent=e.target.value+'px';};
  dark.onchange=e=>{document.body.classList.toggle('dark',e.target.checked); if(e.target.checked){ink.value='#e6e6e6';r.setProperty('--ink','#e6e6e6');}else{ink.value='#1a1a1a';r.setProperty('--ink','#1a1a1a');}};
  count.textContent='$n';
</script>
</body></html>
FOOT
} > "$OUT"
echo "regenerated icons-index.html ($n icons inlined from svg/)"
