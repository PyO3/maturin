set -ex

main() {
    cargo test --target $TARGET --all
    cargo build --target $TARGET --all
}

main
