FROM quay.io/pypa/manylinux2010_x86_64

ENV PATH /root/.cargo/bin:$PATH
# Add all supported python versions
ENV PATH /opt/python/cp36-cp36m/bin/:/opt/python/cp37-cp37m/bin/:/opt/python/cp38-cp38/bin/:/opt/python/cp39-cp39/bin/:$PATH
# Otherwise `cargo new` errors
ENV USER root

RUN curl --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y \
    && python3 -m pip install --no-cache-dir cffi \
    && mkdir /io

# Compile dependencies only for build caching
ADD Cargo.toml /maturin/Cargo.toml
ADD Cargo.lock /maturin/Cargo.lock
RUN mkdir /maturin/src && \
    touch  /maturin/src/lib.rs && \
    echo 'fn main() { println!("Dummy") }' > /maturin/src/main.rs && \
    cargo rustc --bin maturin --manifest-path /maturin/Cargo.toml --release -- -C link-arg=-s

ADD . /maturin/

# Add keeps the local timestamp, so cargo otherwise believes the cache is fresh
RUN touch /maturin/src/lib.rs /maturin/src/main.rs

RUN cargo rustc --bin maturin --manifest-path /maturin/Cargo.toml --release -- -C link-arg=-s \
    && mv /maturin/target/release/maturin /usr/bin/maturin \
    && rm -rf /maturin

WORKDIR /io

ENTRYPOINT ["/usr/bin/maturin"]
