// A sanitized-by-construction Markdown renderer (R3.4 — the NAMED FLOOR). The gate calls for the
// BlockEditor read-path if it renders markdown today; it does NOT exist on this surface yet, so this
// is the explicit floor: a small block/inline parser that emits Solid ELEMENTS (text nodes + known
// tags) and NEVER `innerHTML` / raw-HTML injection — so untrusted README bytes cannot inject markup
// or script by construction. Covers headings, fenced code, lists, blockquotes, paragraphs, and inline
// code / bold / italic / links (http(s)/relative only). Richer markdown (tables, images) degrades to
// text — an honest floor, replaced when the editor read-path lands on this surface. Semantic tokens.
import { For, type JSX } from "solid-js";

export function Markdown(props: { source: string; headingOffset?: number }): JSX.Element {
  // README headings are DEMOTED (default +1) so a README `#` never competes with the page's own h1 —
  // the README is a subsection of the surface, not its title.
  const offset = () => props.headingOffset ?? 1;
  return (
    <div data-testid="readme-render" style={{ display: "flex", "flex-direction": "column", gap: "var(--space-2)" }}>
      <For each={parseBlocks(props.source ?? "")}>{(block) => renderBlock(block, offset())}</For>
    </div>
  );
}

type Block =
  | { t: "h"; level: number; text: string }
  | { t: "code"; text: string }
  | { t: "ul"; items: string[] }
  | { t: "ol"; items: string[] }
  | { t: "quote"; text: string }
  | { t: "p"; text: string };

function parseBlocks(src: string): Block[] {
  const lines = src.replace(/\r\n?/g, "\n").split("\n");
  const blocks: Block[] = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i]!;
    // Fenced code block.
    if (line.trimStart().startsWith("```")) {
      const body: string[] = [];
      i++;
      while (i < lines.length && !lines[i]!.trimStart().startsWith("```")) {
        body.push(lines[i]!);
        i++;
      }
      i++; // skip the closing fence
      blocks.push({ t: "code", text: body.join("\n") });
      continue;
    }
    // Heading.
    const h = /^(#{1,6})\s+(.*)$/.exec(line);
    if (h) {
      blocks.push({ t: "h", level: h[1]!.length, text: h[2]!.trim() });
      i++;
      continue;
    }
    // Unordered list.
    if (/^\s*[-*+]\s+/.test(line)) {
      const items: string[] = [];
      while (i < lines.length && /^\s*[-*+]\s+/.test(lines[i]!)) {
        items.push(lines[i]!.replace(/^\s*[-*+]\s+/, ""));
        i++;
      }
      blocks.push({ t: "ul", items });
      continue;
    }
    // Ordered list.
    if (/^\s*\d+\.\s+/.test(line)) {
      const items: string[] = [];
      while (i < lines.length && /^\s*\d+\.\s+/.test(lines[i]!)) {
        items.push(lines[i]!.replace(/^\s*\d+\.\s+/, ""));
        i++;
      }
      blocks.push({ t: "ol", items });
      continue;
    }
    // Blockquote.
    if (/^\s*>\s?/.test(line)) {
      const body: string[] = [];
      while (i < lines.length && /^\s*>\s?/.test(lines[i]!)) {
        body.push(lines[i]!.replace(/^\s*>\s?/, ""));
        i++;
      }
      blocks.push({ t: "quote", text: body.join(" ") });
      continue;
    }
    // Blank line.
    if (line.trim() === "") {
      i++;
      continue;
    }
    // Paragraph (gather until a blank line or a block starter).
    const para: string[] = [];
    while (
      i < lines.length &&
      lines[i]!.trim() !== "" &&
      !/^(#{1,6}\s|```|\s*[-*+]\s|\s*\d+\.\s|\s*>\s?)/.test(lines[i]!)
    ) {
      para.push(lines[i]!);
      i++;
    }
    blocks.push({ t: "p", text: para.join(" ") });
  }
  return blocks;
}

const codeStyle = {
  border: "var(--hairline) solid var(--border)",
  "border-radius": "var(--radius-1)",
  padding: "var(--space-3)",
  background: "var(--surface)",
  margin: "0",
  "font-family": "var(--font-mono)",
  "white-space": "pre-wrap",
  overflow: "auto",
} as const;

