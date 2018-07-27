#!/usr/bin/env bash
set -ex

main() {
    cross build --bin pyo3-pack --target $TARGET --release

    mkdir stage

    cp target/$TARGET/release/pyo3-pack stage/

    cd stage
    tar czf ../pyo3-pack-$TRAVIS_TAG-$TARGET.tar.gz *
    cd ..

    rm -rf stage
}

main
