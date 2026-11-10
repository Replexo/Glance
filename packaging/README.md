# Release packages

```sh
./packaging/build.sh              # all of them
./packaging/build.sh deb appimage # some of them
```

Packages land in `dist/`, with a `SHA256SUMS` file that the update checker uses to verify downloads. Upload both to the GitHub release.

| Package | Built in | Recipe |
|---|---|---|
| `.deb` | Docker, Ubuntu 25.10 | `deb/` |
| `.rpm` | Docker, Fedora 43 | `rpm/` |
| `.pkg.tar.zst` | Docker, Arch Linux | `arch/` |
| `.AppImage` | Docker, Ubuntu 25.10 | `appimage/` |

The Docker images are built on the first run and cached; Cargo downloads are kept in `packaging/.cargo-cache`. The deb, rpm and AppImage are built with the update checker on. The Arch package keeps it off, since pacman updates it.

The AppImage carries Tesseract language data but uses the host's glycin loaders to open images, so `glycin-loaders` must be installed.

The Flatpak bundle is built from `build-aux/flatpak/io.github.Replexo.Glance.json`. Keep the update checker off in the Flathub manifest: Flathub doesn't allow apps to update themselves.
