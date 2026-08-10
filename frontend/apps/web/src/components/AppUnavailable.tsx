import { Icon } from "@myelin/design-system";

export function AppUnavailable(props: { retryHref: string }) {
  return (
    <main class="app-unavailable" role="alert" data-testid="app-unavailable">
      <Icon name="check-fail" size={32} title="Unavailable" />
      <h1>Myelin is temporarily unavailable</h1>
      <p>
        We couldn&rsquo;t load your workspace. Your place is kept, and no action was inferred.
      </p>
      <a class="app-unavailable-retry" href={props.retryHref} rel="external">
        <Icon name="rerun" /> Try again
      </a>
    </main>
  );
}
