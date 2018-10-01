FROM quay.io/pypa/manylinux1_x86_64

RUN curl https://www.musl-libc.org/releases/musl-1.1.20.tar.gz -o musl.tar.gz \
    && tar -xzf musl.tar.gz \
    && rm -f musl.tar.gz \
    && cd musl-1.1.20 \
    && ./configure \
    && make install -j2
RUN curl https://sh.rustup.rs -sSf | sh -s -- -y
ENV PATH /root/.cargo/bin:$PATH
RUN rustup toolchain add nightly
ADD . /pyo3-pack/
WORKDIR /pyo3-pack
# We can't use the upload feature because either the openssl is too old (native) or the perl version (vendored)
RUN cargo install --path . --no-default-features --features auditwheel
