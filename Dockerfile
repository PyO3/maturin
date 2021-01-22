FROM quay.io/pypa/manylinux2010_x86_64

ENV PATH /root/.cargo/bin:$PATH
# Add all supported python versions
ENV PATH /opt/python/cp36-cp36m/bin/:/opt/python/cp37-cp37m/bin/:/opt/python/cp38-cp38/bin/:/opt/python/cp39-cp39/bin/:$PATH
# Otherwise `cargo new` errors
ENV USER root

RUN curl --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y \
    && python3 -m pip install --no-cache-dir cffi \
    && mkdir /io

ADD . /maturin/

RUN cargo rustc --bin maturin --manifest-path /maturin/Cargo.toml --release -- -C link-arg=-s \
    && mv /maturin/target/release/maturin /usr/bin/maturin \
    && rm -rf /maturin

ADD https://github.com/Kitware/CMake/releases/download/v3.19.1/cmake-3.19.1.tar.gz .
RUN tar -xf cmake-3.19.1.tar.gz && \
    cd cmake-3.19.1 && \
    ./bootstrap -- -DCMAKE_USE_OPENSSL=OFF && \
    make -j8 install && \
    cd .. && \
    rm -r cmake*

WORKDIR /io

ENTRYPOINT ["/usr/bin/maturin"]
