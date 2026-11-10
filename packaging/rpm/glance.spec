Name:           glance
Version:        %{app_version}
Release:        1%{?dist}
Summary:        View and edit images
License:        GPL-3.0-or-later
URL:            https://github.com/Replexo/Glance

Requires:       gtk4 >= 4.20
Requires:       libadwaita >= 1.8
Requires:       glycin-loaders
Requires:       bubblewrap
Requires:       tesseract
Requires:       tesseract-langpack-eng
Recommends:     tesseract-langpack-spa tesseract-langpack-fra tesseract-langpack-deu
Recommends:     tesseract-langpack-por tesseract-langpack-rus tesseract-langpack-pol
Recommends:     tesseract-langpack-tur

%description
Glance is an image viewer based on GNOME Loupe that adds adjusting,
markup, resizing and text recognition.

%build
meson setup %{_builddir}/build %{src} --prefix=%{_prefix} --buildtype=release -Dupdate_check=true
meson compile -C %{_builddir}/build

%install
DESTDIR=%{buildroot} meson install -C %{_builddir}/build
install -Dm644 %{src}/COPYING.md %{buildroot}%{_datadir}/licenses/glance/COPYING.md
%find_lang glance --with-gnome --all-name

%files -f glance.lang
%{_datadir}/licenses/glance/
%{_bindir}/glance
%{_datadir}/applications/*.desktop
%{_datadir}/metainfo/*.xml
%{_datadir}/icons/hicolor/*/apps/*
%{_datadir}/glib-2.0/schemas/*.xml
%{_datadir}/dbus-1/services/*.service

%changelog
* Tue Nov 10 2026 Replexo <196697396+Replexo@users.noreply.github.com> - 1.0-1
- See https://github.com/Replexo/Glance/releases
