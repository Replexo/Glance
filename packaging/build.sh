#!/bin/sh
# Builds release packages into dist/: ./packaging/build.sh [deb] [rpm] [arch] [appimage]
#
# Packages are built from the current commit, so uncommitted changes are
# left out. Each package builds in a Docker image. Cargo downloads are kept
# in packaging/.cargo-cache.
set -eu
cd "$(dirname "$0")/.."
targets=${*:-deb rpm arch appimage}
mkdir -p dist packaging/.cargo-cache/deb packaging/.cargo-cache/rpm packaging/.cargo-cache/arch
src=$(mktemp -d)
trap 'rm -rf "$src"' EXIT
git archive HEAD | tar -x -C "$src"

in_image() {
    docker build -q -t "glance-build-$1" "packaging/$1" >/dev/null
    cache=/root/.cargo/registry
    [ "$1" = arch ] && cache=/home/builder/.cargo/registry
    docker run --rm -v "$src:/src" -v "$PWD/dist:/src/dist" \
        -v "$PWD/packaging/.cargo-cache/$1:$cache" \
        "glance-build-$1" sh "/src/packaging/$2/build.sh"
}

for target in $targets; do
    case $target in
        deb) in_image deb deb ;;
        rpm) in_image rpm rpm ;;
        appimage) in_image deb appimage ;;
        arch) in_image arch arch ;;
        *) echo "Unknown target $target" >&2; exit 1 ;;
    esac
done
# Files made in Docker belong to root
[ "$(id -u)" = 0 ] || docker run --rm -v "$PWD/dist:/dist" alpine chown -R "$(id -u):$(id -g)" /dist
(cd dist && find . -maxdepth 1 -type f \( -name '*.deb' -o -name '*.rpm' \
    -o -name '*.AppImage' -o -name '*.flatpak' -o -name '*.pkg.tar.zst' \) \
    -printf '%f\0' | sort -z | xargs -0r sha256sum > SHA256SUMS)
ls -l dist
