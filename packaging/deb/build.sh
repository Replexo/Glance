#!/bin/sh
# Builds dist/glance_<version>_<arch>.deb inside the deb image
set -eu
cd /src
version=$(sed -n "s/^  version: '\(.*\)',/\1/p" meson.build)
arch=$(dpkg --print-architecture)
root=/tmp/pkg
rm -rf /tmp/build "$root"
meson setup /tmp/build --prefix=/usr --buildtype=release -Dupdate_check=true
meson compile -C /tmp/build
DESTDIR="$root" meson install -C /tmp/build
mkdir -p "$root/DEBIAN"
cat > "$root/DEBIAN/control" <<CONTROL
Package: glance
Version: $version
Architecture: $arch
Maintainer: Replexo <196697396+Replexo@users.noreply.github.com>
Depends: libgtk-4-1 (>= 4.20), libadwaita-1-0 (>= 1.8), libgweather-4-0, libseccomp2, liblcms2-2, libfontconfig1, libsoup-3.0-0, libtesseract5, glycin-loaders, bubblewrap, tesseract-ocr-eng
Recommends: tesseract-ocr-spa, tesseract-ocr-fra, tesseract-ocr-deu, tesseract-ocr-por, tesseract-ocr-rus, tesseract-ocr-pol, tesseract-ocr-tur
Section: graphics
Priority: optional
Homepage: https://github.com/Replexo/Glance
Description: View and edit images
 Glance is an image viewer based on GNOME Loupe that adds adjusting,
 markup, resizing and text recognition.
CONTROL
mkdir -p dist
dpkg-deb --root-owner-group --build "$root" "dist/glance_${version}_${arch}.deb"
