// Solid wrapper for the self-hosted SVG sprite. Icons inherit `currentColor` and are decorative by
// default; passing `title` gives the icon an accessible name.

import { splitProps, mergeProps, type JSX } from "solid-js";
import type { IconName } from "./icon-names";
import spriteHref from "../generated/assets/sprite.svg?url";

// Where the sprite is served from. Overridable per app shell (Tauri vs web base path) without
// editing call sites. Default points at the package's generated asset.
export const SPRITE_HREF_DEFAULT = spriteHref;

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
