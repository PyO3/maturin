set -ex

main() {
    curl https://sh.rustup.rs -sSf \
      | sh -s -- -y --default-toolchain="$TRAVIS_RUST_VERSION"
    rustup target add $TARGET
    rustc -V
    cargo -V
}

main
