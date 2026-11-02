# syntax=docker/dockerfile:1
FROM rust:1.99.1-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends cmake pkg-config libopus-dev python3 && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY scripts/dependency_notices.py ./scripts/dependency_notices.py
RUN cargo build --release --locked
RUN python3 scripts/dependency_notices.py --registry /usr/local/cargo/registry/src

FROM debian:trixie-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates libopus0 pulseaudio pulseaudio-utils && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 cadence && useradd --uid 10001 --gid cadence --no-create-home cadence \
    && mkdir /data && chown cadence:cadence /data && chmod 700 /data
COPY --from=build /app/target/release/cadence /usr/local/bin/cadence
COPY --from=build /app/DEPENDENCY_LICENSES.txt /usr/share/doc/cadence/DEPENDENCY_LICENSES.txt
COPY LICENSE THIRD_PARTY_NOTICES.md /usr/share/doc/cadence/
COPY scripts/container-entrypoint.sh /usr/local/bin/cadence-entrypoint
USER 10001:10001
VOLUME /data
STOPSIGNAL SIGTERM
ENTRYPOINT ["/bin/sh", "/usr/local/bin/cadence-entrypoint"]
