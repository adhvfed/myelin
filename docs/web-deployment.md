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
MYELIN_PUBLIC_ORIGIN=https://myelin.example
MYELIN_EDGE_URL=http://edge.internal:8080
MYELIN_HSTS=1
```

`REDIS_URL`, `MYELIN_PUBLIC_ORIGIN`, and `MYELIN_EDGE_URL` are mandatory. The public origin must be
HTTPS; the Valkey connection must use TLS; and the edge value must be a credential-free HTTP(S)
origin on the private service network. Enable HSTS only after the public hostname is permanently
HTTPS.

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
