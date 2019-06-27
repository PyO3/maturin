#!/usr/bin/env bash
# Based on https://github.com/sharkdp/hyperfine/blob/master/ci/before_deploy.bash
set -ex

make_archive() {
    # We don't care for manylinux compliance for the downloads, so we can use the keyring.
    cargo build --release --target $TARGET --features "password-storage"
    pushd target/$TARGET/release/
    # You can add more files to the archive by adding them to this line
    tar czf $TRAVIS_BUILD_DIR/$BINARY_NAME-$TRAVIS_TAG-$TARGET.tar.gz $BINARY_NAME
    popd
}

make_deb() {
    local tempdir
    local architecture
    local version
    local dpkgname
    local conflictname

    case $TARGET in
        x86_64*)
            architecture=amd64
            ;;
        i686*)
            architecture=i386
            ;;
        *)
            echo "ERROR: unknown target" >&2
            return 1
            ;;
    esac
    version=${TRAVIS_TAG#v}
    if [[ $TARGET = *musl* ]]; then
      dpkgname=$BINARY_NAME-musl
      conflictname=$BINARY_NAME
    else
      dpkgname=$BINARY_NAME
      conflictname=$BINARY_NAME-musl
    fi

    tempdir=$(mktemp -d 2>/dev/null || mktemp -d -t tmp)

    # copy the main binary
    install -Dm755 "target/$TARGET/release/$BINARY_NAME" "$tempdir/usr/bin/$BINARY_NAME"
    strip "$tempdir/usr/bin/$BINARY_NAME"

    # readme and license
    install -Dm644 Readme.md "$tempdir/usr/share/doc/$BINARY_NAME/Readme.md"
    install -Dm644 license-mit "$tempdir/usr/share/doc/$BINARY_NAME/license-mit"
    install -Dm644 license-apache "$tempdir/usr/share/doc/$BINARY_NAME/license-apache"

    # Control file
    mkdir "$tempdir/DEBIAN"
    cat > "$tempdir/DEBIAN/control" <<EOF
Package: $dpkgname
Version: $version
Section: utils
Priority: optional
Maintainer: konstin <konstin@mailbox.org>
Architecture: $architecture
Provides: $BINARY_NAME
Conflicts: $conflictname
Description: Build and publish crates with pyo3, rust-cpython and cffi bindings as well as rust binaries as python packages
EOF

    fakeroot dpkg-deb --build "$tempdir" "$TRAVIS_BUILD_DIR/${dpkgname}_${version}_${architecture}.deb"
}

upload_to_pypi() {
    if [[ $TARGET = x86_64-unknown-linux-musl ]]; then
        cargo run -- publish -u konstin -b bin --target $TARGET
    else
        cargo run -- publish -u konstin -b bin --target $TARGET --no-sdist
    fi
}

main() {
    make_archive
    if [[ $TARGET = *linux* ]]; then
      make_deb
    fi
    upload_to_pypi
}

main
