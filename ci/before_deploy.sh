#!/usr/bin/env bash
# Based on https://github.com/sharkdp/hyperfine/blob/master/ci/before_deploy.bash
set -ex

# The CFLAGS="-fno-stack-protector" story is a weird one; On 32 bit, you get errors such as the following without the option
# /usr/bin/ld: apps/openssl: hidden symbol `__stack_chk_fail_local' isn't defined
# I assume that this is a musl bug fixed in 2015 that didn't make into ubuntu 14.04, but that special
# case seems to be documented nowhere else:
# http://git.musl-libc.org/cgit/musl/commit/?id=55d061f031085f24d138664c897791aebe9a2fab
# We can't have a more recent musl on 14.04 (there's no ppa), so we have to disable that feature

make_archive() {
    # We don't care for manylinux compliance for the downloads, so we can use the keyring.
    CFLAGS="-fno-stack-protector" cargo build --release --target $TARGET --features "password-storage musl"
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
Description: A command-line benchmarking tool.
EOF

    fakeroot dpkg-deb --build "$tempdir" "$TRAVIS_BUILD_DIR/${dpkgname}_${version}_${architecture}.deb"
}

upload_to_pypi() {
    # We do care for manylinux compliance for pypi, so we use the musl feature to get static binaries
    CFLAGS="-fno-stack-protector" cargo run -- publish -u konstin --release -b bin --target $TARGET --strip --cargo-extra-args="--features=musl"
}

main() {
    make_archive
    if [[ $TARGET = *linux* ]]; then
      make_deb
    fi
    upload_to_pypi
}

main
