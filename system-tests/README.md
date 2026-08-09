# Myelin backend system tests

This package treats Myelin as an external system. It sends HTTP requests to the running
edge and observes durable, cross-service outcomes; it does not import application crates
or frontend modules.

Run it through Service Federation so every URL, credential, port, and dependency belongs
to the current checkout:

```sh
fed test:system
```

Fed supplies the ports and credentials allocated to the current checkout, waits for dependency
health checks, and applies its borrow-or-own lifecycle: an existing development stack stays up,
while services started for the command are stopped on exit. The suite uses a dedicated tenant and
unique resource names so repeated runs do not depend on pre-existing product data. Pass Vitest
selectors after `--`, for example:

```sh
fed test:system -- tests/platform.system.test.ts
```

`pnpm --dir system-tests typecheck` is service-independent. Running `pnpm test` directly
is intentionally unsupported because it would bypass Fed's allocated environment.