function renderBlock(b: Block, headingOffset = 1): JSX.Element {
  switch (b.t) {
    case "h": {
      const level = Math.min(6, b.level + headingOffset);
      const size = level <= 2 ? "var(--fs-h2)" : "var(--fs-h3)";
      return (
        <div role="heading" aria-level={level} style={{ "font-size": size, "font-weight": "600", margin: "var(--space-2) 0 0" }}>
          {inline(b.text)}
        </div>
      );
    }
    case "code":
      return <pre style={codeStyle}>{b.text}</pre>;
    case "ul":
      return (
        <ul style={{ margin: "0", "padding-inline-start": "var(--space-4)" }}>
          <For each={b.items}>{(it) => <li>{inline(it)}</li>}</For>
        </ul>
      );
    case "ol":
      return (
        <ol style={{ margin: "0", "padding-inline-start": "var(--space-4)" }}>
          <For each={b.items}>{(it) => <li>{inline(it)}</li>}</For>
        </ol>
      );
    case "quote":
      return (
        <blockquote style={{ margin: "0", "border-inline-start": "var(--border-2, 3px) solid var(--border)", "padding-inline-start": "var(--space-3)", color: "var(--text-muted)" }}>
          {inline(b.text)}
        </blockquote>
      );
    case "p":
      return <p style={{ margin: "0", "line-height": "1.6" }}>{inline(b.text)}</p>;
  }
}

// Inline parsing → an array of text/element nodes (NEVER innerHTML). Order: inline code, links, bold,
// italic. A link URL is allowed only if http(s) or relative (never `javascript:` etc.).
function inline(text: string): JSX.Element {
  return <>{parseInline(text)}</>;
}

function parseInline(text: string): (string | JSX.Element)[] {
  // Inline code first (its content is literal — no further parsing inside).
  const out: (string | JSX.Element)[] = [];
  const codeSplit = text.split(/(`[^`]+`)/g);
  for (const chunk of codeSplit) {
    if (chunk.startsWith("`") && chunk.endsWith("`") && chunk.length > 1) {
      out.push(
        <code style={{ "font-family": "var(--font-mono)", background: "var(--surface)", padding: "0 var(--space-1)", "border-radius": "var(--radius-1)" }}>
          {chunk.slice(1, -1)}
        </code>,
      );
    } else {
      out.push(...parseLinksAndEmphasis(chunk));
    }
  }
  return out;
}

function safeHref(url: string): string | undefined {
  const u = url.trim();
  if (/^https?:\/\//i.test(u) || u.startsWith("/") || u.startsWith("#") || u.startsWith("./") || u.startsWith("../")) {
    return u;
  }
  return undefined; // drop javascript:, data:, etc. (never rendered as a link)
}

function parseLinksAndEmphasis(text: string): (string | JSX.Element)[] {
  const out: (string | JSX.Element)[] = [];
  const linkRe = /\[([^\]]+)\]\(([^)]+)\)/g;
  let last = 0;
  let m: RegExpExecArray | null;
  while ((m = linkRe.exec(text)) !== null) {
    if (m.index > last) out.push(...emphasis(text.slice(last, m.index)));
    const href = safeHref(m[2]!);
    if (href) {
      out.push(
        <a href={href} style={{ color: "var(--text-primary)", "text-decoration": "underline" }}>
          {m[1]}
        </a>,
      );
    } else {
      out.push(m[1]!); // an unsafe URL → render just the link text (no link)
    }
    last = m.index + m[0].length;
  }
  if (last < text.length) out.push(...emphasis(text.slice(last)));
  return out;
}

function emphasis(text: string): (string | JSX.Element)[] {
  const out: (string | JSX.Element)[] = [];
  const re = /(\*\*[^*]+\*\*|\*[^*]+\*|_[^_]+_)/g;
  let last = 0;
  let m: RegExpExecArray | null;
  while ((m = re.exec(text)) !== null) {
    if (m.index > last) out.push(text.slice(last, m.index));
    const tok = m[0];
    if (tok.startsWith("**")) out.push(<strong>{tok.slice(2, -2)}</strong>);
    else out.push(<em>{tok.slice(1, -1)}</em>);
    last = m.index + tok.length;
  }
  if (last < text.length) out.push(text.slice(last));
  return out;
}
