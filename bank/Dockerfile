# syntax=docker/dockerfile:1
# Builder and runtime both use Oracle Linux 9 so glibc matches.
# The `oracledb` driver is pure Rust and needs no Oracle client.

FROM oraclelinux:9 AS dev
ENV ORA_SDTZ=+09:00
ARG RUST_VERSION=1.99.1
RUN dnf -y install gcc && dnf clean all
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --profile minimal --default-toolchain "$RUST_VERSION" -c rustfmt -c clippy
ENV PATH="/root/.cargo/bin:${PATH}" CARGO_TARGET_DIR=/target
WORKDIR /src

FROM dev AS build
COPY . .
RUN --mount=type=cache,target=/root/.cargo/registry --mount=type=cache,id=saltation-bank-build-guarded-v1,target=/target,sharing=locked \
    cargo clean --release --package bank --locked && cargo build --release --locked && cp /target/release/bank /bank

FROM oraclelinux:9-slim AS runtime
ENV ORA_SDTZ=+09:00
RUN microdnf -y install ca-certificates \
 && microdnf clean all && rm -rf /var/cache/yum /var/cache/dnf
COPY --from=build /bank /usr/local/bin/bank
# app.log directory, owned by the runtime user so a fresh named volume inherits it.
RUN mkdir -p /var/log/saltation && chown 10001:10001 /var/log/saltation
USER 10001:10001
ENTRYPOINT ["/usr/local/bin/bank"]
