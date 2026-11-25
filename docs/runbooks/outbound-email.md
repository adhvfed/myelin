# Send email from the edge

The edge emails three things: a member's invitation to a teammate, a password reset link someone
asked for on the sign-in page, and a notice after a password is changed. Without mail, members
get an invitation link to pass on themselves, and the sign-in page tells people to ask whoever
runs the deployment for a reset link. The operator's `edge invite` and `edge reset-link` always
print the link and never send mail.

## How it works

A request never talks to SMTP. It writes a sealed letter to the `edge_mail_outbox` table in the
same transaction that creates the link, and a worker in each edge sends queued mail. Letters are
sealed under a key derived from `MYELIN_KMS_SEAL_KEY`, so the table shows neither addresses nor
links. Several edges can share the outbox safely. A letter whose link has been used, replaced or
has expired is withdrawn or dropped rather than sent.

A person can hold several reset links at once: one asked for on the sign-in page never replaces
another. An operator's `edge reset-link` replaces only the operator's own earlier reset links for
that person, so reissuing revokes a link sent the wrong way. Setting a new password ends all of
the person's reset links.

A failed send is retried after 30 seconds, then after twice as long each time, up to an hour
between tries. A letter still failing a day after it was queued is dropped. Every failure is
logged as `edge.mail.deferred` or `edge.mail.undeliverable` with the message id, the SMTP reply
code, and nothing about the content. Delivery is at least once: a crash between sending and
recording the send means the message goes out again, with the same Message-ID.

## Settings

| Variable | Meaning |
|---|---|
| `MYELIN_SMTP_URL` | The relay. Unset means mail is off. |
| `MYELIN_SMTP_USERNAME` | SMTP username. Set it together with the password, or neither. |
| `MYELIN_SMTP_PASSWORD` | SMTP password. Kept out of the URL so it never appears in logs. |
| `MYELIN_MAIL_FROM` | The sender, e.g. `Myelin <no-reply@example.com>`. Required with the URL. |

`MYELIN_SMTP_URL` takes one of three forms:

- `smtp://host:port?tls=required`: STARTTLS, and nothing is sent unless it succeeds.
- `smtps://host:port`: TLS from the first byte (implicit TLS).
- `smtp://host:port`: no TLS. Allowed only when the host is `localhost` or a loopback address.

The edge refuses to start on anything else, including credentials written into the URL, and on a
mode the port does not speak: STARTTLS on 465 or 2465, or implicit TLS on 25, 587 or 2587. Without
a port, the default is 587 for STARTTLS, 465 for implicit TLS, and 25 for plain loopback SMTP. A
send that takes longer than a minute in all, at any stage, counts as a failure and is retried.
That minute includes the closing QUIT, so a relay that accepts a message and then stalls on QUIT
gets it again.
Links in mail point at `MYELIN_WEB_PUBLIC_URL`, which must be set.

## Scaleway Transactional Email

1. In the Scaleway console, open Transactional Email, add the domain you will send from, and
   publish the SPF and DKIM records it lists (and any others it asks for) in that domain's DNS.
   Wait until Scaleway shows the domain as verified; until then it refuses mail from that domain.
   A DMARC record for the domain also helps receivers trust the mail.
2. Create an API key whose IAM policy allows Transactional Email for the project. Note the
   project ID and the key's secret.
3. Set these on every edge:

   ```sh
   MYELIN_SMTP_URL='smtp://smtp.tem.scw.cloud:587?tls=required'   # or :2587 where 587 is blocked
   MYELIN_SMTP_USERNAME='<Scaleway project ID>'
   MYELIN_SMTP_PASSWORD='<API key secret>'
   MYELIN_MAIL_FROM='Myelin <no-reply@your-verified-domain>'
   ```

   For implicit TLS instead, use `smtps://smtp.tem.scw.cloud:465` (or `:2465`).

4. Restart the edge. Its log says `edge: mail delivery on (…)` with the host, port, and sender, and
   `/v1/auth/config` reports `"mail_delivery_enabled": true`.
5. Use "Forgot password?" on the sign-in page with your own address, and check that the message
   arrives and that its link opens the page for a new password.

## Development

`fed start` runs Mailpit and points the edge at it. Every message the edge sends lands in the
inbox whose URL `fed` prints at startup (`Mail inbox: …`); nothing leaves the machine.
