# x86_64 base
FROM quay.io/pypa/manylinux2014_x86_64 as base-amd64
# x86_64 builder
FROM --platform=$BUILDPLATFORM messense/rust-musl-cross:x86_64-musl as builder-amd64

# aarch64 base
FROM quay.io/pypa/manylinux2014_aarch64 as base-arm64
# aarch64 cross compile builder
FROM --platform=$BUILDPLATFORM messense/rust-musl-cross:aarch64-musl as builder-arm64

ARG TARGETARCH
FROM builder-$TARGETARCH as builder

ENV PATH /root/.cargo/bin:$PATH

# Compile dependencies only for build caching
ADD Cargo.toml /maturin/Cargo.toml
ADD Cargo.lock /maturin/Cargo.lock
RUN --mount=type=cache,target=/root/.cargo/git \
    --mount=type=cache,target=/root/.cargo/registry \
    --mount=type=cache,target=/maturin/target,sharing=locked \
    mkdir /maturin/src && \
    touch  /maturin/src/lib.rs && \
    echo 'fn main() { println!("Dummy") }' > /maturin/src/main.rs && \
    cargo rustc --target $CARGO_BUILD_TARGET --bin maturin --manifest-path /maturin/Cargo.toml --release --features password-storage -- -C link-arg=-s

ADD . /maturin/

# Manually update the timestamps as ADD keeps the local timestamps and cargo would then believe the cache is fresh
RUN touch /maturin/src/lib.rs /maturin/src/main.rs

RUN --mount=type=cache,target=/root/.cargo/git \
    --mount=type=cache,target=/root/.cargo/registry \
    --mount=type=cache,target=/maturin/target,sharing=locked \
    cargo rustc --target $CARGO_BUILD_TARGET --bin maturin --manifest-path /maturin/Cargo.toml --release --features password-storage -- -C link-arg=-s \
    && mv /maturin/target/$CARGO_BUILD_TARGET/release/maturin /usr/bin/maturin

FROM base-$TARGETARCH

ENV PATH /root/.cargo/bin:$PATH
# Add all supported python versions
ENV PATH /opt/python/cp37-cp37m/bin:/opt/python/cp38-cp38/bin:/opt/python/cp39-cp39/bin:/opt/python/cp310-cp310/bin:/opt/python/cp311-cp311/bin:$PATH
# Otherwise `cargo new` errors
ENV USER root

RUN curl --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y \
    && yum install -y libffi-devel openssh-clients \
    && python3.7 -m pip install --no-cache-dir cffi \
    && python3.8 -m pip install --no-cache-dir cffi \
    && python3.9 -m pip install --no-cache-dir cffi \
    && python3.10 -m pip install --no-cache-dir cffi \
    && python3.11 -m pip install --no-cache-dir cffi \
    && mkdir /io

COPY --from=builder /usr/bin/maturin /usr/bin/maturin

WORKDIR /io

ENTRYPOINT ["/usr/bin/maturin"]
