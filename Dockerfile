# x86_64 base
FROM quay.io/pypa/manylinux2014_x86_64 as base-amd64
# x86_64 builder
FROM --platform=$BUILDPLATFORM quay.io/pypa/manylinux2014_x86_64 as builder-amd64
ENV CARGO_BUILD_TARGET=x86_64-unknown-linux-gnu

# aarch64 base
FROM quay.io/pypa/manylinux2014_aarch64 as base-arm64
# aarch64 cross compile builder
FROM --platform=$BUILDPLATFORM quay.io/pypa/manylinux2014_x86_64 as builder-arm64
RUN yum makecache && yum install -y gcc-aarch64-linux-gnu gcc-c++-aarch64-linux-gnu glibc-headers
ENV CARGO_BUILD_TARGET=aarch64-unknown-linux-gnu \
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
    CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
    CXX_aarch64_unknown_linux_gnu=aarch64-linux-gnu-g++

ARG TARGETARCH
FROM builder-$TARGETARCH as builder

ENV PATH /root/.cargo/bin:$PATH

# Use an explicit version to actually install the version we require instead of using the cache
# It would be even cooler to invalidate the cache depending on when the official rust image changes,
# but I don't know how to do that
RUN curl --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- --default-toolchain 1.64.0 -y --target $CARGO_BUILD_TARGET

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
ENV PATH /opt/python/cp37-cp37m/bin:/opt/python/cp38-cp38/bin:/opt/python/cp39-cp39/bin:/opt/python/cp310-cp310/bin:$PATH
# Otherwise `cargo new` errors
ENV USER root

RUN curl --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y \
    && yum install -y libffi-devel \
    && python3.7 -m pip install --no-cache-dir cffi \
    && python3.8 -m pip install --no-cache-dir cffi \
    && python3.9 -m pip install --no-cache-dir cffi \
    && python3.10 -m pip install --no-cache-dir cffi \
    && mkdir /io

COPY --from=builder /usr/bin/maturin /usr/bin/maturin

WORKDIR /io

ENTRYPOINT ["/usr/bin/maturin"]
