#!/usr/bin/env bash
set -ex

host() {
    case "$TRAVIS_OS_NAME" in
        linux)
            echo x86_64-unknown-linux-gnu
            ;;
        osx)
            echo x86_64-apple-darwin
            ;;
    esac
}

main() {
    curl https://sh.rustup.rs -sSf \
      | sh -s -- -y --default-toolchain="$TRAVIS_RUST_VERSION"
    if [ $(host) != "$TARGET" ]; then
        rustup target add $TARGET
    fi
    rustc -V
    cargo -V
}

main
