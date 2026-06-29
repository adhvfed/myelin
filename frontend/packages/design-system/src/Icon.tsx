// <Icon> — the Solid wrapper over the self-hosted strok sprite (no CDN; sovereignty/GDPR, doc 08).
//
// Renders <svg><use href="<sprite>#<name>"/></svg>. The glyph inherits `currentColor` (the sprite
// strokes/fills are authored as currentColor), so an icon is colored by its text context via the
// semantic tokens — never a hardcoded hex here.
//
// A11y model (binding, WAI-ARIA APG):
//   - DECORATIVE (default): aria-hidden, no accessible name — the adjacent text carries meaning.
//   - MEANINGFUL: pass `title`; the icon gets role="img" + aria-label so AT announces it.
// This is the single decision point so no subsystem re-invents icon a11y.

import { splitProps, mergeProps, type JSX } from "solid-js";
import type { IconName } from "./icon-names";

// Where the sprite is served from. Overridable per app shell (Tauri vs web base path) without
// editing call sites. Default points at the package's generated asset.
export const SPRITE_HREF_DEFAULT = new URL(
  "../generated/assets/sprite.svg",
  import.meta.url,
).href;

export interface IconProps extends JSX.SvgSVGAttributes<SVGSVGElement> {
  /** Sprite symbol id (typed against the 42-icon set). */
  name: IconName;
  /** Edge length in px (square). Default 16 — the compact "Instrument" default. */
  size?: number;
  /** Accessible name. Omit for decorative icons (the default; aria-hidden). */
  title?: string;
  /** Override the sprite URL (e.g. a Tauri asset:// base). */
  spriteHref?: string;
}

export function Icon(props: IconProps): JSX.Element {
  // mergeProps keeps defaults reactive; never destructure props in Solid (reactivity foot-gun).
  const merged = mergeProps({ size: 16, spriteHref: SPRITE_HREF_DEFAULT }, props);
  const [local, rest] = splitProps(merged, ["name", "size", "title", "spriteHref"]);
  const labelled = () => local.title !== undefined;
  return (
    <svg
      width={local.size}
      height={local.size}
      viewBox="0 0 24 24"
      fill="none"
      aria-hidden={labelled() ? undefined : "true"}
      role={labelled() ? "img" : undefined}
      aria-label={labelled() ? local.title : undefined}
      {...rest}
    >
      {labelled() ? <title>{local.title}</title> : null}
      <use href={`${local.spriteHref}#${local.name}`} />
    </svg>
  );
}
