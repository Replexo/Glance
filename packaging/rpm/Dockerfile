# Build environment for the .rpm package
FROM fedora:43
RUN dnf install -y \
        git gcc gcc-c++ clang pkgconf-pkg-config meson ninja-build gettext itstool \
        desktop-file-utils libappstream-glib rpm-build rpmdevtools \
        glib2-devel gtk4-devel libadwaita-devel libgweather4-devel \
        libseccomp-devel lcms2-devel fontconfig-devel libsoup3-devel \
        tesseract-devel leptonica-devel \
    && dnf clean all
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable
ENV PATH=/root/.cargo/bin:$PATH
