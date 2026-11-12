# syntax=docker/dockerfile:1
# Builds the SPAs and serves them with nginx. Build context: repository root
# (the API client is generated from contracts/).
FROM node:22.20.0-bookworm-slim AS build
ARG VITE_API_BASE=http://localhost:8080
ENV VITE_API_BASE=$VITE_API_BASE CI=true
RUN corepack enable
WORKDIR /repo
COPY contracts contracts
COPY web web
WORKDIR /repo/web
RUN --mount=type=cache,target=/pnpm-store pnpm config set store-dir /pnpm-store && pnpm install --frozen-lockfile && pnpm build

FROM nginx:1.29.1-alpine AS runtime
COPY infra/nginx/nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=build /repo/web/apps/mobile/dist /usr/share/nginx/html/mobile
