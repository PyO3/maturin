#!/usr/bin/env bash
set -ex

main() {
    cargo test --target $TARGET
    cargo build --target $TARGET
}

main
