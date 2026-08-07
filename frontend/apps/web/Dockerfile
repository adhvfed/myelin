FROM node:22-bookworm-slim@sha256:6c74791e557ce11fc957704f6d4fe134a7bc8d6f5ca4403205b2966bd488f6b3 AS build

ENV PNPM_HOME=/pnpm
ENV PATH=$PNPM_HOME:$PATH
WORKDIR /workspace

RUN corepack enable && corepack prepare pnpm@10.5.2 --activate

COPY frontend/package.json frontend/pnpm-lock.yaml frontend/pnpm-workspace.yaml frontend/.npmrc ./frontend/
COPY frontend/apps/web/package.json ./frontend/apps/web/package.json
COPY frontend/apps/desktop/package.json ./frontend/apps/desktop/package.json
COPY frontend/packages/design-system/package.json ./frontend/packages/design-system/package.json
RUN --mount=type=cache,id=myelin-web-pnpm,target=/pnpm/store \
    pnpm --dir frontend install --frozen-lockfile

COPY frontend ./frontend
COPY design-planning/08-design-system ./design-planning/08-design-system
RUN pnpm --dir frontend --filter @myelin/web... build

FROM node:22-bookworm-slim@sha256:6c74791e557ce11fc957704f6d4fe134a7bc8d6f5ca4403205b2966bd488f6b3 AS runtime

LABEL org.opencontainers.image.source="https://github.com/adhvfed/myelin" \
      org.opencontainers.image.title="Myelin web" \
      org.opencontainers.image.description="Myelin's production SolidStart web gateway"

ENV NODE_ENV=production
ENV HOST=0.0.0.0
ENV PORT=3000
WORKDIR /app

COPY --from=build --chown=node:node /workspace/frontend/apps/web/.output/ ./

USER node
EXPOSE 3000
STOPSIGNAL SIGTERM
HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
  CMD ["node", "-e", "fetch('http://127.0.0.1:3000/healthz',{headers:{'x-forwarded-proto':'https'}}).then(r=>{if(!r.ok)process.exit(1)}).catch(()=>process.exit(1))"]

CMD ["node", "server/index.mjs"]
