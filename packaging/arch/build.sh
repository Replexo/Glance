#!/bin/sh
# Builds dist/glance-<version>-<rel>-<arch>.pkg.tar.zst inside the arch image
set -eu
version=$(sed -n "s/^  version: '\(.*\)',/\1/p" /src/meson.build)
work=/home/builder/pkg
rm -rf "$work"
mkdir -p "$work"
# makepkg builds from a git repository of the sources
cp -r /src "$work/glance-src"
rm -rf "$work/glance-src/dist"
git -C "$work/glance-src" init -q
git -C "$work/glance-src" add -A
git -C "$work/glance-src" -c user.name=build -c user.email=build@localhost commit -qm build
sed -e "s/^pkgver=.*/pkgver=$version/" \
    -e "s|^source=.*|source=(\"glance::git+file://$work/glance-src\")|" \
    /src/packaging/arch/PKGBUILD > "$work/PKGBUILD"
chown -R builder "$work"
cd "$work"
su builder -c "CARGO_HOME=/home/builder/.cargo makepkg -f"
cp glance-*.pkg.tar.zst /src/dist/
