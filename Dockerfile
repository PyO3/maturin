FROM quay.io/pypa/manylinux1_x86_64

ENV PATH /root/.cargo/bin:$PATH
# Add all supported python versions
ENV PATH /opt/python/cp27-cp27mu/bin/:/opt/python/cp35-cp35m/bin/:/opt/python/cp36-cp36m/bin/:/opt/python/cp37-cp37m/bin/:$PATH
# Otherwise `cargo new` errors
ENV USER root

RUN curl https://www.musl-libc.org/releases/musl-1.1.20.tar.gz -o musl.tar.gz \
    && tar -xzf musl.tar.gz \
    && rm -f musl.tar.gz \
    && cd musl-1.1.20 \
    && ./configure \
    && make install -j2 \
    && cd .. \
    && rm -rf musl-1.1.20 \
    && curl https://sh.rustup.rs -sSf | sh -s -- -y \
    && rustup toolchain add nightly-2019-08-21 \
    && rustup target add x86_64-unknown-linux-musl \
    && python3 -m pip install --no-cache-dir cffi \
    && mkdir /io

ADD . /maturin/

RUN cargo +nightly-2019-08-21 rustc --bin maturin --manifest-path /maturin/Cargo.toml -- -C link-arg=-s \
    && mv /maturin/target/debug/maturin /usr/bin/maturin \
    && rm -rf /maturin

WORKDIR /io

ENTRYPOINT ["/usr/bin/maturin"]
