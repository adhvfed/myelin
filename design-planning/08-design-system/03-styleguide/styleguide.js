/* Myelin live styleguide — vanilla JS, no framework, no network.
   It reads the RESOLVED values of the real tokens (getComputedStyle on :root) and
   - renders the token galleries from those values, and
   - MEASURES contrast (WCAG 2.1 relative-luminance) live, so the labels can't drift from the tokens
     and re-compute on every theme switch. (00-plan §1.3 measured-not-claimed gate, made visible.) */
(function () {
  "use strict";
  var root = document.documentElement;
  var live = document.getElementById("live");

  /* ---------- WCAG contrast math (PROVEN formula — WCAG 2.1 1.4.3) ---------- */
  function parseColor(str) {
    // resolve any CSS color string via canvas → rgba
    var c = document.createElement("canvas").getContext("2d");
    c.fillStyle = "#000"; c.fillStyle = str; // canvas normalises named/hex/rgb
    var v = c.fillStyle;
    if (v[0] === "#") {
      var h = v.slice(1);
      if (h.length === 3) h = h[0]+h[0]+h[1]+h[1]+h[2]+h[2];
      return [parseInt(h.slice(0,2),16), parseInt(h.slice(2,4),16), parseInt(h.slice(4,6),16)];
    }
    var m = v.match(/rgba?\(([^)]+)\)/);
    if (!m) return [0,0,0];
    var p = m[1].split(",").map(function (x){return parseFloat(x);});
    return [p[0], p[1], p[2]];
  }
  function lum(rgb) {
    var a = rgb.map(function (v) {
      v /= 255;
      return v <= 0.03928 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4);
    });
    return 0.2126 * a[0] + 0.7152 * a[1] + 0.0722 * a[2];
  }
  function ratio(fg, bg) {
    var l1 = lum(parseColor(fg)), l2 = lum(parseColor(bg));
    var hi = Math.max(l1, l2), lo = Math.min(l1, l2);
    return (hi + 0.05) / (lo + 0.05);
  }
  function v(name) { return getComputedStyle(root).getPropertyValue(name).trim(); }

  /* verdict against the WCAG floor for the swatch's role:
       'text' → normal-text floor 4.5:1 (AAA at 7:1)
       'ui'   → UI-component/graphical-object floor 3:1 (1.4.11); ≥3 passes "AA (UI)" */
  function verdict(r, kind) {
    if (kind === "ui") return r >= 3 ? ["AA (UI ≥3:1)","ratio-aaa"] : ["FAIL","ratio-fail"];
    return r >= 7 ? ["AAA","ratio-aaa"] : r >= 4.5 ? ["AA","ratio-aa"] : ["FAIL","ratio-fail"];
  }

  /* ---------- swatch rendering ---------- */
  // spec: [token, paired-surface-token-for-contrast, kind, onTextToken|null]
  function swatch(spec) {
    var token = spec[0], bgRef = spec[1], kind = spec[2], onText = spec[3], note = spec[4];
    var val = v(token);
    var el = document.createElement("div");
    el.className = "swatch";
    var ratioHtml = "";
    if (kind !== "none") {
      var pairBg = v(bgRef);
      var r = ratio(val, pairBg);
      var vd = verdict(r, kind);
      ratioHtml =
        '<span class="swatch-ratio">' +
        '<span class="ratio-badge ' + vd[1] + '">' + r.toFixed(2) + ':1 ' + vd[0] + "</span>" +
        '<span class="sg-hint">vs ' + bgRef + "</span></span>";
    } else {
      ratioHtml = '<span class="swatch-ratio ratio-na">' + (note || "decorative") + "</span>";
    }
    // chip: if an on-text token is provided, sample it over the fill (proves on-* legibility)
    var chipInner = onText
      ? '<span class="on-sample" style="color:var(' + onText + ')">Aa</span>'
      : "";
    el.innerHTML =
      '<div class="swatch-chip" style="background:var(' + token + ')">' + chipInner + "</div>" +
      '<div class="swatch-meta">' +
      '<span class="swatch-name">' + token + "</span>" +
      '<span class="swatch-val">' + (val || "—") + "</span>" +
      ratioHtml +
      "</div>";
    return el;
  }
  function fill(id, specs) {
    var g = document.getElementById(id);
    g.innerHTML = "";
    specs.forEach(function (s) { g.appendChild(swatch(s)); });
  }

  function renderSwatches() {
    fill("grid-surface", [
      ["--surface", "--text-primary", "none", null, "page base"],
      ["--surface-raised", "--text-primary", "none", null, "raised"],
      ["--surface-overlay", "--text-primary", "none", null, "overlay"],
      ["--surface-hover", "--text-primary", "none", null, "hover"],
    ]);
    fill("grid-text", [
      ["--text-primary", "--surface", "text"],
      ["--text-muted", "--surface", "text"],
      ["--text-subtle", "--surface", "text"],
    ]);
    fill("grid-border", [
      ["--border", "--surface", "none", null, "hairline (exempt)"],
      ["--border-strong", "--surface", "none", null, "strong (exempt)"],
    ]);
    fill("grid-accent", [
      ["--accent", "--surface", "ui", null],         // identity, UI floor
      ["--focus-ring", "--surface", "ui", null],     // derived, the focus/primary token
      ["--accent-weak", "--surface", "none", null, "tint"],
    ]);
    fill("grid-status", [
      ["--success", "--surface", "ui", "--on-success"],
      ["--warning", "--surface", "ui", "--on-warning"],
      ["--danger", "--surface", "ui", "--on-danger"],
      ["--info", "--surface", "ui", "--on-info"],
    ]);
    fill("grid-agent", [
      ["--agent", "--surface", "ui", "--on-agent"],
      ["--agent-subtle", "--surface", "none", null, "tint"],
    ]);
  }

  /* ---------- type scale ---------- */
  function renderType() {
    var rows = [
      ["--fs-display", "Display 30", "--lh-tight", "--weight-semibold"],
      ["--fs-h1", "Heading 1 · 24", "--lh-tight", "--weight-semibold"],
      ["--fs-h2", "Heading 2 · 20", "--lh-tight", "--weight-semibold"],
      ["--fs-h3", "Heading 3 · 16", "--lh-body", "--weight-medium"],
      ["--fs-body", "Body · 14 — the workhorse size", "--lh-body", "--weight-regular"],
      ["--fs-body-sm", "Body small · 13", "--lh-body", "--weight-regular"],
      ["--fs-caption", "Caption · 12", "--lh-body", "--weight-regular"],
    ];
    var c = document.getElementById("type-list");
    c.innerHTML = "";
    rows.forEach(function (r) {
      var row = document.createElement("div");
      row.className = "type-row";
      row.innerHTML =
        '<span class="tk">' + r[0] + "</span>" +
        '<span class="type-sample" style="font-size:var(' + r[0] + ');line-height:var(' + r[2] +
        ");font-weight:var(" + r[3] + ')">' + r[1] + "</span>";
      c.appendChild(row);
    });
  }

  /* ---------- spacing + radius ---------- */
  function renderSpacing() {
    var spaces = ["--space-1","--space-2","--space-3","--space-4","--space-5","--space-6","--space-7","--space-8"];
    var c = document.getElementById("space-list");
    c.innerHTML = "";
    spaces.forEach(function (s) {
      var row = document.createElement("div");
      row.className = "scale-row";
      row.innerHTML =
        '<span class="tk">' + s + " · " + v(s) + "</span>" +
        '<span class="scale-bar" style="width:var(' + s + ')"></span>';
      c.appendChild(row);
    });
    var radii = ["--radius-0","--radius-1","--radius-2","--radius-3","--radius-pill"];
    var rc = document.getElementById("radius-list");
    rc.innerHTML = "";
    radii.forEach(function (s) {
      var d = document.createElement("div");
      d.className = "radius-swatch";
      d.innerHTML =
        '<span class="radius-box" style="border-radius:var(' + s + ')"></span>' +
        '<span class="tk">' + s + " · " + v(s) + "</span>";
      rc.appendChild(d);
    });
  }

  /* ---------- motion ---------- */
  function renderMotion() {
    var durs = ["--dur-instant","--dur-micro","--dur-fast","--dur-base","--dur-deliberate"];
    var c = document.getElementById("motion-grid");
    c.innerHTML = "";
    durs.forEach(function (s) {
      var cell = document.createElement("div");
      cell.className = "motion-cell";
      var dur = v(s) === "0ms" ? "1ms" : v(s); // instant → keep a token-driven loop visible
      cell.innerHTML =
        '<span class="tk">' + s + " · " + v(s) + "</span>" +
        '<div class="motion-track"><span class="motion-dot" style="animation-duration:' + dur + '"></span></div>';
      c.appendChild(cell);
    });
  }

  /* ---------- status set (glyph + label + colour — not colour alone) ---------- */
  var check = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><path d="M4 12l5 5L20 6"/></svg>';
  var warn = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><path d="M12 3l9 16H3z"/><path d="M12 10v4M12 17h.01"/></svg>';
  var fail = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><circle cx="12" cy="12" r="9"/><path d="M15 9l-6 6M9 9l6 6"/></svg>';
  var info = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><circle cx="12" cy="12" r="9"/><path d="M12 11v5M12 8h.01"/></svg>';
  function renderStatus() {
    var set = [
      ["--success", check, "Passed"],
      ["--warning", warn, "At risk"],
      ["--danger", fail, "Failed"],
      ["--info", info, "Info"],
    ];
    var c = document.getElementById("status-set");
    c.innerHTML = "";
    set.forEach(function (s) {
      var el = document.createElement("span");
      el.className = "status-pill";
      el.innerHTML = '<span style="color:var(' + s[0] + ')">' + s[1] + "</span><span>" + s[2] + "</span>";
      c.appendChild(el);
    });
  }

  function renderAll() {
    renderSwatches();
    renderType();
    renderSpacing();
    renderMotion();
    renderStatus();
  }

  /* ---------- theme switcher ---------- */
  var themeBtns = Array.prototype.slice.call(document.querySelectorAll("[data-theme-set]"));
  themeBtns.forEach(function (btn) {
    btn.addEventListener("click", function () { setTheme(btn.getAttribute("data-theme-set")); });
  });
  function setTheme(name) {
    root.setAttribute("data-theme", name);
    themeBtns.forEach(function (b) {
      b.setAttribute("aria-checked", String(b.getAttribute("data-theme-set") === name));
    });
    // re-render so the MEASURED ratios reflect the new theme's resolved tokens
    renderSwatches();
    live.textContent = "Theme: " + name;
  }

  /* ---------- RTL toggle ---------- */
  var dirBtn = document.getElementById("dirbtn");
  var dirVal = document.getElementById("dirval");
  dirBtn.addEventListener("click", function () {
    var rtl = root.getAttribute("dir") !== "rtl";
    root.setAttribute("dir", rtl ? "rtl" : "ltr");
    dirBtn.setAttribute("aria-pressed", String(rtl));
    dirVal.textContent = rtl ? "RTL" : "LTR";
    live.textContent = "Direction: " + (rtl ? "right to left" : "left to right");
  });

  /* ---------- icon gallery: ink + size controls (proves currentColor recolor) ---------- */
  var icoGrid = document.getElementById("ico-grid");
  if (icoGrid) {
    var inkBtns = Array.prototype.slice.call(document.querySelectorAll("[data-ico-ink]"));
    inkBtns.forEach(function (btn) {
      btn.addEventListener("click", function () {
        var tok = btn.getAttribute("data-ico-ink");
        // recolor the whole grid via one CSS color → every inline SVG follows (currentColor)
        icoGrid.style.color = "var(--" + tok + ")";
        inkBtns.forEach(function (b) {
          b.setAttribute("aria-checked", String(b === btn));
        });
        live.textContent = "Icon ink: " + tok;
      });
    });
    var icoSize = document.getElementById("ico-size");
    var icoSv = document.getElementById("ico-sv");
    if (icoSize) {
      icoSize.addEventListener("input", function () {
        icoGrid.style.setProperty("--ico-size", icoSize.value + "px");
        if (icoSv) icoSv.textContent = icoSize.value + "px";
      });
    }
  }

  renderAll();
})();
