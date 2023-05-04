FROM rust:slim-bullseye

RUN set -eux; \
    apt-get update && \
    apt-get install -y --no-install-recommends \
    pkgconf \
    clang \
    make \
    cmake \
    curl \
    git \
    protobuf-compiler \
    libfuse3-dev \
    libssl-dev \
    libkrb5-dev \
    libclamav-dev \
    libx11-dev \
    libxtst-dev \
    libdbus-1-dev \
    libudev-dev \
    libseccomp-dev \
    live-build \
    dpkg-dev && \
    cargo install cargo-deb

WORKDIR /SRC
