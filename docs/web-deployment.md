# Web deployment

The web product ships as a standalone, non-root OCI image. Build it from the repository root so the
locked frontend workspace and canonical design-system sources are available:

```sh
docker build --file frontend/apps/web/Dockerfile --tag myelin-web:local .
```

The Dockerfile pins the Node 22 base by digest, installs pnpm 10.5.2, uses the committed lockfile,
and copies only SolidStart's `.output` into the runtime stage. Dependabot tracks the base image, and
CI rebuilds and smokes the image on every change.

Create a root-readable deployment environment file outside the repository:

```dotenv
REDIS_URL=rediss://user:password@valkey.internal:6380/0
MYELIN_WEB_SESSION_KEY=<32-random-bytes-as-base64url>
MYELIN_PUBLIC_ORIGIN=https://myelin.example
MYELIN_EDGE_URL=https://edge.internal
MYELIN_OIDC_AUTHORIZATION_ENDPOINT=https://identity.example/oauth2/authorize
MYELIN_OIDC_TOKEN_ENDPOINT=https://identity.example/oauth2/token
MYELIN_OIDC_CLIENT_ID=myelin-web
MYELIN_OIDC_CLIENT_SECRET=<secret-manager-value>
MYELIN_OIDC_SCOPES="openid profile email"
MYELIN_HSTS=1
```

`REDIS_URL`, `MYELIN_WEB_SESSION_KEY`, `MYELIN_PUBLIC_ORIGIN`, and `MYELIN_EDGE_URL` are mandatory.
The edge URL must use HTTPS, including on an internal network, because the web server sends bearer
capabilities to this origin. Terminate TLS at the edge or at an authenticated private ingress in
front of it; production startup rejects a plain-HTTP edge URL.
Generate the session key once with `node -p "require('crypto').randomBytes(32).toString('base64url')"`
and inject the same secret into every web replica. It encrypts and authenticates the complete trusted
session record at rest with AES-256-GCM; changing it intentionally invalidates all existing browser
sessions. The public origin must be HTTPS; the Valkey connection must use TLS; and the edge value must
be a credential-free HTTPS origin on the private service network. Enable HSTS only after the public
hostname is permanently HTTPS.

The five `MYELIN_OIDC_*` settings are optional as a group; setting any of the four endpoint/client
fields requires all four. Production endpoints must use HTTPS and contain no credentials, query, or
fragment. Register exactly `${MYELIN_PUBLIC_ORIGIN}/auth/oidc/callback` at the provider and configure
the provider for `client_secret_basic`. `MYELIN_OIDC_SCOPES` defaults to `openid profile email` and
must include `openid`. The edge must independently have its matching issuer, audience, and HTTPS
JWKS refresh URI (plus optional bootstrap keys); the login page advertises SSO only when both halves
report ready.

The web server uses Authorization Code with S256 PKCE, a per-login nonce, and a ten-minute one-time
state transaction. Those transactions are encrypted in the same region-local Valkey backend and
atomically deleted before code exchange. `/readyz` probes both the session and OIDC transaction
namespaces when interactive SSO is enabled.

Run the listener on a private host binding behind the TLS ingress:

```sh
docker run --detach --name myelin-web \
  --read-only \
  --tmpfs /tmp:rw,noexec,nosuid,size=16m \
  --cap-drop ALL \
  --security-opt no-new-privileges:true \
  --env-file /run/secrets/myelin-web.env \
  --publish 127.0.0.1:3000:3000 \
  myelin-web:local
```

The ingress must strip any client-supplied `X-Forwarded-Proto` and replace it with the actual public
scheme. `/healthz` is process liveness. `/readyz` probes the session store with real namespaced
operations and must gate traffic; a Valkey outage returns 503. Send probes through the ingress, or
include its trusted `X-Forwarded-Proto: https` assertion on the private hop. The image declares a
liveness `HEALTHCHECK` and uses `SIGTERM` as its stop signal.

This artifact contains only the web gateway. Deploy the Rust edge, persistent Git storage, and gVisor
guest rootfs through the separate host-native contract in [`edge-deployment.md`](edge-deployment.md).
