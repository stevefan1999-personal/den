FROM alpine:3.22.1@sha256:4bcff63911fcb4448bd4fdacec207030997caf25e9bea4045fa6c8c44de311d1 AS nextest

ARG CARGO_NEXTEST_VERSION=0.9.143
ARG CARGO_NEXTEST_SHA256=6d891c18105ec2d33f6e441a4f92b7ccab47ba263e22055c8bb66884243f3389

RUN apk add --no-cache curl=8.14.1-r3 \
    && archive="cargo-nextest-${CARGO_NEXTEST_VERSION}-x86_64-unknown-linux-musl.tar.gz" \
    && curl --proto '=https' --tlsv1.2 -fsSLo "/tmp/${archive}" \
        "https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-${CARGO_NEXTEST_VERSION}/${archive}" \
    && printf '%s  %s\n' "${CARGO_NEXTEST_SHA256}" "/tmp/${archive}" \
        > /tmp/cargo-nextest.sha256 \
    && sha256sum -c /tmp/cargo-nextest.sha256 \
    && tar -xzf "/tmp/${archive}" -C /tmp

FROM rust:1.98.0-alpine3.22@sha256:2e452153b2dc6bed8ef123c4801cfd3f637e7c092846f974ecdce5e64af3120c

RUN apk add --no-cache linux-headers=6.14.2-r0 make=4.4.1-r3 su-exec=0.2-r3 \
    && addgroup -S -g 10001 den \
    && adduser -S -D -H -u 10001 -G den den \
    && mkdir /cargo /target \
    && chown den:den /cargo /target

COPY --from=nextest /tmp/cargo-nextest /usr/local/bin/cargo-nextest

ENV CARGO_HOME=/cargo \
    CARGO_INCREMENTAL=0 \
    CARGO_PROFILE_TEST_DEBUG=0 \
    CARGO_TARGET_DIR=/target \
    INSTA_UPDATE=no \
    RUST_BACKTRACE=1 \
    RUSTUP_TOOLCHAIN=1.98.0 \
    RUSTFLAGS="-C target-feature=-crt-static"

WORKDIR /work
COPY --chmod=755 musl-nextest-entrypoint.sh /usr/local/bin/musl-nextest
ENTRYPOINT ["musl-nextest"]
