#!/bin/sh
# Builds dist/Glance-<version>-<arch>.AppImage inside the deb image
#
# Image loading uses the host's glycin loaders, which sandbox each image.
set -eu
cd /src
version=$(sed -n "s/^  version: '\(.*\)',/\1/p" meson.build)
appdir=/tmp/AppDir
rm -rf /tmp/build-appimage "$appdir"
meson setup /tmp/build-appimage --prefix=/usr --buildtype=release -Dupdate_check=true
meson compile -C /tmp/build-appimage
DESTDIR="$appdir" meson install -C /tmp/build-appimage

# Language data for text recognition, found through TESSDATA_PREFIX
mkdir -p "$appdir/usr/share/tessdata"
cp /opt/tessdata/*.traineddata "$appdir/usr/share/tessdata/"
mkdir -p "$appdir/apprun-hooks"
cat > "$appdir/apprun-hooks/glance.sh" <<'HOOK'
export TESSDATA_PREFIX="$APPDIR/usr/share/tessdata"
HOOK

# The GTK plugin copies GTK's module folder, which Ubuntu only has with
# optional modules installed
mkdir -p "$(pkg-config --variable=libdir gtk4)/gtk-4.0/4.0.0"

export DEPLOY_GTK_VERSION=4 OUTPUT="/src/dist/Glance-$version-$(uname -m).AppImage"
export APPIMAGE_EXTRACT_AND_RUN=1 LDAI_RUNTIME_FILE=/opt/linuxdeploy/runtime-x86_64 LDAI_UPDATE_INFORMATION="gh-releases-zsync|Replexo|Glance|latest|Glance-*$(uname -m).AppImage.zsync"
mkdir -p /src/dist
cd /opt/linuxdeploy
./linuxdeploy-x86_64.AppImage --appdir "$appdir" \
    --desktop-file "$appdir/usr/share/applications/io.github.Replexo.Glance.desktop" \
    --icon-file "$appdir/usr/share/icons/hicolor/scalable/apps/io.github.Replexo.Glance.svg" \
    --plugin gtk --output appimage
mv Glance*.zsync /src/dist/ 2>/dev/null || true
