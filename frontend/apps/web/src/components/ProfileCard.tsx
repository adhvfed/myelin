import { Icon, type IconName } from "@myelin/design-system";
import type { JSX } from "solid-js";

/** A titled card on the account pages (profile, members). */
export function ProfileCard(props: { icon: IconName; title: string; copy: string; children?: JSX.Element }) {
  return <section class="profile-card"><header><Icon name={props.icon} /><div><h2>{props.title}</h2><p>{props.copy}</p></div></header><div class="profile-card-content">{props.children}</div></section>;
}
