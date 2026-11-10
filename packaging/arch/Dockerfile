# Build environment for the Arch package
FROM archlinux:latest
RUN pacman -Syu --noconfirm --needed base-devel git meson cargo clang itstool appstream \
        gtk4 libadwaita libgweather-4 libseccomp lcms2 fontconfig libsoup3 \
        tesseract tesseract-data-eng glycin bubblewrap \
    && pacman -Scc --noconfirm \
    && useradd -m builder
