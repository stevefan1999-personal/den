FROM ghcr.io/actions/actions-runner:2.337.0@sha256:e5496277be5d09bc968b3d64911b74e219ac4a3f2edce956a3ecf9271bea1ef4

USER root

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential \
        clang \
        cmake \
        file \
        libclang-dev \
        musl-tools \
        nasm \
        perl \
        pkg-config \
        python3 \
        qemu-user \
        util-linux \
    && rm -rf /var/lib/apt/lists/*

USER runner

RUN curl --proto '=https' --tlsv1.2 -fsSLo /tmp/rustup-init \
        https://static.rust-lang.org/rustup/archive/1.29.0/x86_64-unknown-linux-gnu/rustup-init \
    && echo '4acc9acc76d5079515b46346a485974457b5a79893cfb01112423c89aeb5aa10  /tmp/rustup-init' \
        | sha256sum -c - \
    && chmod +x /tmp/rustup-init \
    && /tmp/rustup-init -y --profile minimal --default-toolchain none \
    && rm /tmp/rustup-init

ENV PATH="/home/runner/.cargo/bin:${PATH}"
