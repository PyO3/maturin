#!/usr/bin/env bash
# Based on https://github.com/sharkdp/hyperfine/blob/master/ci/before_deploy.bash
set -ex

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
version=${VERSION#refs/tags/v}
if [[ $TARGET == *musl* ]]; then
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
install -Dm644 README.md "$tempdir/usr/share/doc/$BINARY_NAME/README.md"
install -Dm644 license-mit "$tempdir/usr/share/doc/$BINARY_NAME/license-mit"
install -Dm644 license-apache "$tempdir/usr/share/doc/$BINARY_NAME/license-apache"

# Control file
mkdir "$tempdir/DEBIAN"
cat >"$tempdir/DEBIAN/control" <<EOF
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

fakeroot dpkg-deb --build "$tempdir" "${dpkgname}_${version}_${architecture}.deb"
