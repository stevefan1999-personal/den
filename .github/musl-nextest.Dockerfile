FROM rust:1.98.0-alpine3.22@sha256:2e452153b2dc6bed8ef123c4801cfd3f637e7c092846f974ecdce5e64af3120c

ARG CARGO_NEXTEST_VERSION=0.9.143
ARG CARGO_NEXTEST_SHA256=6d891c18105ec2d33f6e441a4f92b7ccab47ba263e22055c8bb66884243f3389

RUN apk add --no-cache \
        bash \
        build-base \
        cmake \
        curl \
        git \
        linux-headers \
        nasm \
        perl \
        pkgconf \
        su-exec \
    && archive="cargo-nextest-${CARGO_NEXTEST_VERSION}-x86_64-unknown-linux-musl.tar.gz" \
    && curl --proto '=https' --tlsv1.2 -fsSLo "/tmp/${archive}" \
        "https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-${CARGO_NEXTEST_VERSION}/${archive}" \
    && echo "${CARGO_NEXTEST_SHA256}  /tmp/${archive}" | sha256sum -c - \
    && tar -xzf "/tmp/${archive}" -C /usr/local/bin \
    && rm "/tmp/${archive}" \
    && addgroup -S -g 10001 den \
    && adduser -S -D -H -u 10001 -G den den \
    && mkdir /cargo /target \
    && chown den:den /cargo /target

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
