# syntax=docker/dockerfile:1
# Builder and runtime both use Oracle Linux 9 so glibc matches.
# The `oracle` crate compiles ODPI-C and loads the Instant Client at runtime.

FROM oraclelinux:9 AS dev
ARG RUST_VERSION=1.99.1
RUN dnf -y install gcc git && dnf -y install oracle-instantclient-release-23ai-el9 \
 && dnf -y install oracle-instantclient-basiclite libaio && dnf clean all
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --profile minimal --default-toolchain "$RUST_VERSION" -c rustfmt -c clippy
ENV PATH="/root/.cargo/bin:${PATH}" CARGO_TARGET_DIR=/target
WORKDIR /src

FROM dev AS build
COPY . .
RUN --mount=type=cache,target=/root/.cargo/registry --mount=type=cache,target=/target \
    cargo build --release --locked && cp /target/release/bank /bank

FROM oraclelinux:9-slim AS runtime
RUN microdnf -y install oracle-instantclient-release-23ai-el9 \
 && microdnf -y install oracle-instantclient-basiclite libaio ca-certificates \
 && microdnf clean all && rm -rf /var/cache/yum /var/cache/dnf
COPY --from=build /bank /usr/local/bin/bank
# app.log directory, owned by the runtime user so a fresh named volume inherits it.
RUN mkdir -p /var/log/saltation && chown 10001:10001 /var/log/saltation
USER 10001:10001
ENTRYPOINT ["/usr/local/bin/bank"]
