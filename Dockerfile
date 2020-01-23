FROM quay.io/pypa/manylinux1_x86_64

ENV PATH /root/.cargo/bin:$PATH
# Add all supported python versions
ENV PATH /opt/python/cp35-cp35m/bin/:/opt/python/cp36-cp36m/bin/:/opt/python/cp37-cp37m/bin/:$PATH
# Otherwise `cargo new` errors
ENV USER root

RUN curl https://sh.rustup.rs -sSf | sh -s -- -y \
    && rustup toolchain add nightly-2020-01-23 \
    && python3 -m pip install --no-cache-dir cffi \
    && mkdir /io

ADD . /maturin/

RUN cargo +nightly-2020-01-23 rustc --bin maturin --manifest-path /maturin/Cargo.toml -- -C link-arg=-s \
    && mv /maturin/target/debug/maturin /usr/bin/maturin \
    && rm -rf /maturin

WORKDIR /io

ENTRYPOINT ["/usr/bin/maturin"]
