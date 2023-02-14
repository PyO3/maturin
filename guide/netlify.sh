#!/usr/bin/env bash
set -ex

MDBOOK_VER="0.4.27"

pushd guide

curl -L "https://github.com/rust-lang/mdBook/releases/download/v$MDBOOK_VER/mdbook-v$MDBOOK_VER-x86_64-unknown-linux-musl.tar.gz" | tar xvz

rustup default stable
cargo install mdbook-catppuccin

mdbook-catppuccin install
./mdbook build

popd
