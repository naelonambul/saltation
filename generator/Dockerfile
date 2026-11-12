# syntax=docker/dockerfile:1
FROM ghcr.io/astral-sh/uv:0.8.17-python3.13-bookworm-slim
ENV UV_COMPILE_BYTECODE=1 UV_LINK_MODE=copy UV_PROJECT_ENVIRONMENT=/opt/venv PATH="/opt/venv/bin:${PATH}"
WORKDIR /app
COPY pyproject.toml uv.lock ./
RUN --mount=type=cache,target=/root/.cache/uv uv sync --frozen --no-dev --no-install-project
COPY saltation_gen saltation_gen
RUN --mount=type=cache,target=/root/.cache/uv uv sync --frozen --no-dev
USER 10001:10001
ENTRYPOINT ["saltation-gen"]
