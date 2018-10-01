# https://stackoverflow.com/a/40966234/3549270
# docker build -f Dockerfile.build-context -t build-context .
# docker run --rm -it build-context

FROM busybox
COPY . /build-context
WORKDIR /build-context
CMD find .
