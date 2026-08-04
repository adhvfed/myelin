# Launch surface

Everything between "the agent runs" and "a stranger signs up and uses it." Mostly
product work + founder decisions.

- **Self-serve signup / OIDC-SSO** — no registration path exists.
- **Payments** — Stripe top-ups to fund the wallet (metering is decoupled from
  payment, so this plugs in without touching the ledger). Account for the provider
  cut; prepaid credits amortize the fixed fee. EU-sovereign provider is a later swap.
- **UI** — the run view (live transcript + live cost + HITL approval cards), the
  wallet/billing view, and launch points ("assign this to an agent"). Stack decided:
  SolidJS + Tauri 2.
- **Multi-tenant deploy** — the agent host runs on one cell today; real fleet hosting
  + abuse/cost controls (crypto-miner/spam defense, spend caps on a viral spike).
- **Independent pentest** — launch-critical before untrusted strangers, given the
  arbitrary-exec trust surface of compute tools.
