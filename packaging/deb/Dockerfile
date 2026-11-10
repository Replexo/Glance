# Build environment for the .deb package
FROM ubuntu:25.10
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates curl git build-essential pkg-config clang \
        meson ninja-build gettext itstool desktop-file-utils appstream \
        libglib2.0-dev-bin libgtk-4-dev libadwaita-1-dev libgweather-4-dev \
        libseccomp-dev liblcms2-dev libfontconfig-dev libsoup-3.0-dev \
        libtesseract-dev libleptonica-dev \
        dpkg-dev file \
        # AppImage
        libfuse2t64 squashfs-tools patchelf librsvg2-common \
    && rm -rf /var/lib/apt/lists/*
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable
ENV PATH=/root/.cargo/bin:$PATH
# AppImage tools and the language data it carries
RUN mkdir -p /opt/linuxdeploy /opt/tessdata && cd /opt/linuxdeploy \
    && curl -fsSLO https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage \
    && curl -fsSLO https://raw.githubusercontent.com/linuxdeploy/linuxdeploy-plugin-gtk/master/linuxdeploy-plugin-gtk.sh \
    && curl -fsSLO https://github.com/AppImage/type2-runtime/releases/download/continuous/runtime-x86_64 \
    && chmod +x linuxdeploy-x86_64.AppImage linuxdeploy-plugin-gtk.sh \
    && for lang in eng spa fra deu por rus pol tur; do \
        curl -fsSL -o /opt/tessdata/$lang.traineddata \
            https://github.com/tesseract-ocr/tessdata_fast/raw/main/$lang.traineddata; \
    done
