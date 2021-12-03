FROM quay.io/pypa/manylinux2010_x86_64 as builder

ENV PATH /root/.cargo/bin:$PATH

# Use an explicit version to actually install the version we require instead of using the cache
# It would be even cooler to invalidate the cache depending on when the official rust image changes,
# but I don't know how to do that
RUN curl --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- --default-toolchain 1.57.0 -y

# Compile dependencies only for build caching
ADD Cargo.toml /maturin/Cargo.toml
ADD Cargo.lock /maturin/Cargo.lock
RUN mkdir /maturin/src && \
    touch  /maturin/src/lib.rs && \
    echo 'fn main() { println!("Dummy") }' > /maturin/src/main.rs && \
    cargo rustc --bin maturin --manifest-path /maturin/Cargo.toml --release -- -C link-arg=-s

ADD . /maturin/

# Manually update the timestamps as ADD keeps the local timestamps and cargo would then believe the cache is fresh
RUN touch /maturin/src/lib.rs /maturin/src/main.rs

RUN cargo rustc --bin maturin --manifest-path /maturin/Cargo.toml --release --features password-storage -- -C link-arg=-s \
    && mv /maturin/target/release/maturin /usr/bin/maturin \
    && rm -rf /maturin

FROM quay.io/pypa/manylinux2010_x86_64

ENV PATH /root/.cargo/bin:$PATH
# Add all supported python versions
ENV PATH /opt/python/cp36-cp36m/bin/:/opt/python/cp37-cp37m/bin/:/opt/python/cp38-cp38/bin/:/opt/python/cp39-cp39/bin/:$PATH
# Otherwise `cargo new` errors
ENV USER root

RUN curl --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y \
    && python3.6 -m pip install --no-cache-dir cffi \
    && python3.7 -m pip install --no-cache-dir cffi \
    && python3.8 -m pip install --no-cache-dir cffi \
    && python3.9 -m pip install --no-cache-dir cffi \
    && mkdir /io

COPY --from=builder /usr/bin/maturin /usr/bin/maturin

WORKDIR /io

ENTRYPOINT ["/usr/bin/maturin"]
