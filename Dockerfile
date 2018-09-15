FROM quay.io/pypa/manylinux1_x86_64

RUN apt install openssl musl-tools
RUN curl https://sh.rustup.rs -sSf | sh -s -- -y
ENV PATH /root/.cargo/bin:$PATH
RUN rustup toolchain add nightly
RUN mkdir /pyo3-pack
WORKDIR /pyo3-pack
ADD . .
RUN cargo install --path .
